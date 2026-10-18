//! Detect competing token-optimization tools already installed on the
//! host, and classify them so [`tokenade install`] can offer the right thing.
//!
//! Four classes, four UX consequences:
//!
//! | Class | Examples | What `tokenade install` offers |
//! |---|---|---|
//! | [`Class::DropInBetter`] | `rtk`, `codegraph`, `tokensave`, `codesight` | Tokenade covers the same function; safe to remove (no migration — data is regenerable or telemetry-only) |
//! | [`Class::MemoryHolder`] | (currently unused) | Tokenade no longer manages a memory store (it relies on the agent's native memory), so memory tools are classed `Adjacent`, not removed. |
//! | [`Class::AntiPattern`] | `claudeslim` (breaks Anthropic prompt cache) | Known to degrade token efficiency (e.g. breaks the prompt cache); removal recommended. |
//! | [`Class::Adjacent`] | `ccusage`, `lazy-mcp`, `caveman` | Detected for the dashboard but never touched (different layer). |
//!
//! Detection signals are *additive*: a tool is "present" if ANY of its
//! signals fires (binary on PATH, MCP entry in any Claude config, hook
//! script in `~/.claude/hooks/`, data dir at a known location). This is
//! intentional — if the user uninstalled the binary but forgot to remove
//! the hook entry, we still want to clean up the hook.
//!
//! Privacy: this module READS configuration files but **never** mutates
//! them. The `tokenade install` CLI is responsible for the actual removal, and
//! always after explicit user consent.

use crate::platform;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Where a competitor's data was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    /// `which X` returned this absolute path. `installer` tells the CLI
    /// how to uninstall it (npm, brew, raw rm, …).
    Binary {
        path: PathBuf,
        installer: BinaryInstaller,
    },
    /// MCP server entry registered in this config file under this key.
    /// `config_path` is the file (~/.claude.json, ~/.codex/config, …);
    /// `entry_key` is the JSON path (`mcpServers.codegraph`, …).
    McpEntry {
        config_path: PathBuf,
        entry_key: String,
    },
    /// Hook script registered in this Claude settings file matcher this
    /// matcher (`Bash`, `Grep`, …). `command` is the literal `command`
    /// field — useful for the cleanup UX to show what would be removed.
    HookScript {
        settings_path: PathBuf,
        event: String,
        matcher: String,
        command: String,
    },
    /// On-disk data directory (typically holding the tool's memory /
    /// index). Used both as a detection signal AND as the migration
    /// source for memory-holder tools.
    DataDir { path: PathBuf, kind: DataKind },
    /// Lines the tool wrote into a rules file it SHARES with the user and
    /// with us (`@RTK.md` in `~/.claude/CLAUDE.md`, lean-ctx's block in
    /// `~/.gemini/GEMINI.md`). `open` / `close` are the exact trimmed lines
    /// that bound it — the same line when the block is one line. They
    /// outlive the binary: with the tool gone, the agent still loads them.
    RuleBlock {
        file: PathBuf,
        open: String,
        close: String,
    },
    /// A rules file the tool owns outright (`~/.claude/RTK.md`,
    /// `~/.cursor/rules/lean-ctx.mdc`).
    RuleFile { path: PathBuf },
}

/// How a binary was installed — drives how to uninstall it cleanly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BinaryInstaller {
    /// `npm install -g <pkg>` — uninstall with `npm uninstall -g`.
    NpmGlobal { package: String },
    /// `brew install <formula>` — uninstall with `brew uninstall`.
    Homebrew { formula: String },
    /// `cargo install <crate>` — uninstall with `cargo uninstall`.
    CargoInstall { krate: String },
    /// Raw `cp` to `/usr/local/bin/X` (or similar). `sudo rm` required
    /// on system paths; `rm` is enough under `~/.local/bin`.
    RawCopy { needs_sudo: bool },
}

