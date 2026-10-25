use async_channel::Receiver;
use gpui::KeyDownEvent;
use gpui::{
    App, ClipboardItem, Context, Entity, FocusHandle, Focusable, IntoElement, KeyBinding, Render,
    Subscription, Task, Window, actions, div, prelude::*, px, rgb,
};
use k9s_app::{Action, ClusterInfo, ClusterStats, Kind, Options, TextKind, Update};
use k9s_components::{DEFAULT_FONT_PX, DataTable, FONT, Open, data_table_bindings, theme::Theme};
use k9s_core::{Column, Row, RowKind, Span, TableData, Tone};

actions!(
    k9s,
    [
        Quit,
        StartFilter,
        StartCommand,
        ShowHelp,
        ShowYaml,
        ShowDescribe,
        ShowLogs,
        ShowPreviousLogs,
        DeletePod,
        ToggleJson,
        ToggleWrap,
        CopyLine,
        CopyAll,
        ClearFilter,
        ZoomIn,
        ZoomOut,
        ZoomReset
    ]
);

const MIN_FONT_PX: f32 = 9.0;
const MAX_FONT_PX: f32 = 28.0;

type Starter = Box<dyn Fn(Options) -> Receiver<Update>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Prompt {
    Filter,
    Command,
    ConfirmDelete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PickerKind {
    Context,
    Namespace,
    Container,
    Text,
}

/// A selectable list (contexts, namespaces) shown in place of the pod table.
struct Picker {
    kind: PickerKind,
    table: Entity<DataTable>,
    _open: Subscription,
}

/// Pod-name (or node) regex that selects the pods owned by the resource `id` (`ns/name`).
fn drill_filter(kind: Kind, id: &str) -> Option<String> {
    let name = regex_escape(id.split_once('/')?.1);
    match kind {
        Kind::Deployment => Some(format!("^{name}-[^-]+-[^-]+$")),
        Kind::StatefulSet | Kind::DaemonSet | Kind::ReplicaSet | Kind::Job => {
            Some(format!("^{name}-[^-]+$"))
        }
        Kind::Node => Some(format!("^{name}$")),
        _ => None,
    }
}

fn regex_escape(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            let escape = !c.is_alphanumeric() && c != '-' && c != '_';
            escape.then_some('\\').into_iter().chain([c])
        })
        .collect()
}

fn picker_data(title: &str, items: &[String], current: &str) -> TableData {
    TableData {
        columns: vec![Column::new(
            title,
            if title.is_empty() { 4_000 } else { 60 },
        )],
        rows: items
            .iter()
            .map(|name| Row {
                id: name.clone(),
                cells: vec![name.replace('\t', "    ")],
                kind: if name == current {
                    RowKind::Completed
                } else {
                    RowKind::Standard
                },
                age: None,
                fault: false,
                spans: Vec::new(),
            })
            .collect(),
    }
}

const MAX_LOG_LINES: usize = 10_000;
const MAX_FAVOURITES: usize = 10;
const HEADER_ROWS: usize = 7;

/// Key hints shown in the header, mirroring k9s' menu.
const HINTS: &[(&str, &str)] = &[
    ("?", "Help"),
    ("y", "YAML"),
    ("d", "Describe"),
    ("l", "Logs"),
    ("p", "Previous Logs"),
    ("ctrl-d", "Delete"),
    (":", "Command"),
    ("/", "Filter"),
    ("esc", "Back/Clear"),
    ("ctrl-w", "Wide"),
    ("ctrl-z", "Faults"),
    ("q", "Quit"),
    ("shift-n", "Sort Name"),
    ("shift-a", "Sort Age"),
    ("shift-s", "Sort Status"),
    ("ctrl-=", "Zoom In"),
    ("ctrl--", "Zoom Out"),
    ("ctrl-0", "Zoom Reset"),
];

const PICKER_HINTS: &[(&str, &str)] = &[
    ("enter", "Select"),
    ("/", "Filter"),
    ("j/k", "Move"),
    ("esc", "Back"),
    ("?", "Help"),
];

const CONTAINER_HINTS: &[(&str, &str)] = &[
    ("enter", "Logs"),
    ("l", "Logs"),
    ("p", "Previous Logs"),
    ("/", "Filter"),
    ("j/k", "Move"),
    ("esc", "Back"),
    ("?", "Help"),
];

const TEXT_HINTS: &[(&str, &str)] = &[
    ("/", "Search"),
    ("n/shift-n", "Next/Prev Match"),
    ("j/k", "Scroll"),
    ("g", "Top"),
    ("shift-g", "Bottom"),
    ("esc", "Back"),
    ("?", "Help"),
];

const LOG_HINTS: &[(&str, &str)] = &[
    ("t", "JSON/Raw"),
    ("w", "Wrap"),
    ("c", "Copy"),
    ("shift-c", "Copy All"),
    ("/", "Filter"),
    ("j/k", "Scroll"),
    ("g", "Top"),
    ("shift-g", "Follow"),
    ("ctrl-=", "Zoom In"),
    ("ctrl--", "Zoom Out"),
    ("esc", "Back"),
    ("?", "Help"),
];

const HELP: &[(&str, &str)] = &[
    ("j / k / ↑ / ↓", "move selection"),
    ("g / G", "top / bottom"),
    ("ctrl-f / ctrl-b", "page down / up"),
    ("/", "filter (regex, `!` inverts)"),
    (
        ":",
        "command: ns, ctx, q, or a kind: po deploy sts ds rs job cj svc ing cm sec pvc ev no",
    ),
    ("shift-n / a / s", "sort by name / age / status"),
    (
        "y / d / l / p",
        "YAML / describe / logs / previous logs (pods)",
    ),
    ("ctrl-w", "toggle wide columns"),
    ("ctrl-z", "toggle faults only"),
    ("ctrl-= / ctrl-- / ctrl-0", "zoom in / out / reset"),
    ("t", "logs: toggle JSON rendering / raw"),
    ("w", "logs: toggle line wrap"),
    (
        "c / shift-c",
        "YAML/describe/logs: copy line / all visible lines",
    ),
    ("ctrl-d", "delete resource (needs --write; asks to confirm)"),
    ("esc", "close help or picker, clear filter"),
    ("0-9", "switch to favourite namespace"),
    ("q / ctrl-c", "quit"),
];

