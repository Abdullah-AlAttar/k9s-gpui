# k9s-gpui — Plan

Native GPUI desktop app replicating k9s. Phase 1: a window, a live Pods table, proper crate structure.
References (in `tmp/`): k9s (features), dbflux (workspace layout), herdr-gpui (GPUI code patterns).

## Decisions

| Topic | Choice | Why |
|---|---|---|
| GPUI | `gpui = { package = "gpui-pre", version = "=0.3.9" }`, `gpui-pre-platform =0.3.9` (features `font-kit, runtime_shaders, wayland, x11`) | Same pins as dbflux; no vendoring needed. Without the platform features text silently renders nothing. |
| Widgets | Hand-rolled (herdr style), no `gpui-component` | Avoids `Root` and vendored patches; the table is simple (`uniform_list`). |
| Kube | `kube` + `k8s-openapi` + `tokio` on a **dedicated runtime thread** | GPUI executor is not tokio. UI never blocks. |
| UI ↔ kube bridge | bounded `async_channel`, `cx.spawn` loop → `entity.update` + `cx.notify()` | Pattern from dbflux/herdr. Generation counter guards stale results. |
| Data model | Re-implement k9s `render` (columns, status, ready, age) in pure Rust | k9s has no GUI-neutral core; only its tview layer is replaced. |
| Refresh | Phase 1: poll `list` every 2s (k9s default). Later: `watcher` | Simple first; a trait hides the difference. |
| Toolchain | `rust-toolchain.toml` (stable 1.95+), edition 2024, mold on Linux | Matches dbflux. **Rust is not installed here** — provide via nix devShell. |
| Lints | workspace lints: deny `unsafe`; warn `unwrap_used`, `expect_used`; 1000-line file cap | From herdr/dbflux AGENTS.md. |

## Workspace layout

```
k9s-gpui/
├─ Cargo.toml                 # workspace, shared deps/lints
├─ rust-toolchain.toml  rustfmt.toml  clippy.toml  Taskfile.yml  flake.nix
├─ AGENTS.md                  # GPUI rules
├─ docs/PLAN.md
└─ crates/
   ├─ k9s_core/        # no GPUI/kube: Row/Column/TableData, sort, filter, age fmt,
   │                   #   Command/ContextId, ResourceKind, ResourceSource trait
   ├─ k9s_render/      # pure renderers: pod.rs (READY/STATUS/RESTARTS/AGE...), later deploy, node...
   ├─ k9s_kube/        # kube-rs client, kubeconfig/contexts, namespaces, list/watch, metrics,
   │                   #   tokio runtime thread, implements ResourceSource
   ├─ k9s_config/      # config.yaml, per-context config, skins, aliases (XDG dirs)
   ├─ k9s_app/         # AppState: active context/namespace/view, history, worker bridge
   ├─ k9s_components/  # theme + tokens, data_table (uniform_list), header/crumbs/status bar,
   │                   #   text input (filter/command), modal — domain-free
   ├─ k9s_ui/          # Workspace root view, views/pods, key routing, command bar (`:`)
   └─ k9s/             # binary: main.rs, assets, window bootstrap
```

Dependencies point down: `k9s → k9s_ui → {k9s_components, k9s_app} → {k9s_kube, k9s_config} → k9s_render → k9s_core`.
The UI never imports `kube`/`k8s-openapi`; it only sees `k9s_core` types.

## Milestones

### M0 — Scaffold
Install Rust; create workspace, lints, `Taskfile.yml` (`fmt`, `clippy -D warnings`, `test`, `ci`), AGENTS.md.
**Done when:** `task ci` passes on empty crates.

### M1 — Window + shell (no cluster)
- Binary: `gpui_platform::application().with_assets(..).run`, open window, `Quit` action, key bindings.
- `k9s_components`: theme (struct of `u32` colours, k9s stock skin defaults), tokens, fonts.
- `k9s_ui::Workspace`: k9s-like layout — header (context/cluster/ns/user), table area, crumbs, flash/status bar.
**Done when:** window opens on Linux with mock pod data; headless `#[gpui::test]` renders it.

### M2 — Table component
- `TableModel{columns, rows}`, `DataTableState` entity (selection, scroll handle, column widths, focus).
- Virtualised via `uniform_list` + `track_scroll`; fixed header; `scroll_to_item` follows selection.
- Keys: `j/k`, arrows, `g`/`G`, `ctrl-f/b`; Shift-N/A/S sort with direction flip; row colours by status.
**Done when:** 10k mock rows scroll smoothly; tests for navigation and sort.

### M3 — Pure pod rendering (`k9s_render`)
Port k9s `render/pod.go`: NAMESPACE, NAME, PF, READY (incl. sidecars), STATUS (`Init:*`, waiting/terminated reasons,
`Terminating`, `Unknown`), RESTARTS, CPU/MEM + %, IP, NODE, AGE (`HumanDuration`); natural-order sort, tie-break on row id.
Fixture-driven tests mirroring `render/pod_test.go`.
**Done when:** rendered rows match k9s output for the fixtures.

