//! Kubernetes access: a worker thread polls read-only and streams `Update`s to the UI.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use k8s_openapi::api::core::v1::{Event, Pod};
use k9s_core::{ClusterInfo, ClusterStats, Kind, TableData, Update};
use k9s_render::metrics::{PodMetrics, Usage, parse_cpu_milli, parse_mem_bytes};
use kube::{
    Api, Client, Config,
    api::{ApiResource, DynamicObject, GroupVersionKind, ListParams},
    config::{KubeConfigOptions, Kubeconfig},
};

const REFRESH: Duration = Duration::from_secs(2);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Default)]
pub struct Options {
    pub context: Option<String>,
    pub namespace: Option<String>,
    pub all_namespaces: bool,
    pub kind: Kind,
}

/// Starts the worker thread. Dropping the receiver stops it.
pub fn spawn(options: Options) -> async_channel::Receiver<Update> {
    let (tx, rx) = async_channel::bounded(4);
    let spawned = std::thread::Builder::new()
        .name("k9s-kube".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            match runtime {
                Ok(rt) => rt.block_on(run(options, tx)),
                Err(e) => {
                    let _ = tx.send_blocking(Update::Error(format!("runtime: {e}")));
                }
            }
        });
    if spawned.is_err() {
        // The receiver is returned either way; the UI will simply see no updates.
    }
    rx
}

async fn run(options: Options, tx: async_channel::Sender<Update>) {
    let (client, mut info) = match connect(&options).await {
        Ok(v) => v,
        Err(e) => {
            let _ = tx.send(Update::Error(format!("connect: {e}"))).await;
            return;
        }
    };
    if let Ok(v) = client.apiserver_version().await {
        info.k8s_rev = v.git_version;
    }
    if tx.send(Update::Info(info.clone())).await.is_err() {
        return;
    }
    let ar = ApiResource::from_gvk_with_plural(
        &GroupVersionKind::gvk("metrics.k8s.io", "v1beta1", "PodMetrics"),
        "pods",
    );
    let metrics_api: Api<DynamicObject> = if info.all_namespaces {
        Api::all_with(client.clone(), &ar)
    } else {
        Api::namespaced_with(client.clone(), &info.namespace, &ar)
    };
    let mut backoff = REFRESH;
    loop {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
        let update = match list_table(&client, &info, options.kind, &metrics_api, now).await {
            Ok(table) => {
                backoff = REFRESH;
                Update::Table(table)
            }
            Err(e) => {
                backoff = (backoff * 2).min(MAX_BACKOFF);
                Update::Error(e)
            }
        };
        if matches!(update, Update::Table(_)) {
            let stats = fetch_stats(&client).await;
            let _ = tx.try_send(Update::Stats(stats));
        }
        // A full queue means the UI is behind; drop this snapshot rather than block.
        match tx.try_send(update) {
            Err(async_channel::TrySendError::Closed(_)) => return,
            Ok(()) | Err(async_channel::TrySendError::Full(_)) => {}
        }
        tokio::time::sleep(backoff).await;
    }
}

async fn items<K>(client: &Client, info: &ClusterInfo) -> Result<Vec<K>, String>
where
    K: kube::Resource<Scope = k8s_openapi::NamespaceResourceScope>
        + Clone
        + k8s_openapi::serde::de::DeserializeOwned
        + std::fmt::Debug,
    K::DynamicType: Default,
{
    let api: Api<K> = if info.all_namespaces {
        Api::all(client.clone())
    } else {
        Api::namespaced(client.clone(), &info.namespace)
    };
    let list = api
        .list(&ListParams::default())
        .await
        .map_err(|e| e.to_string())?;
    Ok(list.items)
}