/// What kind of data a directory holds — drives migrator selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataKind {
    /// Symbol/import graph snapshot (regenerable from source — migration
    /// is a no-op aside from re-running `tokenade index`).
    SymbolGraph,
    /// Cross-session observation log (one JSONL line per event).
    ObservationLog,
    /// Key-value memory store (we have one too — direct port).
    KeyValueMemory,
    /// Generic cache / telemetry — discardable, no migration value.
    Telemetry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// We do everything this tool does + more. Safe to suggest uninstall
    /// without migration (data is regenerable or telemetry).
    DropInBetter,
    /// Stores user data we should migrate before uninstalling. The
    /// `tokenade install` flow must run the migrator FIRST, only uninstall
    /// the source tool if migration succeeded.
    MemoryHolder,
    /// Actively harmful when co-installed with Tokenade (e.g. breaks the
    /// Anthropic prompt cache we want to ride). Strong recommendation
    /// to remove, no migration possible.
    AntiPattern,
    /// Complementary or different market. Don't touch — only surfaced
    /// in the dashboard so the user can see what was detected.
    Adjacent,
}

/// One known competitor with how to detect it and what to do with it.
pub struct Competitor {
    pub name: &'static str,
    pub class: Class,
    pub one_liner: &'static str,
    /// Why we believe the [`Class`] verdict (shown to the user in the
    /// install prompt — has to be convincing).
    pub rationale: &'static str,
    /// Run all the detectors and return every signal that fired.
    /// Empty vec → not installed.
    pub detect: fn(&DetectionCtx) -> Vec<Signal>,
}

/// Closure type for binary lookups — extracted for clippy::type_complexity.
pub type WhichFn = Box<dyn Fn(&str) -> Option<PathBuf> + Send + Sync>;
/// Closure type for JSON config reads — extracted for clippy::type_complexity.
pub type ReadJsonFn = Box<dyn Fn(&Path) -> Option<Value> + Send + Sync>;

/// What a `detect` fn has at its disposal. Passed in so tests can fake
/// the home / cwd / which-lookup behaviour without process-wide state.
pub struct DetectionCtx {
    pub home: PathBuf,
    pub cwd: PathBuf,
    /// PATH-resolved binary lookup. Default impl scans `PATH`; tests
    /// pass a closure backed by a per-test map.
    pub which: WhichFn,
    /// Read a JSON file; returns None on parse failure or missing. Tests
    /// pass a closure backed by an in-memory fixture.
    pub read_json: ReadJsonFn,
}

impl Default for DetectionCtx {
    fn default() -> Self {
        Self {
            home: platform::home_dir().unwrap_or_else(|| PathBuf::from(".")),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            which: Box::new(real_which),
            read_json: Box::new(real_read_json),
        }
    }
}

