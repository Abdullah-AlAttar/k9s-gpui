# k9s-gpui

A native, GPU-accelerated desktop replica of [k9s](https://k9scli.io), built with
[GPUI](https://www.gpui.rs) and Rust. Same keyboard-first workflow, in a real window that scales with font zoom.

> Early (v0.1): pods are covered in depth; other resource kinds are on the roadmap.

## Features

- Live pod table with k9s columns, metrics (CPU/MEM, % of requests/limits), wide mode and a faults-only filter
- `/` regex filter, `:` command bar with history, `:ctx` / `:ns` pickers, favourite namespaces (`0`-`9`)
- YAML (`y`), native describe (`d`, pod + events), container view, streaming and previous logs
- Logs: JSON rendering with colours (`t`), ANSI colours, wrap (`w`), copy line (`c`) / all visible (`shift-c`)
- Read-only by default; delete (`ctrl-d`) needs `--write` and asks for confirmation
- Zoom with `ctrl-=` / `ctrl--` / `ctrl-0`; `?` shows all keys
- No `kubectl` needed: everything uses the Kubernetes API directly

## Install

Download a build from [Releases](../../releases): Linux (`.tar.gz`, AppImage; x86_64 and arm64), macOS (arm64, x86_64) or Windows (x86_64).
Linux needs a Vulkan driver and Wayland or X11.

## Usage

```
k9s-gpui [--context <ctx>] [-n <namespace>] [-A] [--write]
```

| Flag | Meaning |
| --- | --- |
| `--context` | kubeconfig context |
| `-n`, `--namespace` | start namespace |
| `-A`, `--all-namespaces` | all namespaces |
| `--write` | allow mutating actions (delete) |

## Build from source

The project uses a Nix flake dev shell (works with direnv) and [Task](https://taskfile.dev).

```sh
nix develop
task run -- --context dev -n default      # debug build
task run-release -- --context dev         # release build (NVIDIA offload env set)
task ci                                   # fmt check, clippy -D warnings, nextest
```

Without Nix you need a Rust toolchain plus the Wayland/X11, xkbcommon, fontconfig, freetype and Vulkan development libraries.
On NixOS, if clang builds fail with a stale `TMPDIR`, prefix commands with `TMPDIR=/tmp`.

Live tests against a real cluster: `K9S_TEST_NS=<ns> cargo test -p k9s_kube --test live -- --ignored --nocapture`.

## Releasing

Bump `version` in the root `Cargo.toml`, push, then `git tag vX.Y.Z && git push origin vX.Y.Z`.
The Release workflow builds all platforms and publishes a GitHub Release (tags with `-` are prereleases).

## Project docs

- [`docs/PLAN.md`](docs/PLAN.md): architecture, status and roadmap
- [`AGENTS.md`](AGENTS.md): rules for contributors and coding agents

## License

Apache-2.0. See [`LICENSE`](LICENSE).