async fn list_table(
    client: &Client,
    info: &ClusterInfo,
    kind: Kind,
    metrics_api: &Api<DynamicObject>,
    now: i64,
) -> Result<TableData, String> {
    use k8s_openapi::api::{apps::v1 as apps, batch::v1 as batch, core::v1 as core};
    use k9s_render::resources as r;
    let all = info.all_namespaces;
    Ok(match kind {
        Kind::Pod => {
            let pods = items::<Pod>(client, info).await?;
            // Without metrics-server this stays `None` and the metric columns are hidden.
            let metrics = fetch_metrics(metrics_api).await;
            k9s_render::pod::render(&pods, metrics.as_ref(), all, now)
        }
        Kind::Deployment => {
            r::deployments(&items::<apps::Deployment>(client, info).await?, all, now)
        }
        Kind::StatefulSet => {
            r::stateful_sets(&items::<apps::StatefulSet>(client, info).await?, all, now)
        }
        Kind::DaemonSet => r::daemon_sets(&items::<apps::DaemonSet>(client, info).await?, all, now),
        Kind::ReplicaSet => {
            r::replica_sets(&items::<apps::ReplicaSet>(client, info).await?, all, now)
        }
        Kind::Job => r::jobs(&items::<batch::Job>(client, info).await?, all, now),
        Kind::CronJob => r::cron_jobs(&items::<batch::CronJob>(client, info).await?, all, now),
        Kind::Service => r::services(&items::<core::Service>(client, info).await?, all, now),
        Kind::Ingress => r::ingresses(
            &items::<k8s_openapi::api::networking::v1::Ingress>(client, info).await?,
            all,
            now,
        ),
        Kind::ConfigMap => r::config_maps(&items::<core::ConfigMap>(client, info).await?, all, now),
        Kind::Secret => r::secrets(&items::<core::Secret>(client, info).await?, all, now),
        Kind::PersistentVolumeClaim => r::persistent_volume_claims(
            &items::<core::PersistentVolumeClaim>(client, info).await?,
            all,
            now,
        ),
        Kind::Event => r::events(&items::<Event>(client, info).await?, all, now),
        Kind::Node => {
            let api: Api<core::Node> = Api::all(client.clone());
            let list = api
                .list(&ListParams::default())
                .await
                .map_err(|e| e.to_string())?;
            r::nodes(&list.items, all, now)
        }
    })
}

/// Group, version, kind and plural used for dynamic get and delete.
fn api_resource(kind: Kind) -> ApiResource {
    let (group, version, name, plural) = match kind {
        Kind::Pod => ("", "v1", "Pod", "pods"),
        Kind::Deployment => ("apps", "v1", "Deployment", "deployments"),
        Kind::StatefulSet => ("apps", "v1", "StatefulSet", "statefulsets"),
        Kind::DaemonSet => ("apps", "v1", "DaemonSet", "daemonsets"),
        Kind::ReplicaSet => ("apps", "v1", "ReplicaSet", "replicasets"),
        Kind::Job => ("batch", "v1", "Job", "jobs"),
        Kind::CronJob => ("batch", "v1", "CronJob", "cronjobs"),
        Kind::Service => ("", "v1", "Service", "services"),
        Kind::Ingress => ("networking.k8s.io", "v1", "Ingress", "ingresses"),
        Kind::ConfigMap => ("", "v1", "ConfigMap", "configmaps"),
        Kind::Secret => ("", "v1", "Secret", "secrets"),
        Kind::PersistentVolumeClaim => {
            ("", "v1", "PersistentVolumeClaim", "persistentvolumeclaims")
        }
        Kind::Event => ("", "v1", "Event", "events"),
        Kind::Node => ("", "v1", "Node", "nodes"),
    };
    ApiResource::from_gvk_with_plural(&GroupVersionKind::gvk(group, version, name), plural)
}

fn dynamic_api(client: Client, kind: Kind, namespace: &str) -> Api<DynamicObject> {
    let ar = api_resource(kind);
    if kind.namespaced() && !namespace.is_empty() {
        Api::namespaced_with(client, namespace, &ar)
    } else {
        Api::all_with(client, &ar)
    }
}