/// Default `which` — splits `PATH` on the platform separator and probes
/// each dir for an executable file.
fn real_which(binary: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let sep = if cfg!(windows) { ';' } else { ':' };
    for dir in path.to_string_lossy().split(sep) {
        if dir.is_empty() {
            continue;
        }
        for ext in [""].iter().chain(
            if cfg!(windows) {
                &[".exe", ".cmd", ".bat"][..]
            } else {
                &[][..]
            }
            .iter(),
        ) {
            let candidate = Path::new(dir).join(format!("{binary}{ext}"));
            if let Ok(meta) = std::fs::metadata(&candidate) {
                if meta.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

/// Default JSON reader — best-effort, returns None on any error.
fn real_read_json(path: &Path) -> Option<Value> {
    let s = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&s).ok()
}

/// One competitor's detection result, ready for the install UX.
pub struct DetectionResult {
    pub competitor: &'static Competitor,
    pub signals: Vec<Signal>,
}

impl DetectionResult {
    pub fn is_present(&self) -> bool {
        !self.signals.is_empty()
    }
}

/// Run every known competitor's detector and return only the ones found.
pub fn detect_all(ctx: &DetectionCtx) -> Vec<DetectionResult> {
    CATALOGUE
        .iter()
        .map(|c| DetectionResult {
            competitor: c,
            signals: (c.detect)(ctx),
        })
        .filter(|r| r.is_present())
        .collect()
}

mod catalogue;
mod detectors;
mod probes;
pub use catalogue::*;
pub(crate) use detectors::*;
pub(crate) use probes::*;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Test helper: build a `DetectionCtx` whose `which` and `read_json`
    /// are closures over the given per-test maps. No process-wide state,
    /// no test-ordering hazards, no race on parallel `cargo test`.
    fn ctx_with(
        home: PathBuf,
        which_map: BTreeMap<String, PathBuf>,
        fixtures: BTreeMap<PathBuf, Value>,
    ) -> DetectionCtx {
        DetectionCtx {
            home: home.clone(),
            cwd: home,
            which: Box::new(move |name: &str| which_map.get(name).cloned()),
            read_json: Box::new(move |path: &Path| fixtures.get(path).cloned()),
        }
    }

    #[test]
    fn rtk_detected_via_binary_and_hook_script() {
        let home = std::env::temp_dir().join("tokenade-comp-rtk-bin");
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join(".claude").join("hooks")).unwrap();
        std::fs::write(
            home.join(".claude").join("hooks").join("rtk-rewrite.sh"),
            "#!/bin/bash\n",
        )
        .unwrap();
        let which_map = BTreeMap::from([("rtk".to_string(), PathBuf::from("/usr/local/bin/rtk"))]);
        let fixtures = BTreeMap::from([(
            home.join(".claude").join("settings.json"),
            json!({
                "hooks": {
                    "PreToolUse": [{
                        "matcher": "Bash",
                        "hooks": [{"type": "command", "command": "/home/x/.claude/hooks/rtk-rewrite.sh"}]
                    }]
                }
            }),
        )]);
        let ctx = ctx_with(home.clone(), which_map, fixtures);
        let sigs = detect_rtk(&ctx);
        // Expect: binary + hook-script file + hook-script settings entry.
        assert!(sigs.len() >= 2, "got {sigs:?}");
        assert!(sigs.iter().any(|s| matches!(s, Signal::Binary { .. })));
        assert!(sigs.iter().any(|s| matches!(s, Signal::HookScript { .. })));
        let _ = std::fs::remove_dir_all(&home);
    }

    /// Binary already uninstalled: `RTK.md` + `@RTK.md` are all that is left,
    /// and they are exactly what the agent still loads every session.
    #[test]
    fn rtk_rules_import_detected_without_the_binary() {
        let home = std::env::temp_dir().join("tokenade-comp-rtk-rules");
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::write(home.join(".claude").join("RTK.md"), "# RTK\n").unwrap();
        std::fs::write(home.join(".claude").join("CLAUDE.md"), "@RTK.md\n\nmine\n").unwrap();
        let ctx = ctx_with(home.clone(), BTreeMap::new(), BTreeMap::new());
        let sigs = detect_rtk(&ctx);
        assert_eq!(
            sigs,
            vec![
                Signal::RuleBlock {
                    file: home.join(".claude").join("CLAUDE.md"),
                    open: "@RTK.md".into(),
                    close: "@RTK.md".into(),
                },
                Signal::RuleFile {
                    path: home.join(".claude").join("RTK.md"),
                },
            ]
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// lean-ctx's rules outlive its binary in five agents' globals; the
    /// heading above its marker belongs to the block.
    #[test]
    fn lean_ctx_rules_detected_in_shared_and_own_files() {
        let home = std::env::temp_dir().join("tokenade-comp-lean-ctx");
        let _ = std::fs::remove_dir_all(&home);
        let gemini = home.join(".gemini").join("GEMINI.md");
        let mdc = home.join(".cursor").join("rules").join("lean-ctx.mdc");
        std::fs::create_dir_all(gemini.parent().unwrap()).unwrap();
        std::fs::create_dir_all(mdc.parent().unwrap()).unwrap();
        std::fs::write(
            &gemini,
            "# lean-ctx — Context Engineering Layer\n<!-- lean-ctx-rules-v11 -->\nx\n<!-- /lean-ctx -->\nmine\n",
        )
        .unwrap();
        std::fs::write(&mdc, "---\n").unwrap();
        let ctx = ctx_with(home.clone(), BTreeMap::new(), BTreeMap::new());
        assert_eq!(
            detect_lean_ctx(&ctx),
            vec![
                Signal::RuleBlock {
                    file: gemini,
                    open: "# lean-ctx — Context Engineering Layer".into(),
                    close: "<!-- /lean-ctx -->".into(),
                },
                Signal::RuleFile { path: mdc },
            ]
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn codegraph_detected_via_mcp_entry() {
        let home = std::env::temp_dir().join("tokenade-comp-cg-mcp");
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let fixtures = BTreeMap::from([(
            home.join(".claude.json"),
            json!({"mcpServers": {"codegraph": {"command": "codegraph", "args": ["mcp"]}}}),
        )]);
        let ctx = ctx_with(home.clone(), BTreeMap::new(), fixtures);
        let sigs = detect_codegraph(&ctx);
        assert!(sigs.iter().any(|s| matches!(s, Signal::McpEntry { .. })));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn gsd_detected_via_claude_tree_and_sessionstart_hook() {
        let home = std::env::temp_dir().join("tokenade-comp-gsd");
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join(".claude").join("commands").join("gsd")).unwrap();
        std::fs::create_dir_all(home.join(".claude").join("skills").join("gsd-plan")).unwrap();
        std::fs::write(home.join(".claude").join("gsd-manifest.json"), "{}").unwrap();
        let fixtures = BTreeMap::from([(
            home.join(".claude").join("settings.json"),
            json!({
                "hooks": {
                    "SessionStart": [{
                        "matcher": "",
                        "hooks": [{"type": "command", "command": "node ~/.claude/hooks/gsd-check-update.js"}]
                    }]
                }
            }),
        )]);
        // No binary on purpose: GSD is npx-run, nothing lands on PATH.
        let ctx = ctx_with(home.clone(), BTreeMap::new(), fixtures);
        let sigs = detect_gsd(&ctx);
        let dirs = sigs
            .iter()
            .filter(|s| matches!(s, Signal::DataDir { .. }))
            .count();
        assert_eq!(
            dirs, 3,
            "commands dir + gsd- skill + manifest, got {sigs:?}"
        );
        assert!(
            sigs.iter()
                .any(|s| matches!(s, Signal::HookScript { event, .. } if event == "SessionStart")),
            "got {sigs:?}"
        );
        assert!(!sigs.iter().any(|s| matches!(s, Signal::Binary { .. })));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn classes_are_assigned_per_competitor() {
        for c in CATALOGUE {
            // Sanity: every entry has a non-empty rationale and one-liner.
            assert!(!c.one_liner.is_empty(), "{}: empty one-liner", c.name);
            assert!(!c.rationale.is_empty(), "{}: empty rationale", c.name);
        }
        // Class spot-checks for things we'll assert in install UX.
        let cls = |name: &str| CATALOGUE.iter().find(|c| c.name == name).unwrap().class;
        assert_eq!(cls("rtk"), Class::DropInBetter);
        // codegraph is a code graph (covered by query/impact/map), not memory.
        assert_eq!(cls("codegraph"), Class::DropInBetter);
        assert_eq!(cls("claudeslim"), Class::AntiPattern);
        assert_eq!(cls("ccusage"), Class::Adjacent);
        assert_eq!(cls("get-shit-done"), Class::Adjacent);
        // Memory tools are complementary — tokenade doesn't manage a memory
        // store, so they're never proposed for removal.
        assert_eq!(cls("claude-mem"), Class::Adjacent);
    }

    #[test]
    fn npm_shim_path_resolves_scoped_package() {
        // The codegraph install on this user's box is
        // `bin/codegraph -> ../lib/node_modules/@colbymchenry/codegraph/npm-shim.js`
        // — make sure our resolver pulls back the scoped name.
        let tmp = std::env::temp_dir().join("tokenade-comp-npm-shim");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("bin")).unwrap();
        let pkg_dir = tmp
            .join("lib")
            .join("node_modules")
            .join("@colbymchenry")
            .join("codegraph");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        let shim_target = pkg_dir.join("npm-shim.js");
        std::fs::write(&shim_target, "// shim\n").unwrap();
        #[cfg(unix)]
        let shim = tmp.join("bin").join("codegraph");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&shim_target, &shim).unwrap();
            let pkg = resolve_npm_global_package(&shim);
            assert_eq!(pkg, Some("@colbymchenry/codegraph".to_string()));
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
