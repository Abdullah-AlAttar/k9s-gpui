use std::cmp::Ordering;

/// Colour class of a row, mirroring k9s' per-status colouring.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RowKind {
    #[default]
    Standard,
    Pending,
    Completed,
    Error,
    Killing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    pub width: u32,
    pub right_align: bool,
    /// Sorted by `Row::age` (creation time) instead of the display text.
    pub is_age: bool,
    /// Only shown in the wide view.
    pub wide: bool,
}

impl Column {
    pub fn new(name: &str, width: u32) -> Self {
        Self {
            name: name.to_owned(),
            width,
            right_align: false,
            is_age: false,
            wide: false,
        }
    }

    pub fn age(mut self) -> Self {
        self.is_age = true;
        self
    }

    pub fn wide(mut self) -> Self {
        self.wide = true;
        self
    }

    pub fn right(mut self) -> Self {
        self.right_align = true;
        self
    }
}

/// Semantic colour class of a text span; the UI maps tones to theme colours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Plain,
    Dim,
    Key,
    Str,
    Num,
    Bool,
    Null,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub tone: Tone,
}

impl Span {
    pub fn new(text: impl Into<String>, tone: Tone) -> Self {
        Self {
            text: text.into(),
            tone,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub cells: Vec<String>,
    pub kind: RowKind,
    /// Creation time in unix seconds, used to sort age columns.
    pub age: Option<i64>,
    /// Part of the "faults" view: something is wrong with this row.
    pub fault: bool,
    /// Optional rich rendering of the first cell; empty means plain text.
    pub spans: Vec<Span>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SortKey {
    pub column: usize,
    pub ascending: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TableData {
    pub columns: Vec<Column>,
    pub rows: Vec<Row>,
}

impl TableData {
    /// Column widths in characters: fit the content, never below the header
    /// (plus room for the sort arrow and a gap) nor above the column's `width` cap.
    pub fn column_widths(&self) -> Vec<usize> {
        self.columns
            .iter()
            .enumerate()
            .map(|(i, col)| {
                let header = col.name.chars().count() + 2;
                let cap = (col.width as usize).max(header);
                let content = self
                    .rows
                    .iter()
                    .map(|r| r.cells.get(i).map_or(0, |c| c.chars().count()) + 2)
                    .max()
                    .unwrap_or(0);
                content.clamp(header, cap)
            })
            .collect()
    }

    /// Sorts rows by column using natural order; ties break on row id.
    pub fn sort(&mut self, key: SortKey) {
        self.rows.sort_by(|a, b| {
            let is_age = self.columns.get(key.column).is_some_and(|c| c.is_age);
            let primary = if is_age {
                // Youngest first when ascending.
                b.age.cmp(&a.age)
            } else {
                let ca = a.cells.get(key.column).map_or("", String::as_str);
                let cb = b.cells.get(key.column).map_or("", String::as_str);
                natural_cmp(ca, cb)
            };
            let ord = primary.then_with(|| a.id.cmp(&b.id));
            if key.ascending { ord } else { ord.reverse() }
        });
    }
}

/// Compares strings treating digit runs as numbers ("pod-2" < "pod-10").
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let na = take_number(&mut a);
                let nb = take_number(&mut b);
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                if x != y {
                    return x.cmp(&y);
                }
                a.next();
                b.next();
            }
        }
    }
}

fn take_number(it: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut s = String::new();
    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
        s.push(c);
        it.next();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, name: &str) -> Row {
        Row {
            id: id.into(),
            cells: vec![name.into()],
            kind: RowKind::Standard,
            age: None,
            fault: false,
            spans: Vec::new(),
        }
    }

    #[test]
    fn widths_fit_content_within_header_and_cap() {
        let mut t = TableData {
            columns: vec![Column::new("NAME", 10), Column::new("RESTARTS", 20)],
            rows: vec![row("a", "a-very-long-name-here")],
        };
        t.rows[0].cells.push("1".into());
        // NAME is capped at 10; RESTARTS never shrinks below header + 2.
        assert_eq!(t.column_widths(), [10, 10]);
        t.rows[0].cells[0] = "ab".into();
        assert_eq!(t.column_widths(), [6, 10]);
    }

    #[test]
    fn natural_order_compares_numbers() {
        assert_eq!(natural_cmp("pod-2", "pod-10"), Ordering::Less);
        assert_eq!(natural_cmp("a", "a"), Ordering::Equal);
        assert_eq!(natural_cmp("b1", "a9"), Ordering::Greater);
    }

    #[test]
    fn sort_flips_direction_and_breaks_ties_on_id() {
        let mut t = TableData {
            columns: vec![Column::new("NAME", 10)],
            rows: vec![row("z", "p-10"), row("b", "p-2"), row("a", "p-2")],
        };
        t.sort(SortKey {
            column: 0,
            ascending: true,
        });
        let ids: Vec<_> = t.rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "z"]);
        t.sort(SortKey {
            column: 0,
            ascending: false,
        });
        let ids: Vec<_> = t.rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["z", "b", "a"]);
    }
}
