use k8s_openapi::api::core::v1::{Container, ContainerState, ContainerStatus, Pod};
use k9s_core::{Column, Row, RowKind, TableData};

fn columns() -> Vec<Column> {
    vec![
        Column::new("NAME", 40),
        Column::new("IMAGE", 70),
        Column::new("READY", 6),
        Column::new("STATE", 24),
        Column::new("INIT", 6),
        Column::new("RESTARTS", 10).right(),
        Column::new("PROBES(L:R:S)", 16),
        Column::new("PORTS", 40),
    ]
}

/// Container view of one pod: init containers first, then regular ones (k9s order).
pub fn render(pod: &Pod) -> TableData {
    let spec = pod.spec.as_ref();
    let status = pod.status.as_ref();
    let find = |list: Option<&Vec<ContainerStatus>>, name: &str| {
        list.and_then(|l| l.iter().find(|s| s.name == name))
            .cloned()
    };
    let init = spec
        .into_iter()
        .flat_map(|s| s.init_containers.iter().flatten().map(|c| (c, true)));
    let regular = spec
        .into_iter()
        .flat_map(|s| s.containers.iter().map(|c| (c, false)));
    let rows = init
        .chain(regular)
        .map(|(c, is_init)| {
            let st = if is_init {
                find(
                    status.and_then(|s| s.init_container_statuses.as_ref()),
                    &c.name,
                )
            } else {
                find(status.and_then(|s| s.container_statuses.as_ref()), &c.name)
            };
            row(c, st.as_ref(), is_init)
        })
        .collect();
    TableData {
        columns: columns(),
        rows,
    }
}

fn row(c: &Container, st: Option<&ContainerStatus>, is_init: bool) -> Row {
    let (state, kind) = st
        .and_then(|s| s.state.as_ref())
        .map_or(("Unknown".to_owned(), RowKind::Pending), state_of);
    let probe = |on: bool| if on { "on" } else { "off" };
    let ports = c
        .ports
        .iter()
        .flatten()
        .map(|p| match &p.name {
            Some(n) => format!("{n}:{}", p.container_port),
            None => p.container_port.to_string(),
        })
        .collect::<Vec<_>>()
        .join(",");
    Row {
        id: c.name.clone(),
        cells: vec![
            c.name.clone(),
            c.image.clone().unwrap_or_default(),
            st.map_or("false", |s| if s.ready { "true" } else { "false" })
                .to_owned(),
            state,
            is_init.to_string(),
            st.map_or(0, |s| s.restart_count).to_string(),
            format!(
                "{}:{}:{}",
                probe(c.liveness_probe.is_some()),
                probe(c.readiness_probe.is_some()),
                probe(c.startup_probe.is_some())
            ),
            ports,
        ],
        kind,
        age: None,
        fault: kind == RowKind::Error,
        spans: Vec::new(),
    }
}

fn state_of(state: &ContainerState) -> (String, RowKind) {
    if state.running.is_some() {
        ("Running".to_owned(), RowKind::Standard)
    } else if let Some(w) = &state.waiting {
        (
            w.reason.clone().unwrap_or_else(|| "Waiting".to_owned()),
            RowKind::Pending,
        )
    } else if let Some(t) = &state.terminated {
        let reason = t.reason.clone().unwrap_or_else(|| "Terminated".to_owned());
        let kind = if t.exit_code == 0 {
            RowKind::Completed
        } else {
            RowKind::Error
        };
        (reason, kind)
    } else {
        ("Unknown".to_owned(), RowKind::Pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_init_before_regular_containers() {
        let pod: Pod = serde_json::from_value(serde_json::json!({
            "spec": {
                "initContainers": [{"name": "migrate", "image": "m:1"}],
                "containers": [{"name": "app", "image": "a:1",
                    "ports": [{"name": "http", "containerPort": 8080}],
                    "livenessProbe": {}}]
            },
            "status": {
                "initContainerStatuses": [{"name": "migrate", "image": "m:1", "imageID": "",
                    "ready": true, "restartCount": 0,
                    "state": {"terminated": {"exitCode": 0, "reason": "Completed"}}}],
                "containerStatuses": [{"name": "app", "image": "a:1", "imageID": "",
                    "ready": true, "restartCount": 3, "state": {"running": {}}}]
            }
        }))
        .unwrap();
        let t = render(&pod);
        let names: Vec<_> = t.rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(names, ["migrate", "app"]);
        assert_eq!(t.rows[0].cells[3], "Completed");
        assert_eq!(t.rows[0].cells[4], "true");
        assert_eq!(t.rows[1].cells[5], "3");
        assert_eq!(t.rows[1].cells[6], "on:off:off");
        assert_eq!(t.rows[1].cells[7], "http:8080");
    }
}
