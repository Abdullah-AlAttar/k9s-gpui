# k9s-gpui

Native GPUI replica of k9s. Plan: `docs/PLAN.md`. Reference sources live in `tmp/` (git-ignored).

## Layout
`k9s_core` → `k9s_render` → `k9s_kube`/`k9s_config` → `k9s_app` → `k9s_components`/`k9s_ui` → `k9s` (binary).
Dependencies point left only. `k9s_core` never depends on GPUI, kube or k8s-openapi; `k9s_render` may use `k8s-openapi` types but never GPUI or the kube client.
The UI never imports `kube`/`k8s-openapi`.

Crates: `k9s_core` (pure model), `k9s_render` (columns, describe, logs, ANSI/JSON), `k9s_kube` (worker, logs, delete), `k9s_config` (stub), `k9s_app` (facade over kube), `k9s_components` (table, theme), `k9s_ui` (workspace view), `k9s` (binary `k9s-gpui`).

## Workflow
Use `nix develop` (or direnv). Then `task ci` (fmt check, clippy `-D warnings`, nextest). `task run` / `task run-release` start the app.
Flakes only see git-tracked files: `git add` new files before `nix develop`.
Live tests: `K9S_TEST_NS=<ns> cargo test -p k9s_kube --test live -- --ignored --nocapture`.
Stale `TMPDIR` in a nix shell breaks clang builds: prefix with `TMPDIR=/tmp`.

## Git
Never commit or push unless explicitly asked. Branch is `master`. Personal identity is Abdullah-AlAttar; do not change git config.

## Releases
Tag `vX.Y.Z` matching the root `Cargo.toml` version; `.github/workflows/release.yml` builds Linux (tar.gz + AppImage via `scripts/appimage-build.sh`), macOS and Windows. CI is `ci.yml`.
`.cargo/config.toml` links x86_64 Linux with mold, so CI installs `mold`; the 22.04 AppImage container overrides it with `RUSTFLAGS`.

## GPUI rules
- Never block the UI thread; kube I/O runs on the worker runtime and reaches the UI over a bounded `async_channel`.
- Keep `render` thin; precompute display strings outside it.
- Guard async results against staleness with a generation counter.
- Give interactive elements stable `.id()`s; every feature must be reachable by keyboard.
- Avoid deprecated `Model`/`View`/`WindowContext`; use `Entity`, `Context`, `Window`.

## Code rules
- No `unwrap`/`expect`/indexing outside tests. No `unsafe`.
- Comment only what needs clarification.
- The app is read-only unless `--write`; develop against fixtures or a kind cluster, and never mutate a real cluster in tests.
- No `kubectl` dependency: use the `kube` library.
