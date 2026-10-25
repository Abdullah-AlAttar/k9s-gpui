/// Resource kinds the table view can browse.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Pod,
    Deployment,
    StatefulSet,
    DaemonSet,
    ReplicaSet,
    Job,
    CronJob,
    Service,
    Ingress,
    ConfigMap,
    Secret,
    PersistentVolumeClaim,
    Event,
    Node,
}

impl Kind {
    pub const ALL: [Self; 14] = [
        Self::Pod,
        Self::Deployment,
        Self::StatefulSet,
        Self::DaemonSet,
        Self::ReplicaSet,
        Self::Job,
        Self::CronJob,
        Self::Service,
        Self::Ingress,
        Self::ConfigMap,
        Self::Secret,
        Self::PersistentVolumeClaim,
        Self::Event,
        Self::Node,
    ];

    /// Resolves a `:command` name or alias (k9s style, e.g. `deploy`, `svc`).
    pub fn parse(name: &str) -> Option<Self> {
        let n = name.to_ascii_lowercase();
        Some(match n.as_str() {
            "po" | "pod" | "pods" => Self::Pod,
            "dp" | "deploy" | "deployment" | "deployments" => Self::Deployment,
            "sts" | "statefulset" | "statefulsets" => Self::StatefulSet,
            "ds" | "daemonset" | "daemonsets" => Self::DaemonSet,
            "rs" | "replicaset" | "replicasets" => Self::ReplicaSet,
            "job" | "jobs" => Self::Job,
            "cj" | "cronjob" | "cronjobs" => Self::CronJob,
            "svc" | "service" | "services" => Self::Service,
            "ing" | "ingress" | "ingresses" => Self::Ingress,
            "cm" | "configmap" | "configmaps" => Self::ConfigMap,
            "sec" | "secret" | "secrets" => Self::Secret,
            "pvc" | "persistentvolumeclaim" | "persistentvolumeclaims" => {
                Self::PersistentVolumeClaim
            }
            "ev" | "event" | "events" => Self::Event,
            "no" | "node" | "nodes" => Self::Node,
            _ => return None,
        })
    }

    /// Lower-case plural shown in the table title.
    pub fn title(self) -> &'static str {
        match self {
            Self::Pod => "pods",
            Self::Deployment => "deployments",
            Self::StatefulSet => "statefulsets",
            Self::DaemonSet => "daemonsets",
            Self::ReplicaSet => "replicasets",
            Self::Job => "jobs",
            Self::CronJob => "cronjobs",
            Self::Service => "services",
            Self::Ingress => "ingresses",
            Self::ConfigMap => "configmaps",
            Self::Secret => "secrets",
            Self::PersistentVolumeClaim => "persistentvolumeclaims",
            Self::Event => "events",
            Self::Node => "nodes",
        }
    }

    pub fn namespaced(self) -> bool {
        self != Self::Node
    }
}

#[cfg(test)]
mod tests {
    use super::Kind;

    #[test]
    fn aliases_resolve_and_every_title_round_trips() {
        assert_eq!(Kind::parse("SVC"), Some(Kind::Service));
        assert_eq!(Kind::parse("deploy"), Some(Kind::Deployment));
        assert_eq!(Kind::parse("nope"), None);
        for k in Kind::ALL {
            assert_eq!(Kind::parse(k.title()), Some(k));
        }
    }
}
