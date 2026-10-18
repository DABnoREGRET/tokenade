# Contributing to Tokenade

Thanks for helping. Bug reports, new compactor fixtures and pull requests are
all welcome.

## Reporting a bug

Open an issue with the bug template. The most useful reports include
`tokenade --version`, your OS, your coding agent, and the exact command or
output Tokenade mishandled. **Redact secrets** before posting. Security
problems go through [SECURITY.md](SECURITY.md), not public issues.

## Development setup

- Stable Rust (see `rustup`), a C/C++ compiler (tree-sitter grammars and
  SQLite are compiled from source). On Linux, `lld` is used as the host
  linker (`.cargo/config.toml`) — install it (`apt install lld`).
- Optional: [`cargo-nextest`](https://nexte.st) for the fast test runner.
- Release-style builds per OS (musl, macOS universal, Windows x64/ARM64):
  see [Build from source](README.md#build-from-source) and `scripts/build/`.

```sh
cargo build --release --bin tokenade   # binary at target/release/tokenade
cargo nextest run --workspace          # or: cargo test --workspace
cargo test --doc --workspace           # nextest skips doctests
cargo fmt --all -- --check
cargo clippy --workspace --all-targets
```

The release profile uses fat LTO; `.cargo/config.toml` caps parallel link
jobs because each link is memory-hungry.

## Layout

- `crates/tk-base`, `tk-fold`, `tk-formats`, `tk-agents` — shared building
  blocks (platform, redaction, folding, format detection, agent adapters).
- `crates/tokenade-core` — compactors, code index, semantic search, updater.
- `crates/tokenade-cli` — the `tokenade` binary: commands, hooks, installers.
- `crates/tokenade-bench` — benchmarks and measurement tools.
- `npm/` — the `@tokenade/cli` npm wrapper.

## Pull requests

- One logical change per PR, with a test. A compactor change needs a fixture
  (real captured output, secrets removed) and a test that fails without it.
- Never drop information silently: failures, numbers and decisive lines must
  survive a fold, and anything folded must stay recoverable.
- No new network calls. Tokenade has no telemetry; a guard test fails on any
  hard-coded phone-home URL.
- Keep `cargo fmt`, `cargo clippy` and the test suite green (CI runs them).
- User-visible changes get a line in `PATCHNOTES.md` under the open version.

By contributing you agree that your contribution is licensed under
`MIT OR Apache-2.0`, as described in the README.