async fn connect(options: &Options) -> Result<(Client, ClusterInfo), Box<dyn std::error::Error>> {
    let kubeconfig = Kubeconfig::read()?;
    let context_name = options
        .context
        .clone()
        .or_else(|| kubeconfig.current_context.clone())
        .unwrap_or_default();
    let named = kubeconfig.contexts.iter().find(|c| c.name == context_name);
    let ctx = named.and_then(|c| c.context.as_ref());
    let cluster = ctx.map(|c| c.cluster.clone()).unwrap_or_default();
    let user = ctx.and_then(|c| c.user.clone()).unwrap_or_default();
    let ctx_namespace = ctx.and_then(|c| c.namespace.clone());

    let config = Config::from_custom_kubeconfig(
        kubeconfig,
        &KubeConfigOptions {
            context: Some(context_name.clone()),
            ..Default::default()
        },
    )
    .await?;
    let namespace = options
        .namespace
        .clone()
        .or(ctx_namespace)
        .unwrap_or_else(|| config.default_namespace.clone());
    let client = Client::try_from(config)?;
    let info = ClusterInfo {
        context: context_name,
        cluster,
        user,
        namespace,
        all_namespaces: options.all_namespaces,
        k8s_rev: String::new(),
    };
    Ok((client, info))
}

async fn fetch_metrics(api: &Api<DynamicObject>) -> Option<PodMetrics> {
    let list = api.list(&ListParams::default()).await.ok()?;
    let mut out = PodMetrics::new();
    for item in list.items {
        let ns = item.metadata.namespace.clone().unwrap_or_default();
        let name = item.metadata.name.clone().unwrap_or_default();
        let mut usage = Usage::default();
        for c in item.data.get("containers")?.as_array()? {
            let u = c.get("usage");
            let q = |key: &str| u.and_then(|u| u.get(key)).and_then(|v| v.as_str());
            usage.cpu_milli += q("cpu").and_then(parse_cpu_milli).unwrap_or(0);
            usage.mem_bytes += q("memory").and_then(parse_mem_bytes).unwrap_or(0);
        }
        out.insert(format!("{ns}/{name}"), usage);
    }
    Some(out)
}

/// Context names from the kubeconfig and the current one.
pub fn list_contexts() -> (Vec<String>, Option<String>) {
    Kubeconfig::read().map_or((Vec::new(), None), |k| {
        (
            k.contexts.into_iter().map(|c| c.name).collect(),
            k.current_context,
        )
    })
}

/// Lists namespaces once on a helper thread.
pub fn list_namespaces(options: Options) -> async_channel::Receiver<Result<Vec<String>, String>> {
    let (tx, rx) = async_channel::bounded(1);
    let spawned = std::thread::Builder::new()
        .name("k9s-ns".into())
        .spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())
                .and_then(|rt| rt.block_on(namespaces(&options)));
            let _ = tx.send_blocking(result);
        });
    if spawned.is_err() {
        // The receiver simply sees a closed channel.
    }
    rx
}

async fn namespaces(options: &Options) -> Result<Vec<String>, String> {
    let (client, _) = connect(options).await.map_err(|e| e.to_string())?;
    let api: Api<k8s_openapi::api::core::v1::Namespace> = Api::all(client);
    let list = api
        .list(&ListParams::default())
        .await
        .map_err(|e| e.to_string())?;
    let mut names: Vec<String> = list
        .items
        .into_iter()
        .filter_map(|n| n.metadata.name)
        .collect();
    names.sort();
    Ok(names)
}

