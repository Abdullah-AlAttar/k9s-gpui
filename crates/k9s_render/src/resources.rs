//! Table renderers for the non-pod resource kinds.

use k8s_openapi::{
    Metadata,
    api::{
        apps::v1::{DaemonSet, Deployment, ReplicaSet, StatefulSet},
        batch::v1::{CronJob, Job},
        core::v1::{ConfigMap, Event, Node, PersistentVolumeClaim, PodSpec, Secret, Service},
        networking::v1::Ingress,
    },
    apimachinery::pkg::apis::meta::v1::ObjectMeta,
};
use k9s_core::{Column, Row, RowKind, SortKey, TableData};

use crate::age::human_duration;

/// (name, max width, right aligned, wide only)
type Col = (&'static str, u32, bool, bool);

struct Spec {
    cols: &'static [Col],
    namespaced: bool,
    /// Whether the NAME column is shown outside the wide view (events hide it).
    show_name: bool,
}

fn build<K: Metadata<Ty = ObjectMeta>>(
    items: &[K],
    spec: &Spec,
    all_namespaces: bool,
    now: i64,
    cells: impl Fn(&K, i64) -> (Vec<String>, RowKind, Option<i64>),
) -> TableData {
    let with_ns = spec.namespaced && all_namespaces;
    let mut columns = Vec::new();
    if with_ns {
        columns.push(Column::new("NAMESPACE", 20));
    }
    let mut name = Column::new("NAME", 56);
    name.wide = !spec.show_name;
    columns.push(name);
    for &(n, w, right, wide) in spec.cols {
        let mut c = Column::new(n, w);
        c.right_align = right;
        c.wide = wide;
        columns.push(c);
    }
    columns.push(Column::new("AGE", 7).right().age());
    let rows = items
        .iter()
        .map(|item| {
            let meta = item.metadata();
            let ns = meta.namespace.clone().unwrap_or_default();
            let name = meta.name.clone().unwrap_or_default();
            let created = meta.creation_timestamp.as_ref().map(|t| t.0.as_second());
            let (mid, kind, age_at) = cells(item, now);
            let stamp = age_at.or(created);
            let mut row_cells = Vec::new();
            if with_ns {
                row_cells.push(ns.clone());
            }
            row_cells.push(name.clone());
            row_cells.extend(mid);
            row_cells.push(stamp.map_or_else(String::new, |c| human_duration(now - c)));
            Row {
                id: format!("{ns}/{name}"),
                cells: row_cells,
                kind,
                age: stamp,
                fault: matches!(kind, RowKind::Error | RowKind::Pending),
                ..Row::default()
            }
        })
        .collect();
    let mut data = TableData { columns, rows };
    let column = if spec.show_name {
        usize::from(with_ns)
    } else {
        data.columns.len() - 1
    };
    data.sort(SortKey {
        column,
        ascending: true,
    });
    data
}

fn n(v: Option<i32>) -> i32 {
    v.unwrap_or(0)
}

fn images(spec: Option<&PodSpec>) -> (String, String) {
    let containers = spec.map(|s| s.containers.as_slice()).unwrap_or_default();
    let names = containers
        .iter()
        .map(|c| c.name.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let imgs = containers
        .iter()
        .filter_map(|c| c.image.as_deref())
        .collect::<Vec<_>>()
        .join(",");
    (names, imgs)
}

fn ready_kind(ready: i32, desired: i32) -> RowKind {
    if ready == desired {
        RowKind::Standard
    } else {
        RowKind::Pending
    }
}

fn labels(meta: &ObjectMeta) -> String {
    meta.labels
        .iter()
        .flatten()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(",")
}

pub fn deployments(items: &[Deployment], all_ns: bool, now: i64) -> TableData {
    const SPEC: Spec = Spec {
        cols: &[
            ("READY", 8, false, false),
            ("UP-TO-DATE", 10, true, false),
            ("AVAILABLE", 10, true, false),
            ("CONTAINERS", 30, false, true),
            ("IMAGES", 50, false, true),
        ],
        namespaced: true,
        show_name: true,
    };
    build(items, &SPEC, all_ns, now, |d, _| {
        let st = d.status.as_ref();
        let ready = n(st.and_then(|s| s.ready_replicas));
        let desired = n(d.spec.as_ref().and_then(|s| s.replicas));
        let (names, imgs) = images(d.spec.as_ref().and_then(|s| s.template.spec.as_ref()));
        (
            vec![
                format!("{ready}/{desired}"),
                n(st.and_then(|s| s.updated_replicas)).to_string(),
                n(st.and_then(|s| s.available_replicas)).to_string(),
                names,
                imgs,
            ],
            ready_kind(ready, desired),
            None,
        )
    })
}

pub fn stateful_sets(items: &[StatefulSet], all_ns: bool, now: i64) -> TableData {
    const SPEC: Spec = Spec {
        cols: &[
            ("READY", 8, false, false),
            ("CONTAINERS", 30, false, true),
            ("IMAGES", 50, false, true),
        ],
        namespaced: true,
        show_name: true,
    };
    build(items, &SPEC, all_ns, now, |s, _| {
        let ready = n(s.status.as_ref().and_then(|s| s.ready_replicas));
        let desired = n(s.spec.as_ref().and_then(|s| s.replicas));
        let (names, imgs) = images(s.spec.as_ref().and_then(|s| s.template.spec.as_ref()));
        (
            vec![format!("{ready}/{desired}"), names, imgs],
            ready_kind(ready, desired),
            None,
        )
    })
}

pub fn daemon_sets(items: &[DaemonSet], all_ns: bool, now: i64) -> TableData {
    const SPEC: Spec = Spec {
        cols: &[
            ("DESIRED", 8, true, false),
            ("CURRENT", 8, true, false),
            ("READY", 8, true, false),
            ("UP-TO-DATE", 10, true, false),
            ("AVAILABLE", 10, true, false),
            ("CONTAINERS", 30, false, true),
            ("IMAGES", 50, false, true),
        ],
        namespaced: true,
        show_name: true,
    };
    build(items, &SPEC, all_ns, now, |d, _| {
        let st = d.status.as_ref();
        let desired = st.map_or(0, |s| s.desired_number_scheduled);
        let ready = st.map_or(0, |s| s.number_ready);
        let (names, imgs) = images(d.spec.as_ref().and_then(|s| s.template.spec.as_ref()));
        (
            vec![
                desired.to_string(),
                st.map_or(0, |s| s.current_number_scheduled).to_string(),
                ready.to_string(),
                n(st.and_then(|s| s.updated_number_scheduled)).to_string(),
                n(st.and_then(|s| s.number_available)).to_string(),
                names,
                imgs,
            ],
            ready_kind(ready, desired),
            None,
        )
    })
}

pub fn replica_sets(items: &[ReplicaSet], all_ns: bool, now: i64) -> TableData {
    const SPEC: Spec = Spec {
        cols: &[
            ("DESIRED", 8, true, false),
            ("CURRENT", 8, true, false),
            ("READY", 8, true, false),
        ],
        namespaced: true,
        show_name: true,
    };
    build(items, &SPEC, all_ns, now, |r, _| {
        let st = r.status.as_ref();
        let desired = n(r.spec.as_ref().and_then(|s| s.replicas));
        let ready = n(st.and_then(|s| s.ready_replicas));
        (
            vec![
                desired.to_string(),
                st.map_or(0, |s| s.replicas).to_string(),
                ready.to_string(),
            ],
            ready_kind(ready, desired),
            None,
        )
    })
}

pub fn jobs(items: &[Job], all_ns: bool, now: i64) -> TableData {
    const SPEC: Spec = Spec {
        cols: &[
            ("COMPLETIONS", 12, false, false),
            ("DURATION", 10, true, false),
            ("CONTAINERS", 30, false, true),
            ("IMAGES", 50, false, true),
        ],
        namespaced: true,
        show_name: true,
    };
    build(items, &SPEC, all_ns, now, |j, now| {
        let st = j.status.as_ref();
        let done = n(st.and_then(|s| s.succeeded));
        let want = n(j.spec.as_ref().and_then(|s| s.completions)).max(1);
        let start = st
            .and_then(|s| s.start_time.as_ref())
            .map(|t| t.0.as_second());
        let end = st
            .and_then(|s| s.completion_time.as_ref())
            .map_or(now, |t| t.0.as_second());
        let (names, imgs) = images(j.spec.as_ref().and_then(|s| s.template.spec.as_ref()));
        let failed = n(st.and_then(|s| s.failed)) > 0 && done < want;
        (
            vec![
                format!("{done}/{want}"),
                start.map_or_else(String::new, |s| human_duration(end - s)),
                names,
                imgs,
            ],
            if failed {
                RowKind::Error
            } else if done >= want {
                RowKind::Completed
            } else {
                RowKind::Pending
            },
            None,
        )
    })
}

pub fn cron_jobs(items: &[CronJob], all_ns: bool, now: i64) -> TableData {
    const SPEC: Spec = Spec {
        cols: &[
            ("SCHEDULE", 20, false, false),
            ("SUSPEND", 8, false, false),
            ("ACTIVE", 8, true, false),
            ("LAST SCHEDULE", 14, true, false),
        ],
        namespaced: true,
        show_name: true,
    };
    build(items, &SPEC, all_ns, now, |c, now| {
        let spec = &c.spec;
        let st = c.status.as_ref();
        let last = st
            .and_then(|s| s.last_schedule_time.as_ref())
            .map(|t| human_duration(now - t.0.as_second()))
            .unwrap_or_default();
        (
            vec![
                spec.schedule.clone(),
                spec.suspend.unwrap_or(false).to_string(),
                st.and_then(|s| s.active.as_ref())
                    .map_or(0, Vec::len)
                    .to_string(),
                last,
            ],
            RowKind::Standard,
            None,
        )
    })
}

pub fn services(items: &[Service], all_ns: bool, now: i64) -> TableData {
    const SPEC: Spec = Spec {
        cols: &[
            ("TYPE", 14, false, false),
            ("CLUSTER-IP", 16, false, false),
            ("EXTERNAL-IP", 20, false, false),
            ("PORT(S)", 40, false, false),
            ("SELECTOR", 40, false, true),
            ("LABELS", 60, false, true),
        ],
        namespaced: true,
        show_name: true,
    };
    build(items, &SPEC, all_ns, now, |s, _| {
        let spec = s.spec.as_ref();
        let mut external: Vec<String> = spec
            .and_then(|s| s.external_ips.clone())
            .unwrap_or_default();
        external.extend(
            s.status
                .iter()
                .flat_map(|st| st.load_balancer.iter())
                .flat_map(|lb| lb.ingress.iter().flatten())
                .filter_map(|i| i.ip.clone().or_else(|| i.hostname.clone())),
        );
        let ports = spec
            .and_then(|s| s.ports.as_ref())
            .into_iter()
            .flatten()
            .map(|p| {
                let proto = p.protocol.as_deref().unwrap_or("TCP");
                match p.node_port {
                    Some(np) if np != 0 => format!("{}:{np}/{proto}", p.port),
                    _ => format!("{}/{proto}", p.port),
                }
            })
            .collect::<Vec<_>>()
            .join(",");
        let selector = spec
            .and_then(|s| s.selector.as_ref())
            .into_iter()
            .flatten()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(",");
        (
            vec![
                spec.and_then(|s| s.type_.clone()).unwrap_or_default(),
                spec.and_then(|s| s.cluster_ip.clone()).unwrap_or_default(),
                if external.is_empty() {
                    "<none>".into()
                } else {
                    external.join(",")
                },
                ports,
                selector,
                labels(s.metadata()),
            ],
            RowKind::Standard,
            None,
        )
    })
}

pub fn ingresses(items: &[Ingress], all_ns: bool, now: i64) -> TableData {
    const SPEC: Spec = Spec {
        cols: &[
            ("CLASS", 16, false, false),
            ("HOSTS", 40, false, false),
            ("ADDRESS", 24, false, false),
            ("PORTS", 10, false, false),
        ],
        namespaced: true,
        show_name: true,
    };
    build(items, &SPEC, all_ns, now, |i, _| {
        let spec = i.spec.as_ref();
        let hosts = spec
            .and_then(|s| s.rules.as_ref())
            .into_iter()
            .flatten()
            .filter_map(|r| r.host.clone())
            .collect::<Vec<_>>()
            .join(",");
        let address = i
            .status
            .iter()
            .flat_map(|s| s.load_balancer.iter())
            .flat_map(|lb| lb.ingress.iter().flatten())
            .filter_map(|a| a.ip.clone().or_else(|| a.hostname.clone()))
            .collect::<Vec<_>>()
            .join(",");
        let ports = if spec
            .and_then(|s| s.tls.as_ref())
            .is_some_and(|t| !t.is_empty())
        {
            "80, 443"
        } else {
            "80"
        };
        (
            vec![
                spec.and_then(|s| s.ingress_class_name.clone())
                    .unwrap_or_else(|| "<none>".into()),
                if hosts.is_empty() { "*".into() } else { hosts },
                address,
                ports.into(),
            ],
            RowKind::Standard,
            None,
        )
    })
}

pub fn config_maps(items: &[ConfigMap], all_ns: bool, now: i64) -> TableData {
    const SPEC: Spec = Spec {
        cols: &[("DATA", 8, true, false)],
        namespaced: true,
        show_name: true,
    };
    build(items, &SPEC, all_ns, now, |c, _| {
        let count =
            c.data.as_ref().map_or(0, |d| d.len()) + c.binary_data.as_ref().map_or(0, |d| d.len());
        (vec![count.to_string()], RowKind::Standard, None)
    })
}

/// Secret values are never rendered; only the type and key count.
pub fn secrets(items: &[Secret], all_ns: bool, now: i64) -> TableData {
    const SPEC: Spec = Spec {
        cols: &[("TYPE", 40, false, false), ("DATA", 8, true, false)],
        namespaced: true,
        show_name: true,
    };
    build(items, &SPEC, all_ns, now, |s, _| {
        let count =
            s.data.as_ref().map_or(0, |d| d.len()) + s.string_data.as_ref().map_or(0, |d| d.len());
        (
            vec![s.type_.clone().unwrap_or_default(), count.to_string()],
            RowKind::Standard,
            None,
        )
    })
}

pub fn persistent_volume_claims(
    items: &[PersistentVolumeClaim],
    all_ns: bool,
    now: i64,
) -> TableData {
    const SPEC: Spec = Spec {
        cols: &[
            ("STATUS", 10, false, false),
            ("VOLUME", 40, false, false),
            ("CAPACITY", 10, true, false),
            ("ACCESS MODES", 16, false, false),
            ("STORAGECLASS", 20, false, false),
        ],
        namespaced: true,
        show_name: true,
    };
    build(items, &SPEC, all_ns, now, |p, _| {
        let spec = p.spec.as_ref();
        let phase = p
            .status
            .as_ref()
            .and_then(|s| s.phase.clone())
            .unwrap_or_default();
        let capacity = p
            .status
            .as_ref()
            .and_then(|s| s.capacity.as_ref())
            .and_then(|c| c.get("storage"))
            .map(|q| q.0.clone())
            .unwrap_or_default();
        let kind = match phase.as_str() {
            "Bound" => RowKind::Standard,
            "Pending" => RowKind::Pending,
            _ => RowKind::Error,
        };
        (
            vec![
                phase,
                spec.and_then(|s| s.volume_name.clone()).unwrap_or_default(),
                capacity,
                spec.and_then(|s| s.access_modes.as_ref())
                    .into_iter()
                    .flatten()
                    .map(|m| match m.as_str() {
                        "ReadWriteOnce" => "RWO",
                        "ReadOnlyMany" => "ROX",
                        "ReadWriteMany" => "RWX",
                        "ReadWriteOncePod" => "RWOP",
                        other => other,
                    })
                    .collect::<Vec<_>>()
                    .join(","),
                spec.and_then(|s| s.storage_class_name.clone())
                    .unwrap_or_default(),
            ],
            kind,
            None,
        )
    })
}

pub fn events(items: &[Event], all_ns: bool, now: i64) -> TableData {
    const SPEC: Spec = Spec {
        cols: &[
            ("LAST SEEN", 10, true, false),
            ("TYPE", 8, false, false),
            ("REASON", 24, false, false),
            ("OBJECT", 50, false, false),
            ("COUNT", 7, true, false),
            ("MESSAGE", 120, false, false),
        ],
        namespaced: true,
        show_name: false,
    };
    build(items, &SPEC, all_ns, now, |e, now| {
        let seen = e
            .last_timestamp
            .as_ref()
            .map(|t| t.0.as_second())
            .or_else(|| e.event_time.as_ref().map(|t| t.0.as_second()))
            .or_else(|| {
                e.metadata
                    .creation_timestamp
                    .as_ref()
                    .map(|t| t.0.as_second())
            });
        let obj = &e.involved_object;
        let object = format!(
            "{}/{}",
            obj.kind.as_deref().unwrap_or("").to_ascii_lowercase(),
            obj.name.as_deref().unwrap_or("")
        );
        let kind = match e.type_.as_deref() {
            Some("Warning") => RowKind::Error,
            _ => RowKind::Standard,
        };
        (
            vec![
                seen.map_or_else(String::new, |s| human_duration(now - s)),
                e.type_.clone().unwrap_or_default(),
                e.reason.clone().unwrap_or_default(),
                object,
                n(e.count.or(Some(1))).to_string(),
                e.message.clone().unwrap_or_default().replace('\n', " "),
            ],
            kind,
            seen,
        )
    })
}

pub fn nodes(items: &[Node], _all_ns: bool, now: i64) -> TableData {
    const SPEC: Spec = Spec {
        cols: &[
            ("STATUS", 20, false, false),
            ("ROLES", 20, false, false),
            ("VERSION", 14, false, false),
            ("INTERNAL-IP", 16, false, true),
            ("OS-IMAGE", 30, false, true),
            ("KERNEL", 24, false, true),
            ("CONTAINER-RUNTIME", 28, false, true),
        ],
        namespaced: false,
        show_name: true,
    };
    build(items, &SPEC, false, now, |node, _| {
        let st = node.status.as_ref();
        let ready = st
            .and_then(|s| s.conditions.as_ref())
            .into_iter()
            .flatten()
            .find(|c| c.type_ == "Ready")
            .is_some_and(|c| c.status == "True");
        let mut status = vec![if ready { "Ready" } else { "NotReady" }.to_owned()];
        if node.spec.as_ref().and_then(|s| s.unschedulable) == Some(true) {
            status.push("SchedulingDisabled".into());
        }
        let mut roles: Vec<&str> = node
            .metadata
            .labels
            .iter()
            .flatten()
            .filter_map(|(k, _)| k.strip_prefix("node-role.kubernetes.io/"))
            .collect();
        roles.sort_unstable();
        let info = st.and_then(|s| s.node_info.as_ref());
        let ip = st
            .and_then(|s| s.addresses.as_ref())
            .into_iter()
            .flatten()
            .find(|a| a.type_ == "InternalIP")
            .map(|a| a.address.clone())
            .unwrap_or_default();
        (
            vec![
                status.join(","),
                if roles.is_empty() {
                    "<none>".into()
                } else {
                    roles.join(",")
                },
                info.map(|i| i.kubelet_version.clone()).unwrap_or_default(),
                ip,
                info.map(|i| i.os_image.clone()).unwrap_or_default(),
                info.map(|i| i.kernel_version.clone()).unwrap_or_default(),
                info.map(|i| i.container_runtime_version.clone())
                    .unwrap_or_default(),
            ],
            if ready {
                RowKind::Standard
            } else {
                RowKind::Error
            },
            None,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deployment_row_shows_ready_ratio_and_flags_unready() {
        let mut d = Deployment::default();
        d.metadata.name = Some("api".into());
        d.metadata.namespace = Some("varia".into());
        d.spec = Some(Default::default());
        if let Some(s) = d.spec.as_mut() {
            s.replicas = Some(3);
        }
        let t = deployments(&[d], true, 0);
        let names: Vec<_> = t.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(&names[..3], ["NAMESPACE", "NAME", "READY"]);
        let row = t
            .rows
            .first()
            .map(|r| (r.id.as_str(), r.cells.get(2), r.kind));
        assert_eq!(
            row,
            Some(("varia/api", Some(&"0/3".to_owned()), RowKind::Pending))
        );
    }

    #[test]
    fn node_ids_have_an_empty_namespace_and_no_namespace_column() {
        let mut node = Node::default();
        node.metadata.name = Some("n1".into());
        let t = nodes(&[node], true, 0);
        assert_eq!(t.columns.first().map(|c| c.name.as_str()), Some("NAME"));
        assert_eq!(t.rows.first().map(|r| r.id.as_str()), Some("/n1"));
    }
}
