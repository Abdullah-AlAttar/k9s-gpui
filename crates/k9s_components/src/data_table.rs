use std::ops::Range;

use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, IntoElement, KeyBinding, Render,
    ScrollStrategy, UniformListScrollHandle, Window, actions, div, prelude::*, px, rgb,
    uniform_list,
};
use k9s_core::{RowKind, SortKey, TableData};

use crate::theme::Theme;

actions!(
    data_table,
    [
        MoveUp,
        MoveDown,
        MoveTop,
        MoveBottom,
        PageUp,
        PageDown,
        SortName,
        SortAge,
        SortStatus,
        ToggleWide,
        ToggleFaults,
        Confirm
    ]
);

const PAGE: usize = 20;
pub const FONT: &str = "monospace";
pub const DEFAULT_FONT_PX: f32 = 13.0;
const ROW_HEIGHT_EM: f32 = 1.7;

pub fn bindings() -> Vec<KeyBinding> {
    let c = Some("DataTable");
    vec![
        KeyBinding::new("up", MoveUp, c),
        KeyBinding::new("k", MoveUp, c),
        KeyBinding::new("down", MoveDown, c),
        KeyBinding::new("j", MoveDown, c),
        KeyBinding::new("g", MoveTop, c),
        KeyBinding::new("home", MoveTop, c),
        KeyBinding::new("shift-g", MoveBottom, c),
        KeyBinding::new("end", MoveBottom, c),
        KeyBinding::new("ctrl-b", PageUp, c),
        KeyBinding::new("pageup", PageUp, c),
        KeyBinding::new("ctrl-f", PageDown, c),
        KeyBinding::new("pagedown", PageDown, c),
        KeyBinding::new("shift-n", SortName, c),
        KeyBinding::new("shift-a", SortAge, c),
        KeyBinding::new("shift-s", SortStatus, c),
        KeyBinding::new("ctrl-w", ToggleWide, c),
        KeyBinding::new("ctrl-z", ToggleFaults, c),
        KeyBinding::new("enter", Confirm, c),
    ]
}

/// Emitted when the user presses Enter on a row; carries the row id.
pub struct Open(pub String);

pub struct DataTable {
    source: TableData,
    filter: String,
    wide: bool,
    faults: bool,
    data: TableData,
    theme: Theme,
    font_px: f32,
    char_px: f32,
    /// Pixel width per column; `None` for hidden (wide-only) columns.
    col_px: Vec<Option<f32>>,
    selected: usize,
    sort: Option<SortKey>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
}

impl DataTable {
    pub fn new(data: TableData, theme: Theme, cx: &mut Context<Self>) -> Self {
        Self {
            source: data.clone(),
            filter: String::new(),
            wide: false,
            faults: false,
            data,
            theme,
            font_px: DEFAULT_FONT_PX,
            char_px: DEFAULT_FONT_PX * 0.6,
            col_px: Vec::new(),
            selected: 0,
            sort: None,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
        }
    }

    /// Single source of truth for UI scale: row height, padding and column widths follow it.
    pub fn set_font_px(&mut self, px: f32, cx: &mut Context<Self>) {
        self.font_px = px;
        cx.notify();
    }

    fn row_height(&self) -> f32 {
        (self.font_px * ROW_HEIGHT_EM).round()
    }

    fn measure_char(&mut self, window: &Window) {
        let system = window.text_system();
        let font = gpui::font(FONT);
        let id = system.resolve_font(&font);
        self.char_px = system
            .advance(id, px(self.font_px), 'm')
            .map_or(self.font_px * 0.6, |s| f32::from(s.width));
    }

    /// Replaces the rows like `set_data`, keeping the cursor on the last row if it was there.
    pub fn set_data_follow(&mut self, data: TableData, cx: &mut Context<Self>) {
        let at_end = self.selected + 1 >= self.data.rows.len();
        self.set_data(data, cx);
        if at_end {
            self.select_last(cx);
        }
    }

    pub fn first_row_spans(&self) -> usize {
        self.data.rows.first().map_or(0, |r| r.spans.len())
    }

