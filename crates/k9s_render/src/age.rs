/// Mirrors apimachinery `duration.HumanDuration`.
pub fn human_duration(secs: i64) -> String {
    let secs = secs.max(0);
    let minutes = secs / 60;
    let hours = minutes / 60;
    let days = hours / 24;
    if secs < 120 {
        format!("{secs}s")
    } else if minutes < 10 {
        let s = secs % 60;
        if s == 0 {
            format!("{minutes}m")
        } else {
            format!("{minutes}m{s}s")
        }
    } else if minutes < 180 {
        format!("{minutes}m")
    } else if hours < 8 {
        let m = minutes % 60;
        if m == 0 {
            format!("{hours}h")
        } else {
            format!("{hours}h{m}m")
        }
    } else if hours < 48 {
        format!("{hours}h")
    } else if hours < 24 * 8 {
        let h = hours % 24;
        if h == 0 {
            format!("{days}d")
        } else {
            format!("{days}d{h}h")
        }
    } else if days < 365 * 2 {
        format!("{days}d")
    } else if days < 365 * 8 {
        let d = days % 365;
        if d == 0 {
            format!("{}y", days / 365)
        } else {
            format!("{}y{d}d", days / 365)
        }
    } else {
        format!("{}y", days / 365)
    }
}

#[cfg(test)]
mod tests {
    use super::human_duration as h;

    #[test]
    fn matches_kubectl_style() {
        assert_eq!(h(5), "5s");
        assert_eq!(h(180), "3m");
        assert_eq!(h(185), "3m5s");
        assert_eq!(h(3 * 3600 + 120), "3h2m");
        assert_eq!(h(8 * 3600), "8h");
        assert_eq!(h(4 * 86400 + 19 * 3600), "4d19h");
        assert_eq!(h(12 * 86400), "12d");
    }
}
