#!/usr/bin/env bash
# Build tokenade on macOS.
#
#   scripts/build/build-macos.sh               # this Mac's architecture
#   scripts/build/build-macos.sh --target aarch64-apple-darwin   # Apple Silicon
#   scripts/build/build-macos.sh --target x86_64-apple-darwin    # Intel
#   scripts/build/build-macos.sh --universal   # both, merged with lipo
#
# Needs: stable Rust (rustup) and the Xcode Command Line Tools
# (xcode-select --install).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
out="${CARGO_TARGET_DIR:-target}"

build() {
  rustup target add "$1"
  cargo build --release --locked --bin tokenade --target "$1"
}

case "${1:-}" in
  "")
    arch="$(uname -m)"; [ "$arch" = arm64 ] && arch=aarch64
    build "$arch-apple-darwin"; bin="$out/$arch-apple-darwin/release/tokenade" ;;
  --target)
    build "${2:?--target needs a triple}"; bin="$out/$2/release/tokenade" ;;
  --universal)
    build aarch64-apple-darwin
    build x86_64-apple-darwin
    mkdir -p "$out/universal-apple-darwin/release"
    bin="$out/universal-apple-darwin/release/tokenade"
    lipo -create -output "$bin" \
      "$out/aarch64-apple-darwin/release/tokenade" "$out/x86_64-apple-darwin/release/tokenade" ;;
  -h|--help) sed -n '2,10p' "$0"; exit 0 ;;
  *) echo "unknown argument: $1" >&2; exit 2 ;;
esac

echo "built: $bin"
"$bin" --version 2>/dev/null || echo "(cannot run $bin on this Mac; Intel binaries need Rosetta)"
