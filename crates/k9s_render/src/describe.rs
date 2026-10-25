//! Native `kubectl describe pod` style output.

use std::collections::BTreeMap;
use std::fmt::Write;

use k8s_openapi::api::core::v1::{Container, ContainerState, ContainerStatus, Event, Pod};

use crate::age::human_duration;

const DASH: &str = "<none>";

fn kv_block(out: &mut String, title: &str, map: Option<&BTreeMap<String, String>>) {
    match map.filter(|m| !m.is_empty()) {
        None => {
            let _ = writeln!(out, "{title}:{}{DASH}", pad(title));
        }
        Some(m) => {
            for (i, (k, v)) in m.iter().enumerate() {
                let head = if i == 0 {
                    format!("{title}:{}", pad(title))
                } else {
                    " ".repeat(title.len() + 1 + pad(title).len())
                };
                let _ = writeln!(out, "{head}{k}={v}");
            }
        }
    }
}

fn pad(title: &str) -> String {
    " ".repeat(14usize.saturating_sub(title.len() + 1).max(1))
}

fn field(out: &mut String, title: &str, value: &str) {
    let _ = writeln!(out, "{title}:{}{value}", pad(title));
}

fn state_lines(out: &mut String, label: &str, state: Option<&ContainerState>) {
    let Some(state) = state else { return };
    if let Some(r) = &state.running {
        let _ = writeln!(out, "    {label}:          Running");
        if let Some(s) = &r.started_at {
            let _ = writeln!(out, "      Started:      {}", s.0);
        }
    } else if let Some(w) = &state.waiting {
        let _ = writeln!(out, "    {label}:          Waiting");
        if let Some(r) = &w.reason {
            let _ = writeln!(out, "      Reason:       {r}");
        }
        if let Some(m) = w.message.as_ref().filter(|m| !m.is_empty()) {
            let _ = writeln!(out, "      Message:      {m}");
        }
    } else if let Some(t) = &state.terminated {
        let _ = writeln!(out, "    {label}:          Terminated");
        if let Some(r) = &t.reason {
            let _ = writeln!(out, "      Reason:       {r}");
        }
        if let Some(m) = t.message.as_ref().filter(|m| !m.is_empty()) {
            let _ = writeln!(out, "      Message:      {m}");
        }
        let _ = writeln!(out, "      Exit Code:    {}", t.exit_code);
        if let Some(s) = &t.started_at {
            let _ = writeln!(out, "      Started:      {}", s.0);
        }
        if let Some(f) = &t.finished_at {
            let _ = writeln!(out, "      Finished:     {}", f.0);
        }
    }
}

fn container(out: &mut String, c: &Container, status: Option<&ContainerStatus>) {
    let _ = writeln!(out, "  {}:", c.name);
    if let Some(id) = status.and_then(|s| s.container_id.as_ref()) {
        let _ = writeln!(out, "    Container ID:   {id}");
    }
    let _ = writeln!(
        out,
        "    Image:          {}",
        c.image.as_deref().unwrap_or("")
    );
    if let Some(id) = status
        .map(|s| s.image_id.as_str())
        .filter(|i| !i.is_empty())
    {
        let _ = writeln!(out, "    Image ID:       {id}");
    }
    let ports: Vec<String> = c
        .ports
        .iter()
        .flatten()
        .map(|p| {
            format!(
                "{}/{}",
                p.container_port,
                p.protocol.as_deref().unwrap_or("TCP")
            )
        })
        .collect();
    let ports = if ports.is_empty() {
        DASH.to_owned()
    } else {
        ports.join(", ")
    };
    let _ = writeln!(out, "    Ports:          {ports}");
    for (title, list) in [("Command", &c.command), ("Args", &c.args)] {
        if let Some(list) = list.as_ref().filter(|l| !l.is_empty()) {
            let _ = writeln!(out, "    {title}:");
            for item in list {
                let _ = writeln!(out, "      {item}");
            }
        }
    }
    state_lines(out, "State", status.and_then(|s| s.state.as_ref()));
    if let Some(last) = status.and_then(|s| s.last_state.as_ref()) {
        state_lines(out, "Last State", Some(last));
    }
    if let Some(s) = status {
        let _ = writeln!(out, "    Ready:          {}", s.ready);
        let _ = writeln!(out, "    Restart Count:  {}", s.restart_count);
    }
    if let Some(r) = &c.resources {
        for (title, map) in [("Limits", &r.limits), ("Requests", &r.requests)] {
            if let Some(m) = map.as_ref().filter(|m| !m.is_empty()) {
                let _ = writeln!(out, "    {title}:");
                for (k, q) in m {
                    let _ = writeln!(out, "      {k}:  {}", q.0);
                }
            }
        }
    }
    for (title, probe) in [
        ("Liveness", &c.liveness_probe),
        ("Readiness", &c.readiness_probe),
        ("Startup", &c.startup_probe),
    ] {
        if let Some(p) = probe {
            let kind = if let Some(h) = &p.http_get {
                format!(
                    "http-get {}:{:?}{}",
                    h.host.as_deref().unwrap_or(""),
                    h.port,
                    h.path.as_deref().unwrap_or("/")
                )
            } else if let Some(t) = &p.tcp_socket {
                format!("tcp-socket :{:?}", t.port)
            } else if p.exec.is_some() {
                "exec".to_owned()
            } else {
                "grpc".to_owned()
            };
            let _ = writeln!(
                out,
                "    {title}:  {kind} delay={}s timeout={}s period={}s #success={} #failure={}",
                p.initial_delay_seconds.unwrap_or(0),
                p.timeout_seconds.unwrap_or(1),
                p.period_seconds.unwrap_or(10),
                p.success_threshold.unwrap_or(1),
                p.failure_threshold.unwrap_or(3)
            );
        }
    }
    let env = c.env.as_deref().unwrap_or_default();
    if env.is_empty() {
        let _ = writeln!(out, "    Environment:    {DASH}");
    } else {
        let _ = writeln!(out, "    Environment:");
        for e in env {
            let value = match (&e.value, &e.value_from) {
                (Some(v), _) => v.clone(),
                (None, Some(_)) => "<set from reference>".to_owned(),
                (None, None) => String::new(),
            };
            let _ = writeln!(out, "      {}:  {value}", e.name);
        }
    }
    let mounts = c.volume_mounts.as_deref().unwrap_or_default();
    if mounts.is_empty() {
        let _ = writeln!(out, "    Mounts:         {DASH}");
    } else {
        let _ = writeln!(out, "    Mounts:");
        for m in mounts {
            let ro = if m.read_only == Some(true) {
                "ro"
            } else {
                "rw"
            };
            let _ = writeln!(out, "      {} from {} ({ro})", m.mount_path, m.name);
        }
    }
}

