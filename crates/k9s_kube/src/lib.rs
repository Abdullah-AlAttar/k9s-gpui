//! Kubernetes access: a worker thread polls read-only and streams `Update`s to the UI.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use k8s_openapi::api::core::v1::Pod;
use k9s_core::{ClusterInfo, ClusterStats, TableData, Update};
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
    let (api, metrics_api): (Api<Pod>, Api<DynamicObject>) = if info.all_namespaces {
        (Api::all(client.clone()), Api::all_with(client.clone(), &ar))
    } else {
        (
            Api::namespaced(client.clone(), &info.namespace),
            Api::namespaced_with(client.clone(), &info.namespace, &ar),
        )
    };
    let mut backoff = REFRESH;
    loop {
        let update = match api.list(&ListParams::default()).await {
            Ok(list) => {
                backoff = REFRESH;
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
                // Without metrics-server this stays `None` and the metric columns are hidden.
                let metrics = fetch_metrics(&metrics_api).await;
                Update::Pods(k9s_render::pod::render(
                    &list.items,
                    metrics.as_ref(),
                    info.all_namespaces,
                    now,
                ))
            }
            Err(e) => {
                backoff = (backoff * 2).min(MAX_BACKOFF);
                Update::Error(e.to_string())
            }
        };
        if matches!(update, Update::Pods(_)) {
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
    namespace: String,
    name: String,
) -> async_channel::Receiver<Result<String, String>> {
    let (tx, rx) = async_channel::bounded(1);
    let spawned = std::thread::Builder::new()
        .name("k9s-text".into())
        .spawn(move || {
            let result = if kind == TextKind::Describe {
                describe(&options, &namespace, &name)
            } else {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| e.to_string())
                    .and_then(|rt| rt.block_on(pod_yaml(&options, &namespace, &name)))
            };
            let _ = tx.send_blocking(result);
        });
    if spawned.is_err() {
        // The receiver simply sees a closed channel.
    }
    rx
}

/// `kubectl describe` is the reference output; reimplementing it is not worth it yet.
fn describe(options: &Options, namespace: &str, name: &str) -> Result<String, String> {
    let mut cmd = std::process::Command::new("kubectl");
    if let Some(ctx) = &options.context {
        cmd.args(["--context", ctx]);
    }
    let out = cmd
        .args(["describe", "pod", name, "-n", namespace])
        .output()
        .map_err(|e| format!("kubectl: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
    }
}

async fn pod_yaml(options: &Options, namespace: &str, name: &str) -> Result<String, String> {
    let (client, _) = connect(options).await.map_err(|e| e.to_string())?;
    let api: Api<Pod> = Api::namespaced(client, namespace);
    let pod = api.get(name).await.map_err(|e| e.to_string())?;
    {
        let mut value = serde_json::to_value(&pod).map_err(|e| e.to_string())?;
        if let Some(meta) = value.get_mut("metadata").and_then(|m| m.as_object_mut()) {
            meta.remove("managedFields");
        }
        serde_yaml::to_string(&value).map_err(|e| e.to_string())
    }
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

/// Deletes one pod on a helper thread; the result is delivered once.
pub fn delete_pod(
    options: Options,
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
                .and_then(|rt| rt.block_on(delete(&options, &namespace, &name)));
            let _ = tx.send_blocking(result);
        });
    if spawned.is_err() {
        // The receiver simply sees a closed channel.
    }
    rx
}

async fn delete(options: &Options, namespace: &str, name: &str) -> Result<(), String> {
    let (client, _) = connect(options).await.map_err(|e| e.to_string())?;
    let api: Api<Pod> = Api::namespaced(client, namespace);
    api.delete(name, &kube::api::DeleteParams::default())
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}