async fn fetch_stats(client: &Client) -> ClusterStats {
    let ar = ApiResource::from_gvk_with_plural(
        &GroupVersionKind::gvk("metrics.k8s.io", "v1beta1", "NodeMetrics"),
        "nodes",
    );
    let usage_api: Api<DynamicObject> = Api::all_with(client.clone(), &ar);
    let nodes: Api<k8s_openapi::api::core::v1::Node> = Api::all(client.clone());
    let (Ok(usage), Ok(nodes)) = (
        usage_api.list(&ListParams::default()).await,
        nodes.list(&ListParams::default()).await,
    ) else {
        return ClusterStats::default();
    };
    let (mut cpu_used, mut mem_used) = (0, 0);
    for item in &usage.items {
        let q = |key: &str| {
            item.data
                .get("usage")
                .and_then(|u| u.get(key))
                .and_then(|v| v.as_str())
        };
        cpu_used += q("cpu").and_then(parse_cpu_milli).unwrap_or(0);
        mem_used += q("memory").and_then(parse_mem_bytes).unwrap_or(0);
    }
    let (mut cpu_total, mut mem_total) = (0, 0);
    for node in &nodes.items {
        let alloc = node.status.as_ref().and_then(|s| s.allocatable.as_ref());
        let q = |key: &str| alloc.and_then(|a| a.get(key)).map(|q| q.0.as_str());
        cpu_total += q("cpu").and_then(parse_cpu_milli).unwrap_or(0);
        mem_total += q("memory").and_then(parse_mem_bytes).unwrap_or(0);
    }
    let pct =
        |used: i64, total: i64| (total > 0).then(|| u32::try_from(used * 100 / total).unwrap_or(0));
    ClusterStats {
        cpu: pct(cpu_used, cpu_total),
        mem: pct(mem_used, mem_total),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextKind {
    Yaml,
    Describe,
}

const LOG_TAIL: i64 = 500;

/// Fetches a read-only text view of one pod on a helper thread.
pub fn fetch_text(
    options: Options,
    kind: TextKind,
    resource: Kind,
    namespace: String,
    name: String,
) -> async_channel::Receiver<Result<String, String>> {
    let (tx, rx) = async_channel::bounded(1);
    let spawned = std::thread::Builder::new()
        .name("k9s-text".into())
        .spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())
                .and_then(|rt| {
                    rt.block_on(async {
                        if kind == TextKind::Describe {
                            describe(&options, resource, &namespace, &name).await
                        } else {
                            yaml(&options, resource, &namespace, &name).await
                        }
                    })
                });
            let _ = tx.send_blocking(result);
        });
    if spawned.is_err() {
        // The receiver simply sees a closed channel.
    }
    rx
}

async fn describe(
    options: &Options,
    kind: Kind,
    namespace: &str,
    name: &str,
) -> Result<String, String> {
    let (client, _) = connect(options).await.map_err(|e| e.to_string())?;
    let object = dynamic_api(client.clone(), kind, namespace)
        .get(name)
        .await
        .map_err(|e| e.to_string())?;
    let kind_name = api_resource(kind).kind;
    let mut selector = format!("involvedObject.name={name},involvedObject.kind={kind_name}");
    if let Some(uid) = &object.metadata.uid {
        selector.push_str(&format!(",involvedObject.uid={uid}"));
    }
    let events: Api<Event> = if kind.namespaced() {
        Api::namespaced(client.clone(), namespace)
    } else {
        Api::namespaced(client.clone(), "default")
    };
    // Missing event permissions should not hide the object itself.
    let events = events
        .list(&ListParams::default().fields(&selector))
        .await
        .map(|l| l.items)
        .unwrap_or_default();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
    if kind == Kind::Pod {
        let pods: Api<Pod> = Api::namespaced(client, namespace);
        let pod = pods.get(name).await.map_err(|e| e.to_string())?;
        return Ok(k9s_render::describe::pod(&pod, &events, now));
    }
    let manifest = yaml_of(&object)?;
    Ok(k9s_render::describe::generic(&manifest, &events, now))
}

fn yaml_of(object: &DynamicObject) -> Result<String, String> {
    let mut value = serde_json::to_value(object).map_err(|e| e.to_string())?;
    if let Some(meta) = value.get_mut("metadata").and_then(|m| m.as_object_mut()) {
        meta.remove("managedFields");
    }
    serde_yaml::to_string(&value).map_err(|e| e.to_string())
}