fn log_data(lines: &[String], pretty: bool, wrap: Option<usize>) -> TableData {
    let clean: Vec<String> = lines.iter().map(|l| k9s_render::log::strip(l)).collect();
    let mut data = picker_data("", &clean, "");
    if pretty {
        for ((row, line), raw) in data.rows.iter_mut().zip(&clean).zip(lines) {
            row.spans = k9s_render::log::render(line)
                .or_else(|| k9s_render::log::ansi(raw))
                .unwrap_or_default();
        }
    }
    let Some(width) = wrap.map(|w| w.max(20)) else {
        return data;
    };
    let mut rows = Vec::with_capacity(data.rows.len());
    for row in data.rows {
        let spans = if row.spans.is_empty() {
            vec![Span::new(
                row.cells.first().cloned().unwrap_or_default(),
                Tone::Plain,
            )]
        } else {
            row.spans.clone()
        };
        let plain = row.spans.is_empty();
        for chunk in k9s_render::log::wrap(&spans, width) {
            let mut r = row.clone();
            r.cells = vec![chunk.iter().map(|s| s.text.as_str()).collect()];
            r.spans = if plain { Vec::new() } else { chunk };
            rows.push(r);
        }
    }
    data.rows = rows;
    data
}

pub fn key_bindings() -> Vec<KeyBinding> {
    let mut keys = vec![
        KeyBinding::new("ctrl-c", Quit, Some("Workspace")),
        KeyBinding::new("q", Quit, Some("DataTable")),
        KeyBinding::new("/", StartFilter, Some("DataTable")),
        KeyBinding::new(":", StartCommand, Some("DataTable")),
        KeyBinding::new("y", ShowYaml, Some("DataTable")),
        KeyBinding::new("d", ShowDescribe, Some("DataTable")),
        KeyBinding::new("l", ShowLogs, Some("DataTable")),
        KeyBinding::new("p", ShowPreviousLogs, Some("DataTable")),
        KeyBinding::new("ctrl-d", DeletePod, Some("DataTable")),
        KeyBinding::new("t", ToggleJson, Some("DataTable")),
        KeyBinding::new("w", ToggleWrap, Some("DataTable")),
        KeyBinding::new("c", CopyLine, Some("DataTable")),
        KeyBinding::new("shift-c", CopyAll, Some("DataTable")),
        KeyBinding::new("?", ShowHelp, Some("DataTable")),
        KeyBinding::new("escape", ClearFilter, Some("DataTable")),
        KeyBinding::new("ctrl-=", ZoomIn, Some("Workspace")),
        KeyBinding::new("ctrl-+", ZoomIn, Some("Workspace")),
        KeyBinding::new("ctrl--", ZoomOut, Some("Workspace")),
        KeyBinding::new("ctrl-0", ZoomReset, Some("Workspace")),
    ];
    keys.extend(data_table_bindings());
    keys
}

pub struct Workspace {
    theme: Theme,
    table: Entity<DataTable>,
    info: ClusterInfo,
    flash: Option<String>,
    prompt: Option<Prompt>,
    picker: Option<Picker>,
    help: bool,
    container_pod: Option<(String, String)>,
    container_data: Option<TableData>,
    log_lines: Vec<String>,
    log_pretty: bool,
    log_wrap: Option<usize>,
    streaming: bool,
    read_only: bool,
    pending_delete: Option<(String, String)>,
    _open_pod: Subscription,
    text_title: String,
    stats: ClusterStats,
    favourites: Vec<String>,
    history: Vec<String>,
    history_pos: Option<usize>,
    _picker_load: Option<Task<()>>,
    input: String,
    prompt_focus: FocusHandle,
    font_px: f32,
    options: Options,
    starter: Option<Starter>,
    _updates: Task<()>,
}

impl Workspace {
    pub fn new(updates: Receiver<Update>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let theme = Theme::default();
        let table = cx.new(|cx| DataTable::new(TableData::default(), theme, cx));
        let open_pod = cx.subscribe_in(&table, window, |this, _, Open(id): &Open, window, cx| {
            if this.picker.is_none() {
                match this.options.kind {
                    Kind::Pod => this.begin_logs(None, window, cx),
                    kind => match drill_filter(kind, id) {
                        Some(filter) => this.show_pods(&filter, cx),
                        None => this.open_text(TextKind::Yaml, window, cx),
                    },
                }
            }
        });
        Self {
            _open_pod: open_pod,
            theme,
            table,
            info: ClusterInfo::default(),
            flash: Some("connecting…".into()),
            prompt: None,
            picker: None,
            help: false,
            container_pod: None,
            container_data: None,
            log_lines: Vec::new(),
            log_pretty: true,
            log_wrap: None,
            streaming: false,
            read_only: true,
            pending_delete: None,
            text_title: String::new(),
            stats: ClusterStats::default(),
            favourites: vec!["all".to_owned()],
            history: Vec::new(),
            history_pos: None,
            _picker_load: None,
            input: String::new(),
            prompt_focus: cx.focus_handle(),
            font_px: DEFAULT_FONT_PX,
            options: Options::default(),
            starter: None,
            _updates: Self::listen(updates, cx),
        }
    }

    /// Mutating actions (delete, ...) are refused while read-only, the default.
    pub fn with_read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// Enables `:ns` / `:ctx` by letting the workspace restart the cluster worker.
    pub fn with_starter(
        mut self,
        options: Options,
        starter: impl Fn(Options) -> Receiver<Update> + 'static,
    ) -> Self {
        self.options = options;
        self.starter = Some(Box::new(starter));
        self
    }

