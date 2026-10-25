use crate::TableData;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClusterInfo {
    pub context: String,
    pub cluster: String,
    pub user: String,
    pub namespace: String,
    pub all_namespaces: bool,
    pub k8s_rev: String,
}

/// Cluster-wide utilisation in percent; `None` when metrics are unavailable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClusterStats {
    pub cpu: Option<u32>,
    pub mem: Option<u32>,
}

/// Messages from the kube worker to the UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Update {
    Info(ClusterInfo),
    Stats(ClusterStats),
    Table(TableData),
    Error(String),
}
