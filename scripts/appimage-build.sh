#!/usr/bin/env bash
# Runs inside a native-architecture ubuntu:22.04 container so the binary and the
# bundled libraries only need glibc 2.35. Never run on the host.
set -euo pipefail

: "${CARGO_HOME:?CARGO_HOME must point at the Rust installation}"
export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y --no-install-recommends \
  build-essential ca-certificates curl clang pkg-config \
  libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev libxcb1-dev libxcb-xkb-dev \
  libx11-xcb-dev libfontconfig-dev libfreetype-dev libvulkan-dev libssl-dev

export PATH="${CARGO_HOME}/bin:$PATH"
# Jammy's gcc cannot use mold (.cargo/config.toml); this env var overrides the config rustflags.
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-C debuginfo=0"
cargo build --release --locked -p k9s

appdir=AppDir
rm -rf "$appdir"
install -Dm755 target/release/k9s-gpui "$appdir/usr/bin/k9s-gpui"
install -Dm644 packaging/k9s-gpui.desktop "$appdir/usr/share/applications/k9s-gpui.desktop"
install -Dm644 packaging/k9s-gpui.svg "$appdir/usr/share/icons/hicolor/scalable/apps/k9s-gpui.svg"
cp packaging/k9s-gpui.desktop packaging/k9s-gpui.svg "$appdir"/
install -Dm755 packaging/AppRun "$appdir/AppRun"

# glibc and GPU/graphics stacks must come from the host; everything else is bundled.
mkdir -p "$appdir/usr/lib"
ldd target/release/k9s-gpui | awk '/=> \// {print $1, $3}' | while read -r soname path; do
  case "$soname" in
    ld-linux*|libc.so.*|libm.so.*|libdl.so.*|libpthread.so.*|librt.so.*|libresolv.so.*|libutil.so.*|libGL*|libEGL*|libOpenGL*|libvulkan*|libdrm*|libgbm*|libnvidia*) ;;
    *) cp -L "$path" "$appdir/usr/lib/" ;;
  esac
done
ls "$appdir/usr/lib"
