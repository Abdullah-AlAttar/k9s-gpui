//! Application state and the worker-to-UI bridge.

pub use k9s_core::{ClusterInfo, ClusterStats, Kind, Update};
pub use k9s_kube::Options;

/// Starts the cluster worker and returns the stream of updates for the UI.
pub fn start(options: Options) -> async_channel::Receiver<Update> {
    k9s_kube::spawn(options)
}

pub use k9s_kube::list_contexts;

/// Namespaces of the cluster selected by `options`, delivered once.
pub fn list_namespaces(options: Options) -> async_channel::Receiver<Result<Vec<String>, String>> {
    k9s_kube::list_namespaces(options)
}

pub use k9s_kube::TextKind;

/// Fetches a text view (YAML, describe, logs) of one pod, delivered once.
pub fn fetch_text(
    options: Options,
    kind: TextKind,
    resource: Kind,
    namespace: String,
    name: String,
) -> async_channel::Receiver<Result<String, String>> {
    k9s_kube::fetch_text(options, kind, resource, namespace, name)
}

/// Container table of one pod, delivered once.
pub fn list_containers(
    options: Options,
    namespace: String,
    name: String,
) -> async_channel::Receiver<Result<k9s_core::TableData, String>> {
    k9s_kube::list_containers(options, namespace, name)
}

/// Streams the log lines of one container; dropping the receiver stops it.
pub fn stream_logs(
    options: Options,
    namespace: String,
    name: String,
    container: String,
    previous: bool,
) -> async_channel::Receiver<Result<String, String>> {
    k9s_kube::stream_logs(options, namespace, name, container, previous)
}

/// Deletes one resource; the result is delivered once.
pub fn delete_resource(
    options: Options,
    kind: Kind,
    namespace: String,
    name: String,
) -> async_channel::Receiver<Result<(), String>> {
    k9s_kube::delete_resource(options, kind, namespace, name)
}

pub use k9s_kube::Action;

/// Runs a scale/restart/cordon action; the confirmation message is delivered once.
pub fn run_action(
    options: Options,
    kind: Kind,
    namespace: String,
    name: String,
    action: Action,
) -> async_channel::Receiver<Result<String, String>> {
    k9s_kube::run_action(options, kind, namespace, name, action)
}
