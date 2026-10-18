# Tokenade

**Cut your AI coding agent's token bill — locally, automatically, on every prompt.**

Open source (MIT OR Apache-2.0) · no account · no telemetry ·
**#1 on [THOL](https://tokenade.net/en/thol): 39% cheaper on long sessions.**

```sh
npm install -g @tokenade/cli
tokenade install      # detects your coding agents and wires Tokenade in
tokenade healthcheck  # optional: verify the wiring
```

That's it: Tokenade now trims what your agent sends to the model — noisy
command output, re-reads, verbose MCP payloads, whole-file reads — and keeps
every folded byte recoverable (`tokenade expand-ref <hash>`).

Works with Claude Code, OpenCode, Kilo Code, Copilot CLI, Hermes, Cline,
T3 Code, OpenClaw, Pi, Qwen Code, Codex (CLI and app), Cursor, Grok, Droid,
Aider and Antigravity — see the
[supported-agents section](https://github.com/pi-infected/tokenade#supported-agents)
for what each one gets.

## How this package installs the binary

The binary for your platform comes from a per-platform optional dependency
(`@tokenade/cli-<os>-<cpu>[-musl]`). If npm skipped it (`--no-optional`, an
unusual platform), `postinstall` downloads
`tokenade-<version>-<target>.tar.gz` from the
[GitHub Release](https://github.com/pi-infected/tokenade/releases) of the same
version, after verifying the release's `manifest.json` against its Ed25519
signature and the archive against the SHA-256 listed there.
`TOKENADE_DOWNLOADS_BASE` points that fallback at a mirror serving the same
files. Behind a proxy, set `HTTPS_PROXY`.

Linux (x64 glibc/musl, arm64) · macOS (x64, arm64) · Windows (x64, arm64) ·
Node ≥ 18.

## Privacy

Nothing leaves your machine except what you ask for (`tokenade web`,
`tokenade search`), the one-time embedding-model download, and a daily signed
update check against GitHub Releases. Disable auto-update with
`TOKENADE_NO_AUTO_UPDATE=1` or `tokenade upgrade --off`.

## Upgrading from 1.x

`npm i -g @tokenade/cli@latest`. On first run 2.0 removes the old licence and
usage-report files under `~/.tokenade` and keeps your savings history.

## Links

- Source, issues, docs: <https://github.com/pi-infected/tokenade>
- Changes: [PATCHNOTES.md](https://github.com/pi-infected/tokenade/blob/main/PATCHNOTES.md)
- License: MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`)
