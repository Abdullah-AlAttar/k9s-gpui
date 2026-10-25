//! Structured (JSON) log line rendering into toned spans.

use k9s_core::{Span, Tone};
use serde_json::Value;

const TIME_KEYS: &[&str] = &["time", "timestamp", "ts", "@timestamp", "t"];
const LEVEL_KEYS: &[&str] = &["level", "log.level", "severity", "lvl"];
const MESSAGE_KEYS: &[&str] = &["message", "msg", "log.message"];

/// Renders a JSON-object log line as `time LEVEL message key=value ...`.
/// Returns `None` for anything that is not a JSON object, so callers show it raw.
pub fn render(line: &str) -> Option<Vec<Span>> {
    let trimmed = line.trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    let Value::Object(map) = serde_json::from_str::<Value>(trimmed).ok()? else {
        return None;
    };
    let find = |keys: &'static [&'static str]| keys.iter().find(|k| map.contains_key(**k)).copied();
    let (time_key, level_key, message_key) =
        (find(TIME_KEYS), find(LEVEL_KEYS), find(MESSAGE_KEYS));
    let mut spans = Vec::new();
    if let Some(time) = time_key.and_then(|k| map.get(k)) {
        spans.push(Span::new(short_time(&display(time)), Tone::Dim));
        spans.push(Span::new(" ", Tone::Plain));
    }
    if let Some(level) = level_key.and_then(|k| map.get(k)) {
        let text = display(level).to_uppercase();
        spans.push(Span::new(format!("{text:<5}"), level_tone(&text)));
        spans.push(Span::new(" ", Tone::Plain));
    }
    if let Some(message) = message_key.and_then(|k| map.get(k)) {
        spans.push(Span::new(display(message), Tone::Plain));
    }
    for (key, value) in &map {
        let used = [time_key, level_key, message_key];
        if used.contains(&Some(key.as_str())) {
            continue;
        }
        spans.push(Span::new(" ", Tone::Plain));
        spans.push(Span::new(key.clone(), Tone::Key));
        spans.push(Span::new("=", Tone::Dim));
        spans.push(value_span(value));
    }
    Some(spans)
}

fn value_span(value: &Value) -> Span {
    match value {
        Value::String(s) if s.is_empty() || s.contains(char::is_whitespace) => {
            Span::new(format!("{s:?}"), Tone::Str)
        }
        Value::String(s) => Span::new(s.clone(), Tone::Str),
        Value::Number(n) => Span::new(n.to_string(), Tone::Num),
        Value::Bool(b) => Span::new(b.to_string(), Tone::Bool),
        Value::Null => Span::new("null", Tone::Null),
        other => Span::new(other.to_string(), Tone::Dim),
    }
}