    pub fn select_last(&mut self, cx: &mut Context<Self>) {
        self.select(usize::MAX, cx);
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn sort_key(&self) -> Option<SortKey> {
        self.sort
    }

    pub fn row_count(&self) -> usize {
        self.data.rows.len()
    }

    pub fn set_data(&mut self, data: TableData, cx: &mut Context<Self>) {
        if self.source.columns != data.columns {
            self.sort = None;
        }
        self.source = data;
        self.rebuild(cx);
    }

    /// Case-insensitive regex filter over all cells; a leading `!` inverts it.
    pub fn set_filter(&mut self, filter: &str, cx: &mut Context<Self>) {
        self.filter = filter.to_owned();
        self.rebuild(cx);
    }

    pub fn wide(&self) -> bool {
        self.wide
    }

    pub fn faults(&self) -> bool {
        self.faults
    }

    fn visible_columns(&self) -> impl Iterator<Item = usize> + '_ {
        self.source
            .columns
            .iter()
            .enumerate()
            .filter(|(_, c)| self.wide || !c.wide)
            .map(|(i, _)| i)
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    /// Ids of the visible rows; consecutive duplicates (wrapped pieces) collapse.
    pub fn visible_ids(&self) -> Vec<&str> {
        let mut ids: Vec<&str> = self.data.rows.iter().map(|r| r.id.as_str()).collect();
        ids.dedup();
        ids
    }

    pub fn selected_row_id(&self) -> Option<&str> {
        self.data.rows.get(self.selected).map(|r| r.id.as_str())
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let selected_id = self.selected_row_id().map(str::to_owned);
        let (inverse, pattern) = match self.filter.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, self.filter.as_str()),
        };
        // An incomplete regex while typing falls back to a literal match.
        let matcher = regex::RegexBuilder::new(pattern)
            .case_insensitive(true)
            .build()
            .or_else(|_| {
                regex::RegexBuilder::new(&regex::escape(pattern))
                    .case_insensitive(true)
                    .build()
            })
            .ok();
        let mut data = self.source.clone();
        if self.faults {
            data.rows.retain(|r| r.fault);
        }
        if let Some(re) = matcher.filter(|_| !pattern.is_empty()) {
            data.rows
                .retain(|r| r.cells.iter().any(|c| re.is_match(c)) != inverse);
        }
        self.data = data;
        if let Some(key) = self.sort {
            self.data.sort(key);
        }
        // Keep the cursor on the same row when the list changes underneath it.
        let moved = selected_id.and_then(|id| self.data.rows.iter().position(|r| r.id == id));
        self.selected = moved
            .unwrap_or(self.selected)
            .min(self.data.rows.len().saturating_sub(1));
        cx.notify();
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected = index.min(self.data.rows.len().saturating_sub(1));
        self.scroll
            .scroll_to_item(self.selected, ScrollStrategy::Center);
        cx.notify();
    }

    fn sort_by_header(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(column) = self.data.columns.iter().position(|c| c.name == name) else {
            return;
        };
        let ascending = match self.sort {
            Some(k) if k.column == column => !k.ascending,
            _ => true,
        };
        let key = SortKey { column, ascending };
        self.sort = Some(key);
        self.rebuild(cx);
    }

    fn row_color(&self, kind: RowKind) -> u32 {
        let t = &self.theme;
        match kind {
            RowKind::Standard => t.foreground,
            RowKind::Pending => t.pending,
            RowKind::Completed => t.completed,
            RowKind::Error => t.error,
            RowKind::Killing => t.killing,
        }
    }

    fn render_rows(&self, range: Range<usize>) -> Vec<gpui::Stateful<gpui::Div>> {
        let t = self.theme;
        range
            .filter_map(|ix| self.data.rows.get(ix).map(|row| (ix, row)))
            .map(|(ix, row)| {
                let selected = ix == self.selected;
                let (fg, bg) = if selected {
                    (t.selection_fg, t.selection_bg)
                } else {
                    (self.row_color(row.kind), t.background)
                };
                let mut line = div()
                    .id(("row", ix))
                    .flex()
                    .flex_row()
                    .h(px(self.row_height()))
                    .items_center()
                    .px(px(self.font_px * 0.6))
                    .bg(rgb(bg))
                    .text_color(rgb(fg));
                for (i, ((col, cell), width)) in self
                    .data
                    .columns
                    .iter()
                    .zip(&row.cells)
                    .zip(&self.col_px)
                    .enumerate()
                {
                    let Some(width) = width else { continue };
                    let mut c = div()
                        .w(px(*width))
                        .flex_none()
                        .pr(px(self.char_px))
                        .overflow_hidden()
                        .whitespace_nowrap();
                    c = if i == 0 && !row.spans.is_empty() {
                        // Keep the selection readable: no tone colours on the cursor row.
                        c.flex().flex_row().children(row.spans.iter().map(|span| {
                            let color = if selected { fg } else { t.tone(span.tone) };
                            div().text_color(rgb(color)).child(span.text.clone())
                        }))
                    } else {
                        c.child(cell.clone())
                    };
                    if col.right_align {
                        c = c.text_right();
                    }
                    line = line.child(c);
                }
                line
            })
            .collect()
    }
}

