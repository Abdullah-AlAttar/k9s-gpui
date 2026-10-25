use k8s_openapi::api::core::v1::{ContainerState, ContainerStatus, Pod};
use k9s_core::{Column, Row, RowKind, SortKey, TableData};

use crate::age::human_duration;

use std::collections::BTreeMap;

use k8s_openapi::apimachinery::pkg::api::resource::Quantity;

use crate::metrics::{PodMetrics, Usage, parse_cpu_milli, parse_mem_bytes, percent};

const MIB: i64 = 1024 * 1024;

/// (name, max width, right aligned, wide only, needs metrics)
const COLUMNS: [(&str, u32, bool, bool, bool); 22] = [
    ("NAMESPACE", 20, false, false, false),
    ("NAME", 56, false, false, false),
    ("READY", 8, false, false, false),
    ("STATUS", 20, false, false, false),
    ("RESTARTS", 11, true, false, false),
    ("LAST RESTART", 14, true, true, false),
    ("CPU", 8, true, false, true),
    ("CPU/RL", 10, true, true, true),
    ("%CPU/R", 8, true, false, true),
    ("%CPU/L", 8, true, false, true),
    ("MEM", 8, true, false, true),
    ("MEM/RL", 12, true, true, true),
    ("%MEM/R", 8, true, false, true),
    ("%MEM/L", 8, true, false, true),
    ("IP", 16, false, false, false),
    ("NODE", 24, false, false, false),
    ("SERVICE-ACCOUNT", 24, false, true, false),
    ("NOMINATED NODE", 24, false, true, false),
    ("READINESS GATES", 16, false, true, false),
    ("QOS", 12, false, true, false),
    ("LABELS", 60, false, true, false),
    ("AGE", 7, true, false, false),
];
const NAMESPACE: usize = 0;
const AGE: usize = 21;

/// Renders pods like k9s' pod view. Metric columns are omitted when `metrics` is `None`
/// (no metrics-server); wide columns are always present and hidden by the UI.
pub fn render(
    pods: &[Pod],
    metrics: Option<&PodMetrics>,
    all_namespaces: bool,
    now_unix: i64,
) -> TableData {
    let keep: Vec<bool> = COLUMNS
        .iter()
        .enumerate()
        .map(|(i, c)| (all_namespaces || i != NAMESPACE) && (metrics.is_some() || !c.4))
        .collect();
    let columns = COLUMNS
        .iter()
        .zip(&keep)
        .filter(|(_, k)| **k)
        .map(|(c, _)| {
            let mut col = Column::new(c.0, c.1);
            col.right_align = c.2;
            col.wide = c.3;
            col.is_age = c.0 == "AGE";
            col
        })
        .collect();
    let rows = pods
        .iter()
        .map(|p| {
            let mut r = row(p, metrics, now_unix);
            let mut flags = keep.iter();
            r.cells.retain(|_| flags.next().copied().unwrap_or(false));
            r
        })
        .collect();
    let mut data = TableData { columns, rows };
    let column = data
        .columns
        .iter()
        .position(|c| c.name == if all_namespaces { "NAMESPACE" } else { "NAME" })
        .unwrap_or(0);
    data.sort(SortKey {
        column,
        ascending: true,
    });
    data
}

fn quantity_of(map: &Option<BTreeMap<String, Quantity>>, key: &str) -> Option<String> {
    map.as_ref()?.get(key).map(|q| q.0.clone())
}

/// (cpu request, cpu limit, mem request, mem limit) summed over containers and sidecars.
fn resources(pod: &Pod) -> (i64, i64, i64, i64) {
    let Some(spec) = pod.spec.as_ref() else {
        return (0, 0, 0, 0);
    };
    let sidecars = spec
        .init_containers
        .iter()
        .flatten()
        .filter(|c| is_sidecar(c));
    let mut sum = (0, 0, 0, 0);
    for c in spec.containers.iter().chain(sidecars) {
        let Some(r) = c.resources.as_ref() else {
            continue;
        };
        let cpu = |m: &Option<BTreeMap<String, Quantity>>| {
            quantity_of(m, "cpu")
                .and_then(|q| parse_cpu_milli(&q))
                .unwrap_or(0)
        };
        let mem = |m: &Option<BTreeMap<String, Quantity>>| {
            quantity_of(m, "memory")
                .and_then(|q| parse_mem_bytes(&q))
                .unwrap_or(0)
        };
        sum.0 += cpu(&r.requests);
        sum.1 += cpu(&r.limits);
        sum.2 += mem(&r.requests);
        sum.3 += mem(&r.limits);
    }
    sum
}