fn display(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// `2026-10-25T11:00:00.123Z` becomes `11:00:00.123`; anything else is kept.
fn short_time(time: &str) -> String {
    match time.split_once('T') {
        Some((_, rest)) if time.len() >= 19 => {
            let rest = rest.trim_end_matches('Z');
            let end = rest.find(['+', '-']).unwrap_or(rest.len());
            rest.get(..end.min(12)).unwrap_or(rest).to_owned()
        }
        _ => time.to_owned(),
    }
}

fn level_tone(level: &str) -> Tone {
    match level {
        "ERROR" | "ERR" | "FATAL" | "PANIC" | "CRITICAL" => Tone::Error,
        "WARN" | "WARNING" => Tone::Warn,
        "INFO" => Tone::Info,
        _ => Tone::Debug,
    }
}

/// Splits spans into chunks of at most `width` characters, keeping tones.
pub fn wrap(spans: &[Span], width: usize) -> Vec<Vec<Span>> {
    let width = width.max(1);
    let mut lines: Vec<Vec<Span>> = vec![Vec::new()];
    let mut used = 0;
    for span in spans {
        let mut chars = span.text.chars().peekable();
        while chars.peek().is_some() {
            if used == width {
                lines.push(Vec::new());
                used = 0;
            }
            let part: String = chars.by_ref().take(width - used).collect();
            used += part.chars().count();
            if let Some(line) = lines.last_mut() {
                line.push(Span::new(part, span.tone));
            }
        }
    }
    lines
}

/// Parses ANSI SGR colours into spans; `None` when the line has no escapes.
pub fn ansi(line: &str) -> Option<Vec<Span>> {
    if !line.contains('\u{1b}') {
        return None;
    }
    let mut spans = Vec::new();
    let mut text = String::new();
    let mut tone = Tone::Plain;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            text.push(c);
            continue;
        }
        if chars.peek() != Some(&'[') {
            continue;
        }
        chars.next();
        let mut params = String::new();
        let mut end = None;
        for p in chars.by_ref() {
            if p.is_ascii_digit() || p == ';' {
                params.push(p);
            } else {
                end = Some(p);
                break;
            }
        }
        if end != Some('m') {
            continue;
        }
        if !text.is_empty() {
            spans.push(Span::new(std::mem::take(&mut text), tone));
        }
        for code in params.split(';') {
            tone = match code {
                "" | "0" | "39" | "1" | "22" | "37" | "97" => Tone::Plain,
                "2" | "30" | "90" => Tone::Dim,
                "31" | "91" => Tone::Error,
                "32" | "92" => Tone::Info,
                "33" | "93" => Tone::Warn,
                "34" | "94" => Tone::Key,
                "35" | "95" => Tone::Debug,
                "36" | "96" => Tone::Str,
                _ => tone,
            };
        }
    }
    if !text.is_empty() {
        spans.push(Span::new(text, tone));
    }
    Some(spans)
}

/// The line without any ANSI escape sequences.
pub fn strip(line: &str) -> String {
    ansi(line).map_or_else(
        || line.to_owned(),
        |spans| spans.iter().map(|s| s.text.as_str()).collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(spans: &[Span]) -> String {
        spans.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn ansi_colours_become_tones() {
        let spans = ansi("\u{1b}[90m8:21PM \u{1b}[0m \u{1b}[32mINF\u{1b}[0m hi").unwrap();
        assert_eq!(spans[0], Span::new("8:21PM ", Tone::Dim));
        assert_eq!(spans[2], Span::new("INF", Tone::Info));
        assert_eq!(strip("\u{1b}[1mx\u{1b}[0m"), "x");
        assert!(ansi("plain").is_none());
    }

    #[test]
    fn wrap_splits_by_width_keeping_tones() {
        let spans = vec![Span::new("abcd", Tone::Key), Span::new("efg", Tone::Plain)];
        let out = wrap(&spans, 3);
        let text: Vec<String> = out
            .iter()
            .map(|l| l.iter().map(|s| s.text.as_str()).collect())
            .collect();
        assert_eq!(text, ["abc", "def", "g"]);
        assert_eq!(out[1][0].tone, Tone::Key);
    }

    #[test]
    fn renders_time_level_message_then_fields_in_order() {
        let line = r#"{"zeta":1,"@timestamp":"2026-10-25T11:00:00.123Z","log.level":"error","message":"boom","db":"pg","ok":false}"#;
        let spans = render(line).unwrap();
        assert_eq!(
            text(&spans),
            "11:00:00.123 ERROR boom zeta=1 db=pg ok=false"
        );
        assert_eq!(spans[2].tone, Tone::Error);
    }

    #[test]
    fn plain_text_and_broken_json_stay_raw() {
        assert!(render("just a line").is_none());
        assert!(render("{not json").is_none());
        assert!(render("[1,2]").is_none());
    }

    #[test]
    fn strings_with_spaces_are_quoted() {
        let spans = render(r#"{"msg":"x","path":"a b"}"#).unwrap();
        assert_eq!(text(&spans), "x path=\"a b\"");
    }
}