async fn yaml(
    options: &Options,
    kind: Kind,
    namespace: &str,
    name: &str,
) -> Result<String, String> {
    let (client, _) = connect(options).await.map_err(|e| e.to_string())?;
    let object = dynamic_api(client, kind, namespace)
        .get(name)
        .await
        .map_err(|e| e.to_string())?;
    yaml_of(&object)
}

/// Container table of one pod, delivered once.
pub fn list_containers(
    options: Options,
    namespace: String,
    name: String,
) -> async_channel::Receiver<Result<TableData, String>> {
    let (tx, rx) = async_channel::bounded(1);
    let spawned = std::thread::Builder::new()
        .name("k9s-containers".into())
        .spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())
                .and_then(|rt| rt.block_on(containers(&options, &namespace, &name)));
            let _ = tx.send_blocking(result);
        });
    if spawned.is_err() {
        // The receiver simply sees a closed channel.
    }
    rx
}

async fn containers(options: &Options, namespace: &str, name: &str) -> Result<TableData, String> {
    let (client, _) = connect(options).await.map_err(|e| e.to_string())?;
    let api: Api<Pod> = Api::namespaced(client, namespace);
    let pod = api.get(name).await.map_err(|e| e.to_string())?;
    let mut table = k9s_render::container::render(&pod);
    // The default container goes first among the regular ones, as in `kubectl logs`.
    let default = pod
        .metadata
        .annotations
        .as_ref()
        .and_then(|a| a.get("kubectl.kubernetes.io/default-container"));
    if let Some(d) = default
        && let Some(i) = table.rows.iter().position(|r| r.id == *d)
    {
        let first_regular = table
            .rows
            .iter()
            .position(|r| r.cells.get(4).is_some_and(|v| v == "false"));
        if let Some(f) = first_regular.filter(|f| *f < i) {
            let row = table.rows.remove(i);
            table.rows.insert(f, row);
        }
    }
    Ok(table)
}

/// Streams log lines (follow, or a one-shot snapshot of the previous instance).
/// Dropping the receiver stops the worker. A terminal `Err` ends the stream.
pub fn stream_logs(
    options: Options,
    namespace: String,
    name: String,
    container: String,
    previous: bool,
) -> async_channel::Receiver<Result<String, String>> {
    let (tx, rx) = async_channel::bounded(256);
    let spawned = std::thread::Builder::new()
        .name("k9s-logs".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            match rt {
                Ok(rt) => rt.block_on(logs(&options, &namespace, &name, &container, previous, &tx)),
                Err(e) => {
                    let _ = tx.send_blocking(Err(e.to_string()));
                }
            }
        });
    if spawned.is_err() {
        // The receiver simply sees a closed channel.
    }
    rx
}

async fn logs(
    options: &Options,
    namespace: &str,
    name: &str,
    container: &str,
    previous: bool,
    tx: &async_channel::Sender<Result<String, String>>,
) {
    use futures::{AsyncBufReadExt, StreamExt};
    let result: Result<(), String> = async {
        let (client, _) = connect(options).await.map_err(|e| e.to_string())?;
        let api: Api<Pod> = Api::namespaced(client, namespace);
        let params = kube::api::LogParams {
            container: Some(container.to_owned()),
            follow: !previous,
            previous,
            tail_lines: Some(LOG_TAIL),
            ..Default::default()
        };
        let mut lines = api
            .log_stream(name, &params)
            .await
            .map_err(|e| e.to_string())?
            .lines();
        loop {
            // Wake periodically so a closed viewer stops a quiet stream.
            match tokio::time::timeout(Duration::from_secs(1), lines.next()).await {
                Err(_) if tx.is_closed() => return Ok(()),
                Err(_) => {}
                Ok(None) => return Ok(()),
                Ok(Some(Err(e))) => return Err(e.to_string()),
                Ok(Some(Ok(line))) => {
                    if tx.send(Ok(line)).await.is_err() {
                        return Ok(());
                    }
                }
            }
        }
    }
    .await;
    if let Err(e) = result {
        let _ = tx.send(Err(e)).await;
    }
}