fn or_zero(v: i64) -> String {
    v.to_string()
}

fn last_restart(pod: &Pod, now: i64) -> String {
    container_statuses(pod)
        .iter()
        .filter_map(|c| {
            c.last_state
                .as_ref()?
                .terminated
                .as_ref()?
                .finished_at
                .as_ref()
        })
        .map(|t| t.0.as_second())
        .max()
        .map_or_else(String::new, |t| human_duration(now - t))
}

fn readiness_gates(pod: &Pod) -> String {
    let gates = pod
        .spec
        .iter()
        .flat_map(|s| s.readiness_gates.iter().flatten());
    let conditions: Vec<_> = pod
        .status
        .iter()
        .flat_map(|s| s.conditions.iter().flatten())
        .collect();
    let (mut ready, mut total) = (0, 0);
    for g in gates {
        total += 1;
        if conditions
            .iter()
            .any(|c| c.type_ == g.condition_type && c.status == "True")
        {
            ready += 1;
        }
    }
    if total == 0 {
        "0/0".into()
    } else {
        format!("{ready}/{total}")
    }
}

fn labels(pod: &Pod) -> String {
    pod.metadata
        .labels
        .iter()
        .flatten()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn row(pod: &Pod, metrics: Option<&PodMetrics>, now: i64) -> Row {
    let ns = pod.metadata.namespace.clone().unwrap_or_default();
    let name = pod.metadata.name.clone().unwrap_or_default();
    let status = phase(pod);
    let created = pod
        .metadata
        .creation_timestamp
        .as_ref()
        .map(|t| t.0.as_second());
    let age = created.map_or_else(String::new, |c| human_duration(now - c));
    let st = pod.status.as_ref();
    let spec = pod.spec.as_ref();
    let ready = ready(pod);
    let (cpu_r, cpu_l, mem_r, mem_l) = resources(pod);
    let id = format!("{ns}/{name}");
    let usage = metrics
        .and_then(|m| m.get(&id))
        .copied()
        .unwrap_or(Usage::default());
    let rl = |r: i64, l: i64, div: i64| format!("{}:{}", or_zero(r / div), or_zero(l / div));
    let fault = {
        let (ok, total) = ready.split_once('/').unwrap_or(("0", "0"));
        !matches!(status.as_str(), "Completed" | "Succeeded")
            && (status != "Running" || ok != total)
    };
    let cells = vec![
        ns,
        name,
        ready,
        status.clone(),
        restarts(pod).to_string(),
        last_restart(pod, now),
        or_zero(usage.cpu_milli),
        rl(cpu_r, cpu_l, 1),
        percent(usage.cpu_milli, cpu_r),
        percent(usage.cpu_milli, cpu_l),
        or_zero(usage.mem_bytes / MIB),
        rl(mem_r, mem_l, MIB),
        percent(usage.mem_bytes, mem_r),
        percent(usage.mem_bytes, mem_l),
        st.and_then(|s| s.pod_ip.clone()).unwrap_or_default(),
        spec.and_then(|s| s.node_name.clone()).unwrap_or_default(),
        spec.and_then(|s| s.service_account_name.clone())
            .unwrap_or_default(),
        st.and_then(|s| s.nominated_node_name.clone())
            .unwrap_or_else(|| "<none>".into()),
        readiness_gates(pod),
        st.and_then(|s| s.qos_class.clone()).unwrap_or_default(),
        labels(pod),
        age,
    ];
    debug_assert_eq!(cells.len(), COLUMNS.len());
    let _ = AGE;
    Row {
        id,
        kind: kind_of(&status),
        cells,
        age: created,
        fault,
        spans: Vec::new(),
    }
}

fn is_sidecar(c: &k8s_openapi::api::core::v1::Container) -> bool {
    c.restart_policy.as_deref() == Some("Always")
}

fn container_statuses(pod: &Pod) -> &[ContainerStatus] {
    pod.status
        .as_ref()
        .and_then(|s| s.container_statuses.as_deref())
        .unwrap_or_default()
}

fn sidecar_statuses(pod: &Pod) -> Vec<&ContainerStatus> {
    let names: Vec<&str> = pod
        .spec
        .iter()
        .flat_map(|s| s.init_containers.iter().flatten())
        .filter(|c| is_sidecar(c))
        .map(|c| c.name.as_str())
        .collect();
    pod.status
        .iter()
        .flat_map(|s| s.init_container_statuses.iter().flatten())
        .filter(|s| names.contains(&s.name.as_str()))
        .collect()
}

fn ready(pod: &Pod) -> String {
    let total = pod.spec.as_ref().map_or(0, |s| s.containers.len()) + sidecar_statuses(pod).len();
    let ready = container_statuses(pod)
        .iter()
        .chain(sidecar_statuses(pod))
        .filter(|c| c.ready)
        .count();
    format!("{ready}/{total}")
}

fn restarts(pod: &Pod) -> i32 {
    container_statuses(pod)
        .iter()
        .chain(sidecar_statuses(pod))
        .map(|c| c.restart_count)
        .sum()
}

/// Port of kubectl's pod status column logic.
fn phase(pod: &Pod) -> String {
    let Some(status) = pod.status.as_ref() else {
        return "Unknown".into();
    };
    let mut reason = status.phase.clone().unwrap_or_default();
    if let Some(r) = status.reason.as_deref().filter(|r| !r.is_empty()) {
        reason = r.into();
    }
    if reason.is_empty() && pod.metadata.deletion_timestamp.is_none() {
        reason = "Unknown".into();
    }
    if status.conditions.iter().flatten().any(|c| {
        c.type_ == "PodScheduled"
            && c.status == "False"
            && c.reason.as_deref() == Some("SchedulingGated")
    }) {
        reason = "SchedulingGated".into();
    }

    let init: &[ContainerStatus] = status
        .init_container_statuses
        .as_deref()
        .unwrap_or_default();
    let init_specs = pod
        .spec
        .iter()
        .flat_map(|s| s.init_containers.iter().flatten());
    let sidecars: Vec<&str> = init_specs
        .filter(|c| is_sidecar(c))
        .map(|c| c.name.as_str())
        .collect();
    let total_init = pod
        .spec
        .as_ref()
        .map_or(0, |s| s.init_containers.as_ref().map_or(0, Vec::len));
    let mut blocked = false;
    for (i, c) in init.iter().enumerate() {
        let state = c.state.as_ref();
        let terminated = state.and_then(|s| s.terminated.as_ref());
        let waiting = state.and_then(|s| s.waiting.as_ref());
        if sidecars.contains(&c.name.as_str())
            && state.is_some_and(|s| s.running.is_some())
            && c.started == Some(true)
        {
            continue;
        }
        if let Some(t) = terminated {
            if t.exit_code == 0 {
                continue;
            }
            reason = match (t.reason.as_deref().filter(|r| !r.is_empty()), t.signal) {
                (Some(r), _) => format!("Init:{r}"),
                (None, Some(sig)) if sig != 0 => format!("Init:Signal:{sig}"),
                _ => format!("Init:ExitCode:{}", t.exit_code),
            };
            blocked = true;
        } else if let Some(w) = waiting.filter(|w| {
            w.reason
                .as_deref()
                .is_some_and(|r| !r.is_empty() && r != "PodInitializing")
        }) {
            reason = format!("Init:{}", w.reason.as_deref().unwrap_or_default());
            blocked = true;
        } else {
            reason = format!("Init:{i}/{total_init}");
            blocked = true;
        }
        break;
    }

    if !blocked {
        let mut has_running = false;
        for c in container_statuses(pod).iter().rev() {
            let ContainerState {
                waiting,
                running,
                terminated,
            } = c.state.clone().unwrap_or_default();
            if let Some(r) = waiting
                .as_ref()
                .and_then(|w| w.reason.clone())
                .filter(|r| !r.is_empty())
            {
                reason = r;
            } else if let Some(t) = terminated {
                reason = match (t.reason.filter(|r| !r.is_empty()), t.signal) {
                    (Some(r), _) => r,
                    (None, Some(sig)) if sig != 0 => format!("Signal:{sig}"),
                    _ => format!("ExitCode:{}", t.exit_code),
                };
            } else if running.is_some() && c.ready {
                has_running = true;
            }
        }
        if reason == "Completed" && has_running {
            reason = "Running".into();
        }
    }

    if pod.metadata.deletion_timestamp.is_some() {
        reason = if status.reason.as_deref() == Some("NodeLost") {
            "Unknown".into()
        } else {
            "Terminating".into()
        };
    }
    reason
}

fn kind_of(status: &str) -> RowKind {
    match status {
        "Running" | "Succeeded" => RowKind::Standard,
        "Completed" => RowKind::Completed,
        "Terminating" => RowKind::Killing,
        "Pending" | "ContainerCreating" | "PodInitializing" | "SchedulingGated" => RowKind::Pending,
        s if s.starts_with("Init:")
            && !s.contains("Error")
            && !s.contains("BackOff")
            && !s.contains("ExitCode") =>
        {
            RowKind::Pending
        }
        _ => RowKind::Error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pod(json: serde_json::Value) -> Pod {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn running_pod_with_sidecar_and_restarts() {
        let p = pod(serde_json::json!({
            "metadata": {"name": "a", "namespace": "n", "creationTimestamp": "2026-01-01T00:00:00Z"},
            "spec": {"nodeName": "node1", "containers": [{"name": "c1"}, {"name": "c2"}]},
            "status": {"phase": "Running", "podIP": "10.0.0.1", "containerStatuses": [
                {"name": "c1", "ready": true, "restartCount": 5, "image": "i", "imageID": "x", "state": {"running": {}}},
                {"name": "c2", "ready": false, "restartCount": 0, "image": "i", "imageID": "x", "state": {"running": {}}}
            ]}
        }));
        let t = render(&[p], None, false, 1_767_225_600 + 3 * 86400);
        let r = t.rows.first().unwrap();
        let cell = |name: &str| {
            let i = t.columns.iter().position(|c| c.name == name).unwrap();
            r.cells[i].as_str()
        };
        assert_eq!(
            [
                cell("NAME"),
                cell("READY"),
                cell("STATUS"),
                cell("RESTARTS")
            ],
            ["a", "1/2", "Running", "5"]
        );
        assert_eq!(
            [cell("IP"), cell("NODE"), cell("AGE")],
            ["10.0.0.1", "node1", "3d"]
        );
        assert!(t.columns.iter().all(|c| c.name != "CPU"));
        assert!(r.fault, "1/2 ready is a fault");
        assert_eq!(r.kind, RowKind::Standard);
    }

    #[test]
    fn waiting_reason_and_completed_and_init() {
        let crash = pod(serde_json::json!({
            "metadata": {"name": "c"}, "spec": {"containers": [{"name": "c"}]},
            "status": {"phase": "Running", "containerStatuses": [
                {"name": "c", "ready": false, "restartCount": 1, "image": "i", "imageID": "x",
                 "state": {"waiting": {"reason": "CrashLoopBackOff"}}}]}
        }));
        assert_eq!(phase(&crash), "CrashLoopBackOff");
        assert_eq!(kind_of("CrashLoopBackOff"), RowKind::Error);

        let done = pod(serde_json::json!({
            "metadata": {"name": "d"}, "spec": {"containers": [{"name": "c"}]},
            "status": {"phase": "Succeeded", "containerStatuses": [
                {"name": "c", "ready": false, "restartCount": 0, "image": "i", "imageID": "x",
                 "state": {"terminated": {"exitCode": 0, "reason": "Completed"}}}]}
        }));
        assert_eq!(phase(&done), "Completed");

        let init = pod(serde_json::json!({
            "metadata": {"name": "i"},
            "spec": {"initContainers": [{"name": "a"}, {"name": "b"}], "containers": [{"name": "c"}]},
            "status": {"phase": "Pending", "initContainerStatuses": [
                {"name": "a", "ready": true, "restartCount": 0, "image": "i", "imageID": "x",
                 "state": {"terminated": {"exitCode": 0}}},
                {"name": "b", "ready": false, "restartCount": 0, "image": "i", "imageID": "x",
                 "state": {"running": {}}}]}
        }));
        assert_eq!(phase(&init), "Init:1/2");
        assert_eq!(kind_of("Init:1/2"), RowKind::Pending);
    }

    #[test]
    fn terminating_wins() {
        let p = pod(serde_json::json!({
            "metadata": {"name": "t", "deletionTimestamp": "2026-01-01T00:00:00Z"},
            "spec": {"containers": [{"name": "c"}]}, "status": {"phase": "Running"}
        }));
        assert_eq!(phase(&p), "Terminating");
    }
}
