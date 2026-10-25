//! Run with: `cargo test -p k9s_kube --test live -- --ignored --nocapture`
//! Reads pods from the current kubeconfig context (K9S_TEST_NS, default "default").

use k9s_core::{Kind, Update};
use k9s_kube::{Options, spawn};

#[test]
#[ignore = "needs a live cluster"]
fn lists_pods_from_cluster() {
    let rx = spawn(Options {
        namespace: std::env::var("K9S_TEST_NS").ok(),
        ..Options::default()
    });
    let t = loop {
        match rx.recv_blocking().unwrap() {
            Update::Info(i) => println!("{i:?}"),
            Update::Stats(st) => println!("{st:?}"),
            Update::Table(t) => break t,
            Update::Error(e) => panic!("{e}"),
        }
    };
    for c in &t.columns {
        print!("{:<w$}", c.name, w = c.width as usize / 2);
    }
    println!();
    for r in &t.rows {
        println!("{}  [{:?}]", r.cells.join(" | "), r.kind);
    }
    assert!(!t.columns.is_empty());
}

#[test]
#[ignore = "needs a live cluster"]
fn lists_every_kind_and_fetches_yaml() {
    for kind in Kind::ALL {
        let rx = spawn(Options {
            namespace: Some(std::env::var("K9S_TEST_NS").unwrap_or_else(|_| "varia".into())),
            kind,
            ..Options::default()
        });
        let t = loop {
            match rx.recv_blocking().expect("worker replied") {
                Update::Table(t) => break t,
                Update::Error(e) => panic!("{kind:?}: {e}"),
                Update::Info(_) | Update::Stats(_) => {}
            }
        };
        println!(
            "{kind:?}: {} columns, {} rows",
            t.columns.len(),
            t.rows.len()
        );
        assert!(!t.columns.is_empty(), "{kind:?}");
        let Some(row) = t.rows.first() else { continue };
        let (ns, name) = row.id.split_once('/').expect("ns/name id");
        let rx = k9s_kube::fetch_text(
            Options::default(),
            k9s_kube::TextKind::Yaml,
            kind,
            ns.into(),
            name.into(),
        );
        let yaml = rx.recv_blocking().expect("reply").expect("yaml");
        assert!(yaml.contains("kind:") || yaml.contains("Name:"), "{kind:?}");
        let rx = k9s_kube::fetch_text(
            Options::default(),
            k9s_kube::TextKind::Describe,
            kind,
            ns.into(),
            name.into(),
        );
        let text = rx.recv_blocking().expect("reply").expect("describe");
        assert!(text.contains("Events:"), "{kind:?}");
    }
}

#[test]
#[ignore = "needs a live cluster"]
fn lists_contexts_and_namespaces() {
    let (contexts, current) = k9s_kube::list_contexts();
    assert!(!contexts.is_empty());
    println!("contexts={contexts:?} current={current:?}");
    let rx = k9s_kube::list_namespaces(k9s_kube::Options::default());
    let namespaces = rx.recv_blocking().expect("worker replied").expect("listed");
    println!("namespaces={}", namespaces.len());
    assert!(namespaces.iter().any(|n| n == "varia"));
}

#[test]
#[ignore = "needs a live cluster"]
fn fetches_pod_texts() {
    use k9s_kube::{TextKind, fetch_text};
    let pod = std::env::var("K9S_TEST_POD")
        .unwrap_or_else(|_| "varia-dm-backend-7498764c49-k7dvz".into());
    for kind in [TextKind::Yaml, TextKind::Describe] {
        let rx = fetch_text(
            k9s_kube::Options::default(),
            kind,
            Kind::Pod,
            "varia".into(),
            pod.clone(),
        );
        let text = rx.recv_blocking().expect("reply").expect("text");
        println!(
            "{kind:?}: {} lines; first: {:?}",
            text.lines().count(),
            text.lines().next()
        );
        assert!(!text.is_empty());
    }
}

#[test]
#[ignore = "needs a live cluster"]
fn streams_logs() {
    let pod = std::env::var("K9S_TEST_POD")
        .unwrap_or_else(|_| "varia-dm-backend-7498764c49-k7dvz".into());
    let opts = k9s_kube::Options::default();
    let containers = k9s_kube::list_containers(opts.clone(), "varia".into(), pod.clone())
        .recv_blocking()
        .expect("reply")
        .expect("containers");
    for r in &containers.rows {
        println!("{}", r.cells.join(" | "));
    }
    let containers: Vec<String> = containers.rows.into_iter().map(|r| r.id).collect();
    println!("containers={containers:?}");
    let rx = k9s_kube::stream_logs(opts, "varia".into(), pod, containers[0].clone(), false);
    let first = rx.recv_blocking().expect("line").expect("ok");
    println!("first log line: {first:.80}");
}