fn volume_kind(v: &k8s_openapi::api::core::v1::Volume) -> &'static str {
    if v.config_map.is_some() {
        "ConfigMap"
    } else if v.secret.is_some() {
        "Secret"
    } else if v.empty_dir.is_some() {
        "EmptyDir"
    } else if v.persistent_volume_claim.is_some() {
        "PersistentVolumeClaim"
    } else if v.projected.is_some() {
        "Projected"
    } else if v.host_path.is_some() {
        "HostPath"
    } else if v.downward_api.is_some() {
        "DownwardAPI"
    } else {
        "Other"
    }
}

/// Renders a pod and its events as text, `now` being unix seconds.
#[must_use]
pub fn pod(pod: &Pod, events: &[Event], now: i64) -> String {
    let mut out = String::new();
    let meta = &pod.metadata;
    let spec = pod.spec.as_ref();
    let status = pod.status.as_ref();
    field(&mut out, "Name", meta.name.as_deref().unwrap_or(""));
    field(
        &mut out,
        "Namespace",
        meta.namespace.as_deref().unwrap_or(""),
    );
    if let Some(p) = spec.and_then(|s| s.priority) {
        field(&mut out, "Priority", &p.to_string());
    }
    if let Some(sa) = spec.and_then(|s| s.service_account_name.as_deref()) {
        field(&mut out, "Service Account", sa);
    }
    if let Some(node) = spec.and_then(|s| s.node_name.as_deref()) {
        let ip = status.and_then(|s| s.host_ip.as_deref()).unwrap_or("");
        field(&mut out, "Node", &format!("{node}/{ip}"));
    }
    if let Some(t) = status.and_then(|s| s.start_time.as_ref()) {
        field(&mut out, "Start Time", &t.0.to_string());
    }
    kv_block(&mut out, "Labels", meta.labels.as_ref());
    kv_block(&mut out, "Annotations", meta.annotations.as_ref());
    field(
        &mut out,
        "Status",
        status.and_then(|s| s.phase.as_deref()).unwrap_or("Unknown"),
    );
    field(
        &mut out,
        "IP",
        status.and_then(|s| s.pod_ip.as_deref()).unwrap_or(""),
    );
    let ips: Vec<&str> = status
        .and_then(|s| s.pod_ips.as_ref())
        .map(|v| v.iter().map(|i| i.ip.as_str()).collect())
        .unwrap_or_default();
    if ips.is_empty() {
        field(&mut out, "IPs", DASH);
    } else {
        let _ = writeln!(out, "IPs:");
        for ip in ips {
            let _ = writeln!(out, "  IP:  {ip}");
        }
    }
    if let Some(o) = meta
        .owner_references
        .as_ref()
        .and_then(|o| o.iter().find(|o| o.controller == Some(true)))
    {
        field(&mut out, "Controlled By", &format!("{}/{}", o.kind, o.name));
    }

    let statuses = |list: Option<&Vec<ContainerStatus>>, name: &str| {
        list.and_then(|l| l.iter().find(|s| s.name == name))
            .cloned()
    };
    let init = spec
        .and_then(|s| s.init_containers.as_ref())
        .filter(|c| !c.is_empty());
    if let Some(init) = init {
        let _ = writeln!(out, "Init Containers:");
        for c in init {
            let st = statuses(
                status.and_then(|s| s.init_container_statuses.as_ref()),
                &c.name,
            );
            container(&mut out, c, st.as_ref());
        }
    }
    let _ = writeln!(out, "Containers:");
    for c in spec.map(|s| s.containers.as_slice()).unwrap_or_default() {
        let st = statuses(status.and_then(|s| s.container_statuses.as_ref()), &c.name);
        container(&mut out, c, st.as_ref());
    }

    let _ = writeln!(out, "Conditions:");
    let _ = writeln!(out, "  Type                        Status");
    for c in status
        .and_then(|s| s.conditions.as_ref())
        .into_iter()
        .flatten()
    {
        let _ = writeln!(out, "  {:<27} {}", c.type_, c.status);
    }

    let vols = spec
        .and_then(|s| s.volumes.as_ref())
        .filter(|v| !v.is_empty());
    if let Some(vols) = vols {
        let _ = writeln!(out, "Volumes:");
        for v in vols {
            let _ = writeln!(out, "  {}:", v.name);
            let _ = writeln!(out, "    Type:  {}", volume_kind(v));
            if let Some(cm) = &v.config_map {
                let _ = writeln!(out, "    Name:  {}", cm.name);
            }
            if let Some(s) = &v.secret {
                let _ = writeln!(
                    out,
                    "    SecretName:  {}",
                    s.secret_name.as_deref().unwrap_or("")
                );
            }
            if let Some(p) = &v.persistent_volume_claim {
                let _ = writeln!(out, "    ClaimName:  {}", p.claim_name);
            }
        }
    } else {
        field(&mut out, "Volumes", DASH);
    }

    field(
        &mut out,
        "QoS Class",
        status.and_then(|s| s.qos_class.as_deref()).unwrap_or(""),
    );
    kv_block(
        &mut out,
        "Node-Selectors",
        spec.and_then(|s| s.node_selector.as_ref()),
    );
    let tols = spec
        .and_then(|s| s.tolerations.as_ref())
        .filter(|t| !t.is_empty());
    match tols {
        None => field(&mut out, "Tolerations", DASH),
        Some(tols) => {
            for (i, t) in tols.iter().enumerate() {
                let key = t.key.as_deref().unwrap_or("");
                let op = t.operator.as_deref().unwrap_or("Equal");
                let mut line = match (key.is_empty(), op) {
                    (true, _) => format!("op={op}"),
                    (false, "Exists") => key.to_owned(),
                    (false, _) => format!("{key}={}", t.value.as_deref().unwrap_or("")),
                };
                if let Some(e) = t.effect.as_deref().filter(|e| !e.is_empty()) {
                    let _ = write!(line, ":{e}");
                }
                if let Some(s) = t.toleration_seconds {
                    let _ = write!(line, " for {s}s");
                }
                let head = if i == 0 {
                    "Tolerations:  "
                } else {
                    "              "
                };
                let _ = writeln!(out, "{head}{line}");
            }
        }
    }

    let _ = writeln!(out, "Events:");
    if events.is_empty() {
        let _ = writeln!(out, "  <none>");
    } else {
        let _ = writeln!(out, "  Type    Reason    Age    From    Message");
        let _ = writeln!(out, "  ----    ------    ----   ----    -------");
        let mut sorted: Vec<&Event> = events.iter().collect();
        sorted.sort_by_key(|e| event_time(e));
        for e in sorted {
            let age = event_time(e).map_or_else(String::new, |t| human_duration(now - t));
            let count = e
                .count
                .filter(|c| *c > 1)
                .map_or_else(String::new, |c| format!(" (x{c})"));
            let from = e
                .source
                .as_ref()
                .and_then(|s| s.component.as_deref())
                .or(e.reporting_component.as_deref())
                .unwrap_or("");
            let _ = writeln!(
                out,
                "  {:<7} {:<9} {}{count}  {from}  {}",
                e.type_.as_deref().unwrap_or(""),
                e.reason.as_deref().unwrap_or(""),
                age,
                e.message.as_deref().unwrap_or("").trim_end()
            );
        }
    }
    out
}

fn event_time(e: &Event) -> Option<i64> {
    e.last_timestamp
        .as_ref()
        .map(|t| t.0.as_second())
        .or_else(|| e.event_time.as_ref().map(|t| t.0.as_second()))
        .or_else(|| {
            e.metadata
                .creation_timestamp
                .as_ref()
                .map(|t| t.0.as_second())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_basic_pod() {
        let p: Pod = serde_json::from_value(serde_json::json!({
            "metadata": {"name": "web", "namespace": "ns", "labels": {"a": "b"}},
            "spec": {"containers": [{"name": "app", "image": "nginx"}]},
            "status": {"phase": "Running"}
        }))
        .unwrap();
        let text = pod(&p, &[], 0);
        assert!(text.contains("Name:         web"));
        assert!(text.contains("Labels:       a=b"));
        assert!(text.contains("  app:"));
        assert!(text.contains("Events:\n  <none>"));
    }
}
