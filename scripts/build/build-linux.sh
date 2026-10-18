#!/usr/bin/env bash
# Build tokenade on Linux (x86_64 or aarch64).
#
#   scripts/build/build-linux.sh                    # native glibc build for this host
#   scripts/build/build-linux.sh --musl             # static musl build for this host's arch
#   scripts/build/build-linux.sh --target <triple>  # any other Rust target triple
#
# Needs: stable Rust (rustup), a C/C++ compiler. `lld` is used as the host
# linker on x86_64 if installed (.cargo/config.toml); without it the default
# linker is used. Cross/musl builds use `cargo zigbuild` when it is installed
# (cargo install cargo-zigbuild + zig), plain `cargo build` otherwise.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

target=""
musl=0
while [ $# -gt 0 ]; do
  case "$1" in
    --musl) musl=1 ;;
    --target) target="${2:?--target needs a triple}"; shift ;;
    -h|--help) sed -n '2,11p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done

arch="$(uname -m)"
[ "$arch" = arm64 ] && arch=aarch64
[ "$musl" = 1 ] && [ -z "$target" ] && target="$arch-unknown-linux-musl"

# .cargo/config.toml asks for lld on x86_64-gnu; fall back if it is missing.
if [ "$arch" = x86_64 ] && ! command -v ld.lld >/dev/null; then
  echo "note: lld not found, using the default linker"
  export RUSTFLAGS="${RUSTFLAGS:-}"
fi

if [ -z "$target" ]; then
  cargo build --release --locked --bin tokenade
  bin="${CARGO_TARGET_DIR:-target}/release/tokenade"
else
  command -v rustup >/dev/null && rustup target add "$target"
  case "$target" in
    *-musl)
      # sqlite-vec uses BSD u_int*_t types absent from musl headers (same as CI).
      export "CFLAGS_${target//-/_}=-Du_int8_t=uint8_t -Du_int16_t=uint16_t -Du_int64_t=uint64_t" ;;
  esac
  if command -v cargo-zigbuild >/dev/null; then
    cargo zigbuild --release --locked --bin tokenade --target "$target"
  else
    echo "note: cargo-zigbuild not found, trying plain cargo build (needs a C toolchain for $target)"
    cargo build --release --locked --bin tokenade --target "$target"
  fi
  bin="${CARGO_TARGET_DIR:-target}/$target/release/tokenade"
fi

echo "built: $bin"
"$bin" --version 2>/dev/null || echo "(cannot run $bin on this host)"