### M4 — Live cluster (`k9s_kube`)
- kubeconfig contexts, current namespace; tokio worker thread; `list` pods + optional `metrics.k8s.io` (hide metric columns if absent).
- Poll 2s with backoff → `async_channel` → UI update; generation counter drops stale results; errors → flash.
- Header shows real context/cluster/user/version.
**Done when:** Pods view shows live pods, updates within ~2s, survives API errors.

### M5 — Navigation essentials
`:` command bar (`:pods`, `:ns x`, `:ctx x`, `:q`), `/` filter (regex, `!` inverse), `0–9` namespace favourites,
Ctrl-W wide, Ctrl-Z faults, `?` help overlay.
**Done when:** context/namespace switching and filtering work fully from the keyboard.

### Later
Describe/YAML, logs, delete + confirm, containers drill-down, more kinds (deploy, sts, svc, node, events, generic CRD browser),
port-forward, shell/exec via external `kubectl`, scale/restart, skins, plugins/hotkeys/aliases, xray/pulse, `watcher` updates.

## Phase 1 scope
M0 → M4 plus the `:` / `/` essentials of M5. New kind = one renderer in `k9s_render` + one `ResourceKind`;
new view = one module in `k9s_ui/views`.

## Rules (go into AGENTS.md)
1. Never block the UI thread; all kube I/O on the worker runtime.
2. Keep `render` thin; precompute display strings in `k9s_render`.
3. Bounded channels; coalesce stale snapshots.
4. `k9s_core`/`k9s_render` stay GPUI- and kube-free (testable without window or cluster).
5. Stable `.id()`s for tests; every feature reachable by keyboard.
6. Files ≤ 1000 lines; no `unwrap`/`expect` outside tests.

## Risks / open items
- Rust toolchain missing; Linux deps (xkbcommon, wayland, vulkan-loader, fontconfig, freetype) via nix devShell (copy dbflux `default.nix`).
- `kubectl` points at context `stage`: develop against fixtures or a local kind cluster; keep the app read-only until delete/edit exist.
- Metrics need metrics-server; degrade gracefully.
- GPUI text-input shutdown panic (`WeakInputHandler` workaround in herdr on 0.3.6) — verify on 0.3.9 when building filter/command input.

---

## Status (v0.1 — first version)

| Milestone | State |
|---|---|
| M0–M5 scaffold, window, virtualised table, pod rendering, live worker, `/` `:` bars | done |
| N1 UI zoom (one `font_px` drives all sizes) | done (no screenshot test yet) |
| N2 pod parity: 22 columns, metrics, `ctrl-w` wide, `ctrl-z` faults | done |
| N3 `:ctx` / `:ns` pickers, regex filter, `?` help, `:` history, full k9s-style header, `0-9` namespace favourites | done |
| N4 `y` YAML, `d` describe, container view, streaming/previous logs, back-stack, `ctrl-d` delete behind read-only guard | done |
| Logs: JSON and ANSI colour rendering, `t` JSON/raw, `w` wrap, `c` / `shift-c` copy, per-view header hints | done |

Verified: `task ci` (29 tests, 4 live tests ignored) and live against `dev/varia`. Never checked: delete on a real cluster, GPU selection, hi-dpi.

## Known gaps / cleanup candidates
- No `kubectl` dependency: `d` describe is native (`k9s_render::describe`, Pod + Events); everything uses `kube` + `k8s-openapi`.
- Wrap width is fixed when toggled (not recomputed on zoom/resize); filter matches wrapped pieces, not whole lines.
- No mouse text selection (keyboard copy only); no horizontal scroll; column widths are not content-based.
- Favourites and last namespace are not persisted; `k9s_config` is a stub.
- No screenshot (`render_to_image`) test; no row colours for add/update/delete events.
- Container view lacks CPU/MEM and port-forward columns.

## Next plan

### N3b — Navigation leftovers
Autocomplete for `:`, `-l` label selectors, per-context config (persisted favourites, last namespace).

### N4b — Pod actions
2. Log toggles: timestamps, tail length, level filter, expand one JSON line.
3. `e` edit: in-app editor view (own or `gpui-component`), server-side apply; no `$EDITOR`.
4. `s` shell / `a` attach: Herdr-style PTY + terminal emulator + cell painter, last.
5. Mouse text selection in viewers.

### N5 — More resource kinds
Generic `ResourceKind` registry: deploy, sts, ds, job, cronjob, svc, node, ns, cm, secret, events, CRDs; per-kind actions (scale, restart, cordon/drain); `shift-j` jump to owner, `o` show node.

### N6 — Later
Port-forward, skins, aliases/hotkeys/plugins, xray, pulse, watch-based updates (`kube::runtime::watcher`), screenshot tests, packaging (nix package, release CI).