/// Deletes one resource on a helper thread; the result is delivered once.
pub fn delete_resource(
    options: Options,
    kind: Kind,
    namespace: String,
    name: String,
) -> async_channel::Receiver<Result<(), String>> {
    let (tx, rx) = async_channel::bounded(1);
    let spawned = std::thread::Builder::new()
        .name("k9s-delete".into())
        .spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())
                .and_then(|rt| rt.block_on(delete(&options, kind, &namespace, &name)));
            let _ = tx.send_blocking(result);
        });
    if spawned.is_err() {
        // The receiver simply sees a closed channel.
    }
    rx
}

async fn delete(options: &Options, kind: Kind, namespace: &str, name: &str) -> Result<(), String> {
    let (client, _) = connect(options).await.map_err(|e| e.to_string())?;
    dynamic_api(client, kind, namespace)
        .delete(name, &kube::api::DeleteParams::default())
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Mutating actions beyond delete, run from `:scale N`, `:restart`, `:cordon`, `:uncordon`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Scale(i32),
    Restart,
    Cordon(bool),
}

impl Action {
    /// Whether the action applies to `kind`.
    pub fn supports(self, kind: Kind) -> bool {
        match self {
            Self::Scale(_) => matches!(
                kind,
                Kind::Deployment | Kind::StatefulSet | Kind::ReplicaSet
            ),
            Self::Restart => matches!(kind, Kind::Deployment | Kind::StatefulSet | Kind::DaemonSet),
            Self::Cordon(_) => kind == Kind::Node,
        }
    }
}

/// Runs an action on one resource; the confirmation message is delivered once.
pub fn run_action(
    options: Options,
    kind: Kind,
    namespace: String,
    name: String,
    action: Action,
) -> async_channel::Receiver<Result<String, String>> {
    let (tx, rx) = async_channel::bounded(1);
    let spawned = std::thread::Builder::new()
        .name("k9s-action".into())
        .spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())
                .and_then(|rt| rt.block_on(act(&options, kind, &namespace, &name, action)));
            let _ = tx.send_blocking(result);
        });
    if spawned.is_err() {
        // The receiver simply sees a closed channel.
    }
    rx
}

async fn act(
    options: &Options,
    kind: Kind,
    namespace: &str,
    name: &str,
    action: Action,
) -> Result<String, String> {
    use kube::api::{Patch, PatchParams};
    if !action.supports(kind) {
        return Err(format!("{action:?} is not supported for {}", kind.title()));
    }
    let (client, _) = connect(options).await.map_err(|e| e.to_string())?;
    let api = dynamic_api(client, kind, namespace);
    let params = PatchParams::default();
    match action {
        Action::Scale(n) => {
            let patch = serde_json::json!({ "spec": { "replicas": n } });
            api.patch_scale(name, &params, &Patch::Merge(&patch))
                .await
                .map_err(|e| e.to_string())?;
            Ok(format!("scaled {name} to {n}"))
        }
        Action::Restart => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let patch = serde_json::json!({ "spec": { "template": { "metadata": { "annotations": {
                "kubectl.kubernetes.io/restartedAt": now.to_string()
            } } } } });
            api.patch(name, &params, &Patch::Merge(&patch))
                .await
                .map_err(|e| e.to_string())?;
            Ok(format!("restarting {name}"))
        }
        Action::Cordon(on) => {
            let patch = serde_json::json!({ "spec": { "unschedulable": on } });
            api.patch(name, &params, &Patch::Merge(&patch))
                .await
                .map_err(|e| e.to_string())?;
            Ok(format!(
                "{} {name}",
                if on { "cordoned" } else { "uncordoned" }
            ))
        }
    }
}