impl EventEmitter<Open> for DataTable {}

impl Focusable for DataTable {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DataTable {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        self.measure_char(window);
        let char_px = self.char_px;
        let visible: Vec<usize> = self.visible_columns().collect();
        self.col_px = self
            .source
            .column_widths()
            .iter()
            .enumerate()
            .map(|(i, w)| visible.contains(&i).then_some(*w as f32 * char_px))
            .collect();
        let total_px: f32 = self.col_px.iter().flatten().sum::<f32>() + self.font_px * 1.2;
        let mut head = div()
            .id("table-header")
            .flex()
            .flex_row()
            .h(px(self.row_height()))
            .items_center()
            .px(px(self.font_px * 0.6))
            .text_color(rgb(t.header));
        for (i, (col, width)) in self.data.columns.iter().zip(&self.col_px).enumerate() {
            let Some(width) = width else { continue };
            let arrow = match self.sort {
                Some(k) if k.column == i => {
                    if k.ascending {
                        "↑"
                    } else {
                        "↓"
                    }
                }
                _ => "",
            };
            let mut c = div()
                .w(px(*width))
                .flex_none()
                .pr(px(char_px))
                .overflow_hidden()
                .whitespace_nowrap()
                .child(format!("{}{arrow}", col.name));
            if col.right_align {
                c = c.text_right();
            }
            head = head.child(c);
        }
        let count = self.data.rows.len();
        div()
            .id("data-table")
            .key_context("DataTable")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .font_family(FONT)
            .text_size(px(self.font_px))
            .on_action(cx.listener(|this, _: &MoveUp, _, cx| {
                this.select(this.selected.saturating_sub(1), cx)
            }))
            .on_action(cx.listener(|this, _: &MoveDown, _, cx| this.select(this.selected + 1, cx)))
            .on_action(cx.listener(|this, _: &MoveTop, _, cx| this.select(0, cx)))
            .on_action(cx.listener(|this, _: &MoveBottom, _, cx| {
                this.select(this.data.rows.len().saturating_sub(1), cx)
            }))
            .on_action(cx.listener(|this, _: &PageUp, _, cx| {
                this.select(this.selected.saturating_sub(PAGE), cx)
            }))
            .on_action(
                cx.listener(|this, _: &PageDown, _, cx| this.select(this.selected + PAGE, cx)),
            )
            .on_action(cx.listener(|this, _: &Confirm, _, cx| {
                if let Some(id) = this.selected_row_id() {
                    cx.emit(Open(id.to_owned()));
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleWide, _, cx| {
                this.wide = !this.wide;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleFaults, _, cx| {
                this.faults = !this.faults;
                this.rebuild(cx);
            }))
            .on_action(cx.listener(|this, _: &SortName, _, cx| this.sort_by_header("NAME", cx)))
            .on_action(cx.listener(|this, _: &SortAge, _, cx| this.sort_by_header("AGE", cx)))
            .on_action(cx.listener(|this, _: &SortStatus, _, cx| this.sort_by_header("STATUS", cx)))
            .child(
                div()
                    .id("table-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_x_scroll()
                    .child(
                        div()
                            .w(px(total_px))
                            .min_w_full()
                            .h_full()
                            .flex()
                            .flex_col()
                            .child(head)
                            .child(
                                uniform_list(
                                    "table-rows",
                                    count,
                                    cx.processor(|this, range: Range<usize>, _, _| {
                                        this.render_rows(range)
                                    }),
                                )
                                .track_scroll(&self.scroll)
                                .flex_1(),
                            ),
                    ),
            )
    }
}