    fn listen(updates: Receiver<Update>, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            while let Ok(update) = updates.recv().await {
                if this.update(cx, |this, cx| this.apply(update, cx)).is_err() {
                    break;
                }
            }
        })
    }

    fn active_table(&self) -> &Entity<DataTable> {
        self.picker.as_ref().map_or(&self.table, |p| &p.table)
    }

    fn open_picker(
        &mut self,
        kind: PickerKind,
        items: &[String],
        current: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = match kind {
            PickerKind::Context => "CONTEXT",
            PickerKind::Namespace => "NAMESPACE",
            PickerKind::Container | PickerKind::Text => "",
        };
        self.open_picker_data(kind, picker_data(title, items, current), window, cx);
    }

    fn open_container_view(
        &mut self,
        data: TableData,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.container_data = Some(data.clone());
        self.open_picker_data(PickerKind::Container, data, window, cx);
    }

    fn open_picker_data(
        &mut self,
        kind: PickerKind,
        data: TableData,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.streaming = false;
        let theme = self.theme;
        let font_px = self.font_px;
        let table = cx.new(|cx| {
            let mut t = DataTable::new(data, theme, cx);
            t.set_font_px(font_px, cx);
            t
        });
        let open = cx.subscribe_in(&table, window, |this, _, Open(id): &Open, window, cx| {
            let Some(kind) = this.picker.as_ref().map(|p| p.kind) else {
                return;
            };
            let command = match kind {
                PickerKind::Text => return,
                PickerKind::Container => {
                    this.picker = None;
                    if let Some((ns, name)) = this.container_pod.clone() {
                        this.start_stream(ns, name, id.clone(), false, window, cx);
                    }
                    return;
                }
                PickerKind::Context => format!("ctx {id}"),
                PickerKind::Namespace => format!("ns {id}"),
            };
            this.picker = None;
            this.run_command(&command, cx);
            cx.notify();
        });
        window.focus(&table.focus_handle(cx), cx);
        self.picker = Some(Picker {
            kind,
            table,
            _open: open,
        });
        cx.notify();
    }

    fn open_text(&mut self, kind: TextKind, window: &mut Window, cx: &mut Context<Self>) {
        let Some((namespace, name)) = self
            .table
            .read(cx)
            .selected_row_id()
            .and_then(|id| id.split_once('/'))
            .map(|(ns, n)| (ns.to_owned(), n.to_owned()))
        else {
            return;
        };
        let title = match kind {
            TextKind::Yaml => "yaml",
            TextKind::Describe => "describe",
        };
        self.text_title = format!("{title}({namespace}/{name})");
        self.flash = Some(format!("loading {title}…"));
        let rx = k9s_app::fetch_text(
            self.options.clone(),
            kind,
            self.options.kind,
            namespace,
            name,
        );
        let window_handle = window.window_handle();
        self._picker_load = Some(cx.spawn(async move |this, cx| {
            let result = rx.recv().await;
            let _ = window_handle.update(cx, |_, window, cx| {
                let _ = this.update(cx, |this, cx| match result {
                    Ok(Ok(text)) => {
                        this.flash = None;
                        let lines: Vec<String> = text.lines().map(str::to_owned).collect();
                        this.open_picker(PickerKind::Text, &lines, "", window, cx);
                        if let Some(p) = &this.picker {
                            p.table.update(cx, |t, cx| t.set_search_mode(true, cx));
                        }
                    }
                    Ok(Err(e)) => {
                        this.flash = Some(e);
                        cx.notify();
                    }
                    Err(_) => {}
                });
            });
        }));
    }

    /// In the container view: ((namespace, container), pod name) of the selected container.
    fn container_logs_target(&self, cx: &Context<Self>) -> Option<((String, String), String)> {
        let picker = self
            .picker
            .as_ref()
            .filter(|p| p.kind == PickerKind::Container)?;
        let (ns, pod) = self.container_pod.clone()?;
        let container = picker.table.read(cx).selected_row_id()?.to_owned();
        Some(((ns, container), pod))
    }

    fn request_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            self.flash = Some("read-only mode: start with --write to change the cluster".into());
            cx.notify();
            return;
        }
        if let Some(pod) = self.selected_pod(cx) {
            self.pending_delete = Some(pod);
            self.open_prompt(Prompt::ConfirmDelete, window, cx);
        }
    }

    fn delete_selected(&mut self, namespace: String, name: String, cx: &mut Context<Self>) {
        self.flash = Some(format!("deleting {namespace}/{name}…"));
        let rx = k9s_app::delete_resource(
            self.options.clone(),
            self.options.kind,
            namespace.clone(),
            name.clone(),
        );
        self._picker_load = Some(cx.spawn(async move |this, cx| {
            let result = rx.recv().await;
            let _ = this.update(cx, |this, cx| {
                this.flash = Some(match result {
                    Ok(Ok(())) => format!("deleted {namespace}/{name}"),
                    Ok(Err(e)) => e,
                    Err(_) => "delete failed".into(),
                });
                cx.notify();
            });
        }));
    }

    fn selected_pod(&self, cx: &Context<Self>) -> Option<(String, String)> {
        let id = self.table.read(cx).selected_row_id()?;
        let (ns, name) = id.split_once('/')?;
        Some((ns.to_owned(), name.to_owned()))
    }

    /// Opens the container view of the selected pod; with `Some(previous)` a
    /// single-container pod skips straight to its logs.
    fn begin_logs(&mut self, direct: Option<bool>, window: &mut Window, cx: &mut Context<Self>) {
        if self.options.kind != Kind::Pod {
            return;
        }
        let Some((namespace, name)) = self.selected_pod(cx) else {
            return;
        };
        self.flash = Some("loading containers…".into());
        let rx = k9s_app::list_containers(self.options.clone(), namespace.clone(), name.clone());
        let window_handle = window.window_handle();
        self._picker_load = Some(cx.spawn(async move |this, cx| {
            let result = rx.recv().await;
            let _ = window_handle.update(cx, |_, window, cx| {
                let _ = this.update(cx, |this, cx| match result {
                    Ok(Ok(data)) => {
                        this.flash = None;
                        match (direct, data.rows.as_slice()) {
                            (Some(previous), [only]) => {
                                this.container_data = None;
                                let container = only.id.clone();
                                this.start_stream(namespace, name, container, previous, window, cx);
                            }
                            _ => {
                                this.container_pod = Some((namespace, name));
                                this.open_container_view(data, window, cx);
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        this.flash = Some(e);
                        cx.notify();
                    }
                    Err(_) => {}
                });
            });
        }));
    }

    fn start_stream(
        &mut self,
        namespace: String,
        name: String,
        container: String,
        previous: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let suffix = if previous { " previous" } else { "" };
        self.text_title = format!("logs({namespace}/{name}:{container}{suffix})");
        let rx = k9s_app::stream_logs(self.options.clone(), namespace, name, container, previous);
        self.log_lines.clear();
        self.open_picker(PickerKind::Text, &[], "", window, cx);
        self.streaming = true;
        self._picker_load = Some(cx.spawn(async move |this, cx| {
            while let Ok(first) = rx.recv().await {
                let mut batch = vec![first];
                while batch.len() < 500
                    && let Ok(more) = rx.try_recv()
                {
                    batch.push(more);
                }
                let alive = this.update(cx, |this, cx| {
                    if !this.streaming
                        || this
                            .picker
                            .as_ref()
                            .is_none_or(|p| p.kind != PickerKind::Text)
                    {
                        return false;
                    }
                    let mut failed = false;
                    for item in batch {
                        match item {
                            Ok(line) => this.log_lines.push(line),
                            Err(e) => {
                                this.flash = Some(e);
                                failed = true;
                            }
                        }
                    }
                    if this.log_lines.len() > MAX_LOG_LINES {
                        let extra = this.log_lines.len() - MAX_LOG_LINES;
                        this.log_lines.drain(..extra);
                    }
                    this.refresh_log(cx);
                    !failed
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        }));
    }

    fn picker_table(&self) -> Option<Entity<DataTable>> {
        self.picker
            .as_ref()
            .filter(|p| p.kind == PickerKind::Text)
            .map(|p| p.table.clone())
    }

    fn copy(&mut self, text: Option<String>, what: &str, cx: &mut Context<Self>) {
        if let Some(text) = text.filter(|t| !t.is_empty()) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.flash = Some(format!("copied {what}"));
            cx.notify();
        }
    }

    fn refresh_log(&mut self, cx: &mut Context<Self>) {
        let data = log_data(&self.log_lines, self.log_pretty, self.log_wrap);
        if let Some(p) = self.picker.as_ref().filter(|p| p.kind == PickerKind::Text) {
            p.table.update(cx, |t, cx| t.set_data_follow(data, cx));
        }
        cx.notify();
    }

    /// Key hints for the current view, like k9s' per-view menu.
    fn hints(&self) -> &'static [(&'static str, &'static str)] {
        match self.picker.as_ref().map(|p| p.kind) {
            None => HINTS,
            Some(PickerKind::Context | PickerKind::Namespace) => PICKER_HINTS,
            Some(PickerKind::Container) => CONTAINER_HINTS,
            Some(PickerKind::Text) if self.streaming => LOG_HINTS,
            Some(PickerKind::Text) => TEXT_HINTS,
        }
    }

    fn open_namespaces(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.flash = Some("loading namespaces…".into());
        let rx = k9s_app::list_namespaces(self.options.clone());
        let window_handle = window.window_handle();
        self._picker_load = Some(cx.spawn(async move |this, cx| {
            let result = rx.recv().await;
            let _ = window_handle.update(cx, |_, window, cx| {
                let _ = this.update(cx, |this, cx| match result {
                    Ok(Ok(mut names)) => {
                        this.flash = None;
                        names.insert(0, "all".into());
                        let current = this.info.namespace.clone();
                        this.open_picker(PickerKind::Namespace, &names, &current, window, cx);
                    }
                    Ok(Err(e)) => {
                        this.flash = Some(e);
                        cx.notify();
                    }
                    Err(_) => {}
                });
            });
        }));
    }

    fn set_font_px(&mut self, px: f32, cx: &mut Context<Self>) {
        self.font_px = px.clamp(MIN_FONT_PX, MAX_FONT_PX);
        let font_px = self.font_px;
        self.table.update(cx, |t, cx| t.set_font_px(font_px, cx));
        if let Some(p) = &self.picker {
            p.table.update(cx, |t, cx| t.set_font_px(font_px, cx));
        }
        cx.notify();
    }

    fn open_prompt(&mut self, prompt: Prompt, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt = Some(prompt);
        self.history_pos = None;
        self.input = match prompt {
            Prompt::Filter => self.active_table().read(cx).filter().to_owned(),
            Prompt::Command | Prompt::ConfirmDelete => String::new(),
        };
        window.focus(&self.prompt_focus, cx);
        cx.notify();
    }

    fn close_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt = None;
        self.input.clear();
        window.focus(&self.active_table().focus_handle(cx), cx);
        cx.notify();
    }

    fn on_prompt_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        if self.prompt == Some(Prompt::ConfirmDelete) {
            let confirmed = key.key == "y";
            self.close_prompt(window, cx);
            if let (true, Some((ns, name))) = (confirmed, self.pending_delete.take()) {
                self.delete_selected(ns, name, cx);
            }
            self.pending_delete = None;
            return;
        }
        match key.key.as_str() {
            "escape" => {
                if self.prompt == Some(Prompt::Filter) {
                    self.active_table()
                        .clone()
                        .update(cx, |t, cx| t.set_filter("", cx));
                }
                self.close_prompt(window, cx);
            }
            "enter" => {
                let (prompt, text) = (self.prompt, std::mem::take(&mut self.input));
                self.close_prompt(window, cx);
                if prompt == Some(Prompt::Command) {
                    self.picker = None;
                    let entry = text.trim();
                    if !entry.is_empty() && self.history.last().map(String::as_str) != Some(entry) {
                        self.history.push(entry.to_owned());
                    }
                    match text.split_whitespace().collect::<Vec<_>>().as_slice() {
                        ["ctx" | "context"] => {
                            let (names, current) = k9s_app::list_contexts();
                            let current =
                                self.info.context.clone().max(current.unwrap_or_default());
                            self.open_picker(PickerKind::Context, &names, &current, window, cx);
                        }
                        ["ns" | "namespace"] => self.open_namespaces(window, cx),
                        _ => self.run_command(&text, cx),
                    }
                }
            }
            "up" | "down" if self.prompt == Some(Prompt::Command) => {
                let len = self.history.len();
                let next = match (key.key.as_str(), self.history_pos) {
                    ("up", None) => len.checked_sub(1),
                    ("up", Some(i)) => Some(i.saturating_sub(1)),
                    (_, Some(i)) if i + 1 < len => Some(i + 1),
                    _ => None,
                };
                self.history_pos = next;
                self.input = next
                    .and_then(|i| self.history.get(i))
                    .cloned()
                    .unwrap_or_default();
            }
            "backspace" => {
                self.input.pop();
                self.live_filter(cx);
            }
            _ => {
                if let Some(ch) = key.key_char.as_deref().filter(|_| !key.modifiers.control) {
                    self.input.push_str(ch);
                    self.live_filter(cx);
                }
            }
        }
        cx.notify();
    }

    fn live_filter(&mut self, cx: &mut Context<Self>) {
        if self.prompt == Some(Prompt::Filter) {
            let text = self.input.clone();
            self.active_table()
                .clone()
                .update(cx, |t, cx| t.set_filter(&text, cx));
        }
    }

    fn run_action(&mut self, action: Action, cx: &mut Context<Self>) {
        let kind = self.options.kind;
        if self.read_only {
            self.flash = Some("read-only mode: start with --write to change the cluster".into());
        } else if !action.supports(kind) {
            self.flash = Some(format!("{action:?} does not apply to {}", kind.title()));
        } else if let Some((ns, name)) = self.selected_pod(cx) {
            self.flash = Some(format!("{action:?} {name}…"));
            let rx = k9s_app::run_action(self.options.clone(), kind, ns, name, action);
            self._picker_load = Some(cx.spawn(async move |this, cx| {
                let result = rx.recv().await;
                let _ = this.update(cx, |this, cx| {
                    this.flash = Some(match result {
                        Ok(Ok(message)) => message,
                        Ok(Err(e)) => e,
                        Err(_) => "action failed".into(),
                    });
                    cx.notify();
                });
            }));
        }
        cx.notify();
    }

    /// Switches to the pod table narrowed by `filter` (drill-down from a workload or node).
    fn show_pods(&mut self, filter: &str, cx: &mut Context<Self>) {
        self.run_command("po", cx);
        self.table.update(cx, |t, cx| t.set_filter(filter, cx));
    }

    fn run_command(&mut self, text: &str, cx: &mut Context<Self>) {
        let mut parts = text.split_whitespace();
        let mut options = self.options.clone();
        match (parts.next(), parts.next()) {
            (None, _) => return,
            (Some(cmd @ ("scale" | "restart" | "cordon" | "uncordon")), arg) => {
                let action = match (cmd, arg.map(str::parse::<i32>)) {
                    ("scale", Some(Ok(n))) if n >= 0 => Action::Scale(n),
                    ("scale", _) => {
                        self.flash = Some("usage: scale <replicas>".into());
                        return;
                    }
                    ("restart", _) => Action::Restart,
                    ("cordon", _) => Action::Cordon(true),
                    _ => Action::Cordon(false),
                };
                return self.run_action(action, cx);
            }
            (Some(name), None) if Kind::parse(name).is_some() => {
                options.kind = Kind::parse(name).unwrap_or_default();
            }
            (Some("q" | "q!" | "quit"), _) => return cx.quit(),
            (Some("ns" | "namespace"), Some("all" | "-A")) => {
                options.all_namespaces = true;
                options.namespace = None;
            }
            (Some("ns" | "namespace"), Some(ns)) => {
                options.all_namespaces = false;
                options.namespace = Some(ns.to_owned());
            }
            (Some("ctx" | "context"), Some(ctx)) => {
                options.context = Some(ctx.to_owned());
                options.namespace = None;
            }
            _ => {
                self.flash = Some(format!("unknown command: {text}"));
                return;
            }
        }
        let Some(starter) = &self.starter else {
            self.flash = Some("switching is unavailable".into());
            return;
        };
        let updates = starter(options.clone());
        if options.kind != self.options.kind {
            self.table.update(cx, |t, cx| t.set_filter("", cx));
        }
        self.options = options;
        self.flash = None;
        self.table
            .update(cx, |t, cx| t.set_data(TableData::default(), cx));
        self._updates = Self::listen(updates, cx);
    }

    fn apply(&mut self, update: Update, cx: &mut Context<Self>) {
        match update {
            Update::Info(info) => {
                if !info.all_namespaces
                    && !info.namespace.is_empty()
                    && !self.favourites.contains(&info.namespace)
                {
                    self.favourites.truncate(MAX_FAVOURITES - 1);
                    self.favourites.push(info.namespace.clone());
                }
                self.info = info;
            }
            Update::Stats(stats) => self.stats = stats,
            Update::Table(data) => {
                self.flash = None;
                self.table.update(cx, |t, cx| t.set_data(data, cx));
            }
            Update::Error(message) => self.flash = Some(message),
        }
        cx.notify();
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.table.focus_handle(cx)
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focus = self.active_table().focus_handle(cx);
        if !focus.contains_focused(window, cx) && !self.prompt_focus.is_focused(window) {
            window.focus(&focus, cx);
        }
        let rows = self.active_table().read(cx).row_count();
        let t = self.theme;
        let em = self.font_px;
        let filter = self.active_table().read(cx).filter().to_owned();
        let mut crumb = if let Some(p) = &self.picker {
            match p.kind {
                PickerKind::Context => format!("contexts({rows})"),
                PickerKind::Namespace => format!("namespaces({rows})"),
                PickerKind::Container => format!("containers({rows})"),
                PickerKind::Text => format!("{}({rows})", self.text_title),
            }
        } else if self.info.all_namespaces {
            format!("{}({rows})", self.options.kind.title())
        } else if self.options.kind.namespaced() {
            format!(
                "{}({}) [{rows}]",
                self.options.kind.title(),
                self.info.namespace
            )
        } else {
            format!("{}({rows})", self.options.kind.title())
        };
        let (wide, faults) = {
            let t = self.table.read(cx);
            (t.wide(), t.faults())
        };
        if faults {
            crumb.push_str(" <faults>");
        }
        if wide {
            crumb.push_str(" <wide>");
        }
        if !filter.is_empty() {
            crumb.push_str(&format!(" </{filter}>"));
            if let Some((at, total)) = self.active_table().read(cx).match_status() {
                crumb.push_str(&format!(" [{at}/{total}]"));
            }
        }
        let prompt_text = self.prompt.map(|p| match p {
            Prompt::Filter => format!("/{}▏", self.input),
            Prompt::Command => format!(":{}▏", self.input),
            Prompt::ConfirmDelete => {
                let (ns, name) = self.pending_delete.clone().unwrap_or_default();
                format!("Delete {} {ns}/{name}? [y/N]", self.options.kind.title())
            }
        });
        let label = |name: &'static str, value: String| {
            div()
                .flex()
                .flex_row()
                .child(div().w(px(em * 6.0)).text_color(rgb(t.accent)).child(name))
                .child(div().text_color(rgb(t.header)).child(value))
        };
        let pct = |v: Option<u32>| v.map_or_else(|| "n/a".to_owned(), |v| format!("{v}%"));
        let current_ns = if self.info.all_namespaces {
            "all"
        } else {
            self.info.namespace.as_str()
        };
        let favourites = div().flex().flex_col().min_w(px(em * 14.0)).children(
            self.favourites.iter().enumerate().map(|(i, ns)| {
                let active = ns == current_ns;
                div()
                    .flex()
                    .flex_row()
                    .gap(px(em * 0.5))
                    .child(
                        div()
                            .w(px(em * 2.4))
                            .text_color(rgb(t.accent))
                            .child(format!("<{i}>")),
                    )
                    .child(
                        div()
                            .text_color(rgb(if active { t.header } else { t.foreground }))
                            .child(ns.clone()),
                    )
            }),
        );
        let hints = div().flex().flex_row().gap(px(em * 1.5)).children(
            self.hints().chunks(HEADER_ROWS).map(|chunk| {
                div()
                    .flex()
                    .flex_col()
                    .children(chunk.iter().map(|(key, what)| {
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(em * 0.5))
                            .child(
                                div()
                                    .w(px(em * 8.5))
                                    .text_color(rgb(t.accent))
                                    .child(format!("<{key}>")),
                            )
                            .child(*what)
                    }))
            }),
        );
        div()
            .id("workspace")
            .key_context("Workspace")
            .on_action(|_: &Quit, _, cx| cx.quit())
            .on_action(cx.listener(|this, _: &StartFilter, window, cx| {
                this.open_prompt(Prompt::Filter, window, cx)
            }))
            .on_action(cx.listener(|this, _: &StartCommand, window, cx| {
                this.open_prompt(Prompt::Command, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ZoomIn, _, cx| this.set_font_px(this.font_px + 1.0, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ZoomOut, _, cx| this.set_font_px(this.font_px - 1.0, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ZoomReset, _, cx| this.set_font_px(DEFAULT_FONT_PX, cx)),
            )
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                let k = &e.keystroke;
                let plain = !k.modifiers.control && !k.modifiers.alt && !k.modifiers.platform;
                let digit = k.key.parse::<usize>().ok().filter(|_| k.key.len() == 1);
                if let (true, None, Some(i)) = (plain, this.prompt, digit)
                    && let Some(ns) = this.favourites.get(i).cloned()
                {
                    this.picker = None;
                    this.run_command(&format!("ns {ns}"), cx);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &ShowYaml, window, cx| {
                if this.picker.is_none() {
                    this.open_text(TextKind::Yaml, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ShowDescribe, window, cx| {
                if this.picker.is_none() {
                    this.open_text(TextKind::Describe, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ShowLogs, window, cx| {
                if this.picker.is_none() {
                    this.begin_logs(Some(false), window, cx);
                } else if let Some((ns, name)) = this.container_logs_target(cx) {
                    this.start_stream(ns.0, name, ns.1, false, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ShowPreviousLogs, window, cx| {
                if this.picker.is_none() {
                    this.begin_logs(Some(true), window, cx);
                } else if let Some((ns, name)) = this.container_logs_target(cx) {
                    this.start_stream(ns.0, name, ns.1, true, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &DeletePod, window, cx| {
                if this.picker.is_none() {
                    this.request_delete(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleJson, _, cx| {
                if this.streaming && this.picker.is_some() {
                    this.log_pretty = !this.log_pretty;
                    this.refresh_log(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleWrap, window, cx| {
                if this.streaming && this.picker.is_some() {
                    this.log_wrap = match this.log_wrap {
                        Some(_) => None,
                        None => {
                            let cell = f32::from(window.rem_size()) * 0.6;
                            let usable = f32::from(window.viewport_size().width) - 48.0;
                            Some((usable / cell.max(1.0)) as usize)
                        }
                    };
                    this.refresh_log(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &CopyLine, _, cx| {
                let text = this
                    .picker_table()
                    .and_then(|t| t.read(cx).selected_row_id().map(str::to_owned));
                this.copy(text, "line", cx);
            }))
            .on_action(cx.listener(|this, _: &CopyAll, _, cx| {
                let text = this
                    .picker_table()
                    .map(|t| t.read(cx).visible_ids().join("\n"));
                this.copy(text, "all lines", cx);
            }))
            .on_action(cx.listener(|this, _: &ShowHelp, _, cx| {
                this.help = true;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ClearFilter, window, cx| {
                if this.help {
                    this.help = false;
                    cx.notify();
                } else if let Some(kind) = this.picker.as_ref().map(|p| p.kind) {
                    this._picker_load = None;
                    this.streaming = false;
                    match (kind, this.container_data.take()) {
                        // Logs opened from the container view go back to it.
                        (PickerKind::Text, Some(data)) => {
                            this.open_container_view(data, window, cx)
                        }
                        _ => {
                            this.picker = None;
                            this.container_pod = None;
                            window.focus(&this.table.focus_handle(cx), cx);
                        }
                    }
                    cx.notify();
                } else {
                    this.table.update(cx, |t, cx| t.set_filter("", cx));
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            .font_family(FONT)
            .text_size(px(self.font_px))
            .bg(rgb(t.background))
            .text_color(rgb(t.foreground))
            .child(
                div()
                    .id("header")
                    .flex()
                    .flex_row()
                    .gap(px(em * 2.5))
                    .px(px(em * 0.9))
                    .py(px(em * 0.5))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w(px(em * 26.0))
                            .child(label(
                                "Context:",
                                format!(
                                    "{} {}",
                                    self.info.context,
                                    if self.read_only { "[RO]" } else { "[RW]" }
                                ),
                            ))
                            .child(label("Cluster:", self.info.cluster.clone()))
                            .child(label("User:", self.info.user.clone()))
                            .child(label("Rev:", env!("CARGO_PKG_VERSION").to_owned()))
                            .child(label("K8s Rev:", self.info.k8s_rev.clone()))
                            .child(label("CPU:", pct(self.stats.cpu)))
                            .child(label("MEM:", pct(self.stats.mem))),
                    )
                    .child(favourites)
                    .child(hints),
            )
            .when_some(prompt_text, |d, text| {
                d.child(
                    div()
                        .id("prompt")
                        .key_context("Prompt")
                        .track_focus(&self.prompt_focus)
                        .on_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                            this.on_prompt_key(e, window, cx)
                        }))
                        .px(px(em * 0.9))
                        .py(px(em * 0.3))
                        .border_1()
                        .border_color(rgb(t.border))
                        .text_color(rgb(t.header))
                        .child(text),
                )
            })
            .child(
                div()
                    .id("body")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .border_1()
                    .border_color(rgb(t.border))
                    .child(self.active_table().clone())
                    .when(self.help, |d| {
                        d.child(
                            div()
                                .absolute()
                                .inset_0()
                                .bg(rgb(t.background))
                                .p(px(em))
                                .flex()
                                .flex_col()
                                .gap(px(em * 0.3))
                                .children(HELP.iter().map(|(key, what)| {
                                    div()
                                        .flex()
                                        .flex_row()
                                        .child(
                                            div()
                                                .w(px(em * 14.0))
                                                .text_color(rgb(t.accent))
                                                .child(*key),
                                        )
                                        .child(*what)
                                })),
                        )
                    }),
            )
            .child(
                div()
                    .id("crumbs")
                    .px(px(em * 0.9))
                    .py(px(em * 0.3))
                    .bg(rgb(t.accent))
                    .text_color(rgb(t.background))
                    .child(crumb),
            )
            .child(
                div()
                    .id("flash")
                    .h(px(em * 1.7))
                    .px(px(em * 0.9))
                    .text_color(rgb(t.error))
                    .child(self.flash.clone().unwrap_or_default()),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};

    fn setup(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
        cx.update(|cx| cx.bind_keys(key_bindings()));
        let (tx, rx) = async_channel::unbounded();
        tx.send_blocking(Update::Table(crate::mock::pods(10_000)))
            .unwrap();
        // Keep the sender alive so the update task stays pending after the first message.
        std::mem::forget(tx);
        cx.add_window_view(|window, cx| Workspace::new(rx, window, cx))
    }

    #[gpui::test]
    fn navigation_keys_move_selection(cx: &mut TestAppContext) {
        let (ws, cx) = setup(cx);
        let table = ws.read_with(cx, |w, _| w.table.clone());
        cx.run_until_parked();
        cx.simulate_keystrokes("j j j");
        assert_eq!(table.read_with(cx, |t, _| t.selected()), 3);
        cx.simulate_keystrokes("k");
        assert_eq!(table.read_with(cx, |t, _| t.selected()), 2);
        cx.simulate_keystrokes("shift-g");
        assert_eq!(table.read_with(cx, |t, _| t.selected()), 9_999);
        cx.simulate_keystrokes("g");
        assert_eq!(table.read_with(cx, |t, _| t.selected()), 0);
    }

    #[gpui::test]
    fn filter_prompt_narrows_rows_and_escape_clears(cx: &mut TestAppContext) {
        let (ws, cx) = setup(cx);
        let table = ws.read_with(cx, |w, _| w.table.clone());
        cx.run_until_parked();
        cx.simulate_keystrokes("/");
        cx.simulate_input("app-12-");
        cx.run_until_parked();
        let rows = table.read_with(cx, |t, _| t.row_count());
        assert!(rows > 0 && rows < 10_000, "rows = {rows}");
        cx.simulate_keystrokes("escape");
        assert_eq!(table.read_with(cx, |t, _| t.row_count()), 10_000);
    }

    #[gpui::test]
    fn zoom_keys_scale_the_ui_within_bounds(cx: &mut TestAppContext) {
        let (ws, cx) = setup(cx);
        cx.run_until_parked();
        cx.simulate_keystrokes("ctrl-=");
        assert_eq!(ws.read_with(cx, |w, _| w.font_px), DEFAULT_FONT_PX + 1.0);
        cx.simulate_keystrokes("ctrl-0");
        assert_eq!(ws.read_with(cx, |w, _| w.font_px), DEFAULT_FONT_PX);
        for _ in 0..40 {
            cx.simulate_keystrokes("ctrl--");
        }
        assert_eq!(ws.read_with(cx, |w, _| w.font_px), MIN_FONT_PX);
    }

    #[gpui::test]
    fn ctrl_z_shows_only_faults_and_ctrl_w_toggles_wide(cx: &mut TestAppContext) {
        let (ws, cx) = setup(cx);
        let table = ws.read_with(cx, |w, _| w.table.clone());
        cx.run_until_parked();
        cx.simulate_keystrokes("ctrl-z");
        let rows = table.read_with(cx, |t, _| t.row_count());
        assert_eq!(rows, 2_000, "1 in 5 mock pods is in error");
        cx.simulate_keystrokes("ctrl-z");
        assert_eq!(table.read_with(cx, |t, _| t.row_count()), 10_000);
        cx.simulate_keystrokes("ctrl-w");
        assert!(table.read_with(cx, |t, _| t.wide()));
    }

    #[gpui::test]
    fn regex_filter_matches_alternatives(cx: &mut TestAppContext) {
        let (ws, cx) = setup(cx);
        let table = ws.read_with(cx, |w, _| w.table.clone());
        cx.run_until_parked();
        cx.simulate_keystrokes("/");
        cx.simulate_input("app-12-7d|app-13-7d");
        cx.run_until_parked();
        let rows = table.read_with(cx, |t, _| t.row_count());
        assert!(rows > 0 && rows < 10_000, "rows = {rows}");
        cx.simulate_keystrokes("escape");
        cx.simulate_keystrokes("/");
        cx.simulate_input("app-1(");
        cx.run_until_parked();
        assert_eq!(
            table.read_with(cx, |t, _| t.row_count()),
            0,
            "unclosed group is literal"
        );
    }

    #[gpui::test]
    fn picker_selects_with_enter_and_closes_with_escape(cx: &mut TestAppContext) {
        let (ws, cx) = setup(cx);
        cx.run_until_parked();
        ws.update_in(cx, |w, window, cx| {
            w.open_picker(
                PickerKind::Context,
                &["a".into(), "b".into()],
                "a",
                window,
                cx,
            );
        });
        cx.simulate_keystrokes("j");
        let picked = ws.read_with(cx, |w, cx| w.active_table().read(cx).selected());
        assert_eq!(picked, 1);
        cx.simulate_keystrokes("escape");
        assert!(ws.read_with(cx, |w, _| w.picker.is_none()));
        ws.update_in(cx, |w, window, cx| {
            w.open_picker(PickerKind::Context, &["a".into()], "a", window, cx);
        });
        cx.simulate_keystrokes("enter");
        assert!(ws.read_with(cx, |w, _| w.picker.is_none()));
    }

    #[gpui::test]
    fn help_toggles_and_command_history_recalls(cx: &mut TestAppContext) {
        let (ws, cx) = setup(cx);
        cx.run_until_parked();
        cx.simulate_keystrokes("?");
        assert!(ws.read_with(cx, |w, _| w.help));
        cx.simulate_keystrokes("escape");
        assert!(!ws.read_with(cx, |w, _| w.help));
        cx.simulate_keystrokes(":");
        cx.simulate_input("pods");
        cx.simulate_keystrokes("enter");
        cx.simulate_keystrokes(":");
        cx.simulate_keystrokes("up");
        assert_eq!(ws.read_with(cx, |w, _| w.input.clone()), "pods");
    }

    #[gpui::test]
    fn digit_switches_to_favourite_namespace(cx: &mut TestAppContext) {
        let (ws, cx) = setup(cx);
        let (tx, rx) = async_channel::unbounded();
        std::mem::forget(tx);
        ws.update(cx, |w, _| {
            w.favourites = vec!["all".into(), "varia".into()];
            w.starter = Some(Box::new(move |_| rx.clone()));
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("1");
        assert_eq!(
            ws.read_with(cx, |w, _| w.options.namespace.clone()),
            Some("varia".to_owned())
        );
        cx.simulate_keystrokes("0");
        assert!(ws.read_with(cx, |w, _| w.options.all_namespaces));
    }

    #[gpui::test]
    fn text_viewer_is_filterable_and_closes_with_escape(cx: &mut TestAppContext) {
        let (ws, cx) = setup(cx);
        cx.run_until_parked();
        ws.update_in(cx, |w, window, cx| {
            let lines: Vec<String> = (0..50).map(|i| format!("line {i}")).collect();
            w.open_picker(PickerKind::Text, &lines, "", window, cx);
            if let Some(p) = &w.picker {
                p.table.update(cx, |t, cx| t.select_last(cx));
            }
        });
        let sel = ws.read_with(cx, |w, cx| w.active_table().read(cx).selected());
        assert_eq!(sel, 49);
        cx.simulate_keystrokes("enter");
        assert!(
            ws.read_with(cx, |w, _| w.picker.is_some()),
            "enter keeps the viewer open"
        );
        cx.simulate_keystrokes("/");
        cx.simulate_input("line 4");
        cx.run_until_parked();
        assert_eq!(
            ws.read_with(cx, |w, cx| w.active_table().read(cx).row_count()),
            11
        );
        cx.simulate_keystrokes("enter escape");
        assert!(ws.read_with(cx, |w, _| w.picker.is_none()));
    }

    #[gpui::test]
    fn delete_is_refused_read_only_and_cancellable_when_writable(cx: &mut TestAppContext) {
        let (ws, cx) = setup(cx);
        cx.run_until_parked();
        cx.simulate_keystrokes("ctrl-d");
        assert!(ws.read_with(cx, |w, _| w.prompt.is_none() && w.flash.is_some()));
        ws.update(cx, |w, _| w.read_only = false);
        cx.simulate_keystrokes("ctrl-d");
        assert_eq!(
            ws.read_with(cx, |w, _| w.prompt),
            Some(Prompt::ConfirmDelete)
        );
        cx.simulate_keystrokes("n");
        assert!(ws.read_with(cx, |w, _| w.prompt.is_none() && w.pending_delete.is_none()));
    }

    #[test]
    fn drill_filters_match_owned_pods_only() {
        let re = |kind, id| regex::Regex::new(&drill_filter(kind, id).unwrap()).unwrap();
        let dep = re(Kind::Deployment, "ns/api.v1");
        assert!(dep.is_match("api.v1-5d9c7b8f6-x2x4z"));
        assert!(!dep.is_match("api.v1-gateway-5d9c7b8f6-x2x4z"));
        assert!(!dep.is_match("apixv1-5d9c7b8f6-x2x4z"));
        assert!(re(Kind::StatefulSet, "ns/db").is_match("db-0"));
        assert!(drill_filter(Kind::Service, "ns/x").is_none());
    }

    #[gpui::test]
    fn actions_are_refused_read_only_and_on_unsupported_kinds(cx: &mut TestAppContext) {
        let (ws, cx) = setup(cx);
        cx.run_until_parked();
        ws.update(cx, |w, cx| w.run_command("scale 3", cx));
        let flash = ws.read_with(cx, |w, _| w.flash.clone().unwrap_or_default());
        assert!(flash.contains("read-only"), "{flash}");
        ws.update(cx, |w, cx| {
            w.read_only = false;
            w.run_command("cordon", cx);
        });
        let flash = ws.read_with(cx, |w, _| w.flash.clone().unwrap_or_default());
        assert!(flash.contains("does not apply"), "{flash}");
    }

    #[gpui::test]
    fn viewer_search_highlights_and_jumps_between_matches(cx: &mut TestAppContext) {
        let (ws, cx) = setup(cx);
        cx.run_until_parked();
        ws.update_in(cx, |w, window, cx| {
            let lines: Vec<String> = (0..50).map(|i| format!("line {i}")).collect();
            w.open_picker(PickerKind::Text, &lines, "", window, cx);
            if let Some(p) = &w.picker {
                p.table.update(cx, |t, cx| t.set_search_mode(true, cx));
            }
        });
        cx.simulate_keystrokes("/");
        cx.simulate_input("line 4");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let (rows, sel, status) = ws.read_with(cx, |w, cx| {
            let t = w.active_table().read(cx);
            (t.row_count(), t.selected(), t.match_status())
        });
        assert_eq!((rows, sel, status), (50, 4, Some((1, 11))));
        cx.simulate_keystrokes("n");
        assert_eq!(
            ws.read_with(cx, |w, cx| w.active_table().read(cx).selected()),
            40
        );
        cx.simulate_keystrokes("shift-n");
        assert_eq!(
            ws.read_with(cx, |w, cx| w.active_table().read(cx).selected()),
            4
        );
    }

    #[gpui::test]
    fn escape_from_logs_returns_to_container_view(cx: &mut TestAppContext) {
        let (ws, cx) = setup(cx);
        cx.run_until_parked();
        ws.update_in(cx, |w, window, cx| {
            w.container_pod = Some(("ns".into(), "pod".into()));
            let data = picker_data("NAME", &["app".into()], "");
            w.open_container_view(data, window, cx);
            w.open_picker(PickerKind::Text, &["log".into()], "", window, cx);
        });
        cx.simulate_keystrokes("escape");
        assert_eq!(
            ws.read_with(cx, |w, _| w.picker.as_ref().map(|p| p.kind)),
            Some(PickerKind::Container)
        );
        cx.simulate_keystrokes("escape");
        assert!(ws.read_with(cx, |w, _| w.picker.is_none()));
    }

    #[gpui::test]
    fn log_view_toggles_json_rendering_and_changes_header_hints(cx: &mut TestAppContext) {
        let (ws, cx) = setup(cx);
        cx.run_until_parked();
        assert_eq!(ws.read_with(cx, |w, _| w.hints().len()), HINTS.len());
        ws.update_in(cx, |w, window, cx| {
            w.open_picker(PickerKind::Text, &[], "", window, cx);
            w.streaming = true;
            w.log_lines = vec![
                r#"{"level":"info","msg":"hi","a":1}"#.into(),
                "plain".into(),
            ];
            w.refresh_log(cx);
        });
        assert_eq!(ws.read_with(cx, |w, _| w.hints().len()), LOG_HINTS.len());
        let spans = |ws: &Entity<Workspace>, cx: &mut VisualTestContext| {
            ws.read_with(cx, |w, cx| w.active_table().read(cx).first_row_spans())
        };
        assert!(spans(&ws, cx) > 0);
        cx.simulate_keystrokes("t");
        assert_eq!(spans(&ws, cx), 0, "raw mode has no spans");
    }

    #[gpui::test]
    fn colon_opens_command_prompt(cx: &mut TestAppContext) {
        let (ws, cx) = setup(cx);
        cx.run_until_parked();
        cx.simulate_keystrokes(":");
        assert!(ws.read_with(cx, |w, _| w.prompt.is_some()));
        cx.simulate_keystrokes("escape");
        assert!(ws.read_with(cx, |w, _| w.prompt.is_none()));
    }

    #[gpui::test]
    fn shift_n_toggles_sort_direction(cx: &mut TestAppContext) {
        let (ws, cx) = setup(cx);
        let table = ws.read_with(cx, |w, _| w.table.clone());
        cx.run_until_parked();
        cx.simulate_keystrokes("shift-n");
        assert_eq!(
            table.read_with(cx, |t, _| t.sort_key().map(|k| k.ascending)),
            Some(true)
        );
        cx.simulate_keystrokes("shift-n");
        assert_eq!(
            table.read_with(cx, |t, _| t.sort_key().map(|k| k.ascending)),
            Some(false)
        );
    }
}
