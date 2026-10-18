<p align="center">
  <img src="docs/assets/logo.webp" alt="Tokenade — a lemon slice dissolving into pixels" width="180" />
</p>

# Tokenade

> The universal token-optimization engine for AI coding agents — Claude Code,
> Cursor, Codex, Copilot, Kilo Code, Windsurf… one ultra-light Rust binary.

Tokenade combines the best **complementary** mechanisms from across the
token-optimization ecosystem (see [`reports/COMPARISON.md`](reports/COMPARISON.md))
into a single dependency-free tool:

| Layer | Mechanism | Inspired by | Measured impact (token-weighted, v5 bench) |
|-------|-----------|-------------|-----------------|
| Pre-execution | **Command rewriting** — terser flags before the shell runs (`git status` → `--porcelain=v2`, `pytest` → `-q --tb=line`, `kubectl get` → `-o name`, `docker ps` → `--format`, `tsc` → `--pretty false`, …) + compound-chain splitter | token-savior | source-side trim that stacks with the per-format compactor |
| Shell/tool boundary | **Output filtering** (git/cargo/test/ls/grep + 48 format-specific compactors) | rtk | −99% on git logs, −99.5% on cargo logs, −98% on terraform, −95% on k8s manifests |
| Code navigation | **Symbol + import-graph index** (14 languages, **parallelised with rayon**) | tokensave, codegraph | 3-5× faster build; ≥95% recall on 14-repo battery |
| Concept search | **Hybrid BM25 + dense static embeddings (`potion-code-16M`, 63 MB bundled) + RRF score fusion + code-aware reranking** | claude-context, semble | ranks definitions above usages, code over tests; <1 ms encode, 30 ms warm query |
| Framework boost | **Detect Next/Django/Rails/Spring/Phoenix/Axum**, rank canonical files higher | codegraph | semantic search +25% on framework-canonical paths |
| Context | **Skeleton compression** (code + YAML + Markdown + Terraform) | token-optimizer, codesight | −64% on file reads, every top-level decl preserved |
| Tests | **Group identical tracebacks** in pytest/cargo-test | (new) | 50 identical failures → 1 example + `(×50)` |
| Web | **SERP compactor** + improved markdown (−63%) + a11y (drop empty wrappers, role prefixes) + **upfront token cap** on HTML input | defuddle, readability, jina-reader | −62.8% on README, −80%+ on SERPs; bail before regex on 10 MB SPAs |
| JSON/API | **Schema-aware filtering** for kubectl + GitHub PR/issue | (new) | −40% on kubectl JSON, −50% on GitHub PRs |
| MCP transport | **Lazy tool loading** (50+ tools) + **adaptive filtering** (hide tools whose target binary isn't installed) | lazy-mcp | removes per-turn manifest cost |
| Per-user extension | **TOML preset auto-discovery** from `~/.config/tokenade/presets/*.toml` — add command-rewrite rules for niche CLIs without recompiling | mcp-rtk | zero-recompile rule extension |
| Cache | **Content-addressed disk cache** with hit/miss metrics + `gain.jsonl` 10-MB rotation | (new) | persists across sessions, bounded growth |
| Secrets | **Argv + log redaction** of Bearer/Basic/JWT/AWS/GitHub PAT/Stripe/etc. patterns | (new) | gain.jsonl safe to commit/share |
| Tokenizer | **BPE-calibrated heuristic** (default) or real `tiktoken-rs` cl100k (`--features bpe`) | (new) | heuristic within 5% of real BPE on JSON/code |
| Proof | **`tokenade dashboard`** — measured savings + bypass-per-project + bypass-per-tool + cache + framework | rtk, token-optimizer | one-screen view of what tokenade compacted vs what slipped through |

On a 14-repo battery (Rust/Python/Go/JS/TS + a real monorepo + ruff +
synthetic Next.js/Django mini-repos for framework boost), with
**realistic in-process grep/find baselines**, **token-weighted aggregation**,
and 118 adversarial impact tests:

| Session mix | Savings | Quality | −70% target |
|-------------|:--:|:--:|:--:|
| balanced | **88.3%** | 1.00 | ✅ |
| build-heavy | **86.7%** | 1.00 | ✅ |
| navigation-heavy | **83.5%** | 0.94 | ✅ |
| web-heavy | **68.6%** | 1.00 | ❌ (1.4 pp away) |

Adversarial impact-negative tests: min quality 1.00, mean 1.00 over 118 runs.
**9.6k+ unit + property-based tests pass**, clippy `-D warnings` and
`cargo fmt --check` clean. Full methodology and the v1 retraction in
[`reports/EVALUATION.md`](reports/EVALUATION.md).

### Zero-config defaults (no flags, no env vars)

| Mechanism | What you get | Cost |
|---|---|---|
| **SQLite memory** (rusqlite + bundled) | indexed lookups, WAL concurrency, auto-migration from v3 JSON | +5 MB binary |
| **Static-embedding model** (`potion-code-16M` via `model2vec-rs`) | code-specialised 256-d semantic search distilled from CodeRankEmbed | **63 MB bundled in the binary** — no first-run download, no `~/.cache/` pollution, ~200× faster encode than a transformer |
| **Hybrid BM25 + dense + RRF fusion** | catches both keyword (function name) and intent (paraphrase) queries; auto-tunes the alpha weight by query shape | ~30 ms per query on a 5k-chunk corpus |
| **AST-aware chunking** (tree-sitter, 16 languages) | chunks respect function/class boundaries — a long function never gets split between chunks A and B | none |
| **Code-aware reranking** (RRF + multi-chunk-file boost + definition-keyword boost + file-stem match + test-path penalty) | the chunk that *defines* `User` outranks the chunk that *uses* it; tests demoted for symbol queries | pure logic, microseconds |
| **Parallel index build** (rayon) | 3-5× faster on multi-core | none |
| **Secret redaction** | gain.jsonl safe to commit/share | none |
| **Postcard binary serialisation** for `.tokenade/semantic.bin` | 8-12× smaller cache files | none |

The embedder is bundled at build time — no download path, no offline-mode
fallback needed, no `XDG_CACHE_HOME` mucking (this was the L100 leak source).

### Opt-in features

| Feature | What it enables | When to turn on |
|---|---|---|
| `--features bpe` | Real `tiktoken-rs` cl100k tokenizer instead of the calibrated heuristic | Exact token counting for bill-down tooling |
| `tokenade install --with-hook` | PostToolUse hook in `.claude/settings.json` — every Bash output goes through `auto::compact` transparently | You don't want to wrap commands with `tokenade proxy` |
| `TOKENADE_CACHE_HINTS=1` | Prepend `[tokenade-cache:k=…]` markers so Anthropic's prompt cache hits across turns | Long-lived sessions on Claude API |

## Install

```sh
npm install -g @tokenade/cli
tokenade install    # auto-detects your coding agents and wires itself in
tokenade login      # connect your free account (opens tokenade.net to approve this device)
```

Tokenade is **free** — you just need a free account. After `tokenade login`
savings show up in your status line immediately (Free plan: 10M tokens saved /
month). Don't have an account yet?
[Create one](https://tokenade.net/signup) — it's free — or go
[Pro](https://tokenade.net/pricing) for 100M/month.

The binary embeds the 63 MB `potion-code-16M` embedder, so first-run encode is
free — no model download, no `~/.cache/tokenade/` pollution.

### From source (contributors)

```sh
cargo build --release --bin tokenade
cargo install --path crates/tokenade-cli
tokenade install
```

### Plug into your agent

Nothing to do: `tokenade install` wires the hooks into every detected agent
(Claude Code, Cursor, Gemini CLI, Copilot, Codex, opencode, Cline, …) and
drops a `CLAUDE.md`/`AGENTS.md` guidance block so the model prefers the
token-cheap tools on its own.
## Token economy

The `tokenade` MCP server is available globally. Prefer these tools over their
raw equivalents — they save 35-95 % of tokens with no quality loss:

- `exec_compact <cmd>` instead of raw `Bash <cmd>` for noisy commands (git,
  cargo, kubectl, terraform, docker)
- `exec_script` (think-in-code) instead of reading data into context and
  filtering it afterwards — the script runs apart and only its stdout
  reaches you (it runs as you, with your filesystem: treat a script the
  way you would treat running it in your own shell)
- `symbol_find` / `impact_of` / `structure_map` / `skeleton` instead of
  reading source files whole
- `semantic_search` for natural-language queries across the codebase
- `auto_compact <text>` when you already have a noisy blob and want it
  compressed in-place
```

### Measured savings (run `cargo run -p tokenade-bench --bin per_command --release`)

Per-command bench across 355 real fixtures captured from Windows + macOS VMs
(`crates/tokenade-core/tests/fixtures/`):

| Metric | Value |
|---|---|
| Fixtures routed via `dispatch_named` | 310 / 355 (87.3 %) |
| Overall tokens before → after | 121 279 → 78 836 |
| **Overall token saving** | **35.0 %** |
| Windows median saving | 45.5 % (45.5 % saved on long cmd.exe outputs) |
| macOS median saving | 3.1 % (BSD outputs already concise) |
| Top wins | `schtasks` -91 %, `driverquery` -84 %, `tasklist` -75 %, `systeminfo` -55 % |

## Usage
```sh
tokenade proxy git status        # run a command, emit compressed output, log savings
tokenade map                     # one-screen codebase structure map
tokenade query parse_config      # locate a symbol by name (substring)
tokenade ask "auth flow login"   # hybrid BM25 + dense embeddings, framework-aware
tokenade impact src/foo.rs       # refactor blast radius (dependents + symbols)
tokenade skeleton src/foo.rs     # signatures-only view of a file
tokenade auto report.json        # auto-detect format + compact (THE one to call)
tokenade dashboard               # one-screen savings + cache + index
tokenade gain --history          # raw cumulative token savings
```

## Two modes
- **Hooks** (Claude Code, Gemini CLI, Cursor, …): installed by
  `tokenade install` — every noisy tool output is compacted transparently at
  the agent-harness boundary, with a recovery path (`tokenade expand-ref`)
  for the raw bytes.
- **Wrap** (any other agent/CI): `tokenade wrap '<cmd>'` runs the command and
  emits the compacted output; `tokenade mcp-proxy -- <server-cmd>` folds MCP
  tool results in transit. `tokenade mcp-proxy install [<server>]` routes your
  configured MCP servers through that fold automatically (reversible with
  `uninstall`); `tokenade mcp-proxy report` shows per-tool tokens and dollars
  saved.

## Architecture
```
crates/
  tokenade-core    mechanisms: tokenizer · filter · index · compress · cache · detect
  tokenade-cli     the `tokenade` binary (proxy/index/query/gain/install/…)
  tokenade-bench   hermetic benchmarking framework (CSV/JSON output)
```
Dependencies: `serde`, `serde_json`, `regex`. No async runtime, no vector DB,
no heavy ML. Builds for Linux (musl), macOS, and Windows via GitHub Actions.

## Reports & benchmarks
- `reports/<tool>.md` — per-competitor analysis (14 tools)
- `reports/COMPARISON.md` — mechanism comparison, competing vs complementary
- `reports/EVALUATION.md` — competitor scoring + isolated mechanism impact + self-eval
- `benchmarks/` — test repos, task set, `results.csv` / `results.json`

## License
MIT
