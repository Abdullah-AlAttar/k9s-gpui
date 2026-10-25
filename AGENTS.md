# k9s-gpui

Native GPUI replica of k9s. Plan: `docs/PLAN.md`. Reference sources live in `tmp/` (git-ignored).

## Layout
`k9s_core` → `k9s_render` → `k9s_kube`/`k9s_config` → `k9s_app` → `k9s_components`/`k9s_ui` → `k9s` (binary).
Dependencies point left only. `k9s_core` never depends on GPUI, kube or k8s-openapi; `k9s_render` may use `k8s-openapi` types but never GPUI or the kube client.
The UI never imports `kube`/`k8s-openapi`.

## Workflow
Use `nix develop` (or direnv). Then `task ci` (fmt check, clippy `-D warnings`, nextest).

## GPUI rules
- Never block the UI thread; kube I/O runs on the worker runtime and reaches the UI over a bounded `async_channel`.
- Keep `render` thin; precompute display strings outside it.
- Guard async results against staleness with a generation counter.
- Give interactive elements stable `.id()`s; every feature must be reachable by keyboard.
- Avoid deprecated `Model`/`View`/`WindowContext`; use `Entity`, `Context`, `Window`.

## Code rules
- Files ≤ 1000 lines. No `unwrap`/`expect`/indexing outside tests. No `unsafe`.
- Comment only what needs clarification.
- The cluster is read-only until delete/edit features exist; develop against fixtures or a kind cluster.
