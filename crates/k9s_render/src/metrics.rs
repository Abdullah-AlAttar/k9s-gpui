use std::collections::HashMap;

/// Summed container usage for one pod.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Usage {
    pub cpu_milli: i64,
    pub mem_bytes: i64,
}

/// Pod usage keyed by `namespace/name`.
pub type PodMetrics = HashMap<String, Usage>;

/// Parses a Kubernetes CPU quantity ("250m", "1", "123456n") into millicores.
pub fn parse_cpu_milli(q: &str) -> Option<i64> {
    let q = q.trim();
    let (num, div, mul) = match q.chars().last()? {
        'n' => (&q[..q.len() - 1], 1_000_000.0, 1.0),
        'u' => (&q[..q.len() - 1], 1_000.0, 1.0),
        'm' => (&q[..q.len() - 1], 1.0, 1.0),
        _ => (q, 1.0, 1000.0),
    };
    let v: f64 = num.parse().ok()?;
    Some((v * mul / div).round() as i64)
}

/// Parses a Kubernetes memory quantity ("128Mi", "1G", "1024") into bytes.
pub fn parse_mem_bytes(q: &str) -> Option<i64> {
    let q = q.trim();
    const UNITS: [(&str, f64); 12] = [
        ("Ki", 1024.0),
        ("Mi", 1024.0 * 1024.0),
        ("Gi", 1024.0 * 1024.0 * 1024.0),
        ("Ti", 1024.0 * 1024.0 * 1024.0 * 1024.0),
        ("k", 1e3),
        ("K", 1e3),
        ("M", 1e6),
        ("G", 1e9),
        ("T", 1e12),
        ("P", 1e15),
        ("E", 1e18),
        ("m", 1e-3),
    ];
    for (suffix, mul) in UNITS {
        if let Some(num) = q.strip_suffix(suffix) {
            return Some((num.parse::<f64>().ok()? * mul).round() as i64);
        }
    }
    Some(q.parse::<f64>().ok()?.round() as i64)
}

/// k9s' `ToPercentageStr`: floor(value / total * 100), or "n/a" without a denominator.
pub fn percent(value: i64, total: i64) -> String {
    if total <= 0 {
        "n/a".into()
    } else {
        (value * 100 / total).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quantities() {
        assert_eq!(parse_cpu_milli("250m"), Some(250));
        assert_eq!(parse_cpu_milli("1"), Some(1000));
        assert_eq!(parse_cpu_milli("2500000n"), Some(3));
        assert_eq!(parse_mem_bytes("128Mi"), Some(128 * 1024 * 1024));
        assert_eq!(parse_mem_bytes("1G"), Some(1_000_000_000));
        assert_eq!(parse_mem_bytes("1024"), Some(1024));
        assert_eq!(parse_mem_bytes("12Ki"), Some(12288));
    }

    #[test]
    fn percent_floors_and_guards_zero() {
        assert_eq!(percent(150, 400), "37");
        assert_eq!(percent(1, 0), "n/a");
    }
}
