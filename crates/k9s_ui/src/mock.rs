use k9s_core::{Column, Row, RowKind, TableData};

/// Static pod rows used until the live cluster source lands.
pub(crate) fn pods(count: usize) -> TableData {
    let columns = vec![
        Column::new("NAMESPACE", 14),
        Column::new("NAME", 36),
        Column::new("READY", 7),
        Column::new("STATUS", 18),
        Column::new("RESTARTS", 9).right(),
        Column::new("IP", 16),
        Column::new("NODE", 18),
        Column::new("AGE", 6).right().age(),
    ];
    let statuses = [
        ("Running", RowKind::Standard, "1/1"),
        ("Pending", RowKind::Pending, "0/1"),
        ("Completed", RowKind::Completed, "0/1"),
        ("CrashLoopBackOff", RowKind::Error, "0/1"),
        ("Terminating", RowKind::Killing, "1/1"),
    ];
    let rows = (0..count)
        .map(|i| {
            let (status, kind, ready) = statuses.iter().cycle().nth(i).copied().unwrap_or_default();
            let ns = ["default", "kube-system", "monitoring"]
                .iter()
                .cycle()
                .nth(i)
                .copied()
                .unwrap_or_default();
            let name = format!("app-{i}-7d9f8c6b5-x{:03}", i % 1000);
            Row {
                id: format!("{ns}/{name}"),
                cells: vec![
                    ns.into(),
                    name,
                    ready.into(),
                    status.into(),
                    (i % 4).to_string(),
                    format!("10.0.{}.{}", (i / 250) % 250, i % 250),
                    format!("node-{}", i % 5),
                    format!("{}m", (i % 59) + 1),
                ],
                kind,
                age: None,
                spans: Vec::new(),
                fault: kind == RowKind::Error,
            }
        })
        .collect();
    TableData { columns, rows }
}
