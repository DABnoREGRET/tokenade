//! Response-style preamble injection (Tokenless-inspired).
//!
//! ## What this is
//!
//! Compactors shrink what the agent *reads*. They cannot shrink what the
//! agent *writes back* — that's a function of the LLM's response, and the
//! LLM only writes verbosely because nothing tells it not to. A short
//! instruction prepended to the user's prompt cuts response tokens
//! dramatically: Tokenless reports a ~80 % cut on natural chat (7 223 →
//! 1 442 tokens) with zero downstream changes.
//!
//! We expose three styles:
//!
//! | Style | When to use | Effect on the LLM |
//! |---|---|---|
//! | [`Style::Chat`] | Conversational, debugging, planning | Skip greetings/recaps/process narration. Answer directly. |
//! | [`Style::Coding`] | Heavy-edit, multi-tool workflows | Emit ASCII-dense status lines. No verbose preambles. |
//! | [`Style::Off`] | Anything where the default verbosity is wanted (e.g. user-facing docs) | No injection. |
//!
//! ## How it gets injected
//!
//! - Persisted as `~/.tokenade/style.json` so the choice survives restarts.
//! - The Claude Code `UserPromptSubmit` hook reads it and prepends
//!   [`Style::preamble`] before the user's literal prompt.
//! - Other agents (Cursor, Codex, …) don't expose an equivalent hook
//!   today; they keep the style override available via the
//!   `set_response_style` MCP tool which their planner can call once at
//!   session start.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// One of three preset response shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Style {
    /// Conversational answers, no chrome. ~80 % response-token reduction
    /// vs. the default Claude Code reply for chat workloads.
    Chat,
    /// Dense status lines for multi-tool coding sessions. The agent emits
    /// one-line state transitions instead of paragraphs.
    Coding,
    /// No injection. The agent's native verbosity. Pick it explicitly with
    /// `tokenade style off` when you never want tokenade to touch the voice.
    Off,
    /// Per-TURN auto-selection (default since 0.7.1). At each UserPromptSubmit
    /// the prompt is classified (cheap keyword heuristic, FR/EN, no LLM) into
    /// `coding` (imperative dev task), `chat` (a question / explanation), or
    /// `off` (ambiguous → inject nothing — the conservative fallback, so auto
    /// is never worse than off). See [`Style::classify`].
    #[default]
    Auto,
}

/// Classify a user prompt into the concrete preset `Auto` should apply THIS
/// turn. Keyword-scored, bilingual (FR/EN), boundary-insensitive `contains`
/// matching — cheap enough for the hot UserPromptSubmit path. Conservative:
/// returns `Off` on a tie or no signal, so auto never changes the agent's
/// voice unless the intent is clear. NOT a method on the loaded style — it
/// operates on the prompt, independent of the persisted preference.
pub fn classify(prompt: &str) -> Style {
    let p = prompt.to_lowercase();
    // Imperative dev-task signals → terse status-line `coding`.
    const CODING: &[&str] = &[
        "implement",
        "implément",
        "refactor",
        "fix ",
        "corrige",
        "débug",
        "debug",
        "compile",
        "build ",
        "ajoute",
        "add a",
        "add the",
        "write a test",
        "écris un test",
        "rename",
        "renomme",
        "migrate",
        "migration",
        "stack trace",
        "traceback",
        "the bug",
        "le bug",
        "ça compile",
        "doesn't compile",
        "ne compile",
        "patch",
        "git ",
        "cargo ",
        "npm ",
        "déploie",
        "deploy",
        "ship",
        "commit",
        "edit the",
        "fais le",
        "vas y",
        "go ahead",
        "code ",
        ".rs",
        ".py",
        ".ts",
        ".js",
        ".go",
    ];
    // Question / explanation signals → readable-but-lean `chat`.
    const CHAT: &[&str] = &[
        "explique",
        "explain",
        "c'est quoi",
        "what is",
        "what does",
        "qu'est-ce",
        "pourquoi",
        "why ",
        "comment ",
        "how do",
        "how does",
        "peut-on",
        "peut on",
        "can we",
        "can you explain",
        "should i",
        "dois-je",
        "quelle est",
        "what's the",
        "différence",
        "difference",
        "?",
    ];
    let score = |kws: &[&str]| kws.iter().filter(|k| p.contains(**k)).count();
    let c = score(CODING);
    let q = score(CHAT);
    if c > q {
        Style::Coding
    } else if q > c {
        Style::Chat
    } else {
        Style::Off
    }
}

impl Style {
    /// Stable id used in CLI / MCP arguments.
    pub fn as_str(&self) -> &'static str {
        match self {
            Style::Chat => "chat",
            Style::Coding => "coding",
            Style::Off => "off",
            Style::Auto => "auto",
        }
    }

    /// Parse from the CLI / MCP argument. Case-insensitive.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "chat" | "c" => Some(Style::Chat),
            "coding" | "code" | "d2" => Some(Style::Coding),
            "off" => Some(Style::Off),
            "auto" | "default" | "" => Some(Style::Auto),
            _ => None,
        }
    }

    /// Resolve to the concrete preset to apply for `prompt`. Identity for the
    /// fixed presets; `Auto` defers to [`classify`].
    pub fn resolve(self, prompt: &str) -> Style {
        match self {
            Style::Auto => classify(prompt),
            other => other,
        }
    }

    /// The text prepended to the user's prompt.
    ///
    /// We deliberately keep these short — every byte of preamble is a
    /// token spent on the input side. Both `chat` and `coding` are
    /// engineered to fit in roughly 50 tokens, an amortised win as soon
    /// as the response would have been longer than 50 tokens (i.e.
    /// always).
    pub fn preamble(&self) -> &'static str {
        match self {
            Style::Chat => {
                "[tokenade:style=chat] Minimise output tokens while staying human-readable. \
                 Answer directly. No greetings, no request recap, no process narration, no \
                 generic summary at the end."
            }
            Style::Coding => {
                "[tokenade:style=coding] Multi-tool coding workflow. Emit one ASCII status line \
                 per tool call (verb · target · outcome). No paragraphs of narration. \
                 Skip greetings, recaps, and trailing summaries. Group identical findings \
                 with `(×N)`."
            }
            // Off injects nothing; Auto is resolved to a concrete preset by
            // the caller (UserPromptSubmit) BEFORE `preamble`, so it never
            // reaches here as Auto — map to empty for total safety.
            Style::Off | Style::Auto => "",
        }
    }
}

/// Where the persisted choice lives. Style is a USER preference, not
/// a project trait — since L57 it lives at `~/.tokenade/style.json`
/// regardless of the project root. The `root` argument is kept for
/// API stability but ignored.
///
/// Test override: `TOKENADE_STYLE_PATH` env var points at a temp
/// file so unit tests stay hermetic.
/// Where [`save`] and [`load`] actually keep the setting.
///
/// `root` is accepted and ignored: the style is a HOME-global preference, not a
/// per-project one. Public because the CLI must print THIS path — it used to
/// print `<root>/.tokenade/style.json`, a file `save` never creates, so
/// `tokenade style chat` reported success and pointed at nothing. Measured
/// 2026-09-02: the message named the repository, the bytes landed in `$HOME`,
/// and the directory it named did not exist afterwards.
pub fn config_path(_root: &Path) -> PathBuf {
    if let Some(p) = std::env::var_os("TOKENADE_STYLE_PATH") {
        return PathBuf::from(p);
    }
    crate::platform::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".tokenade")
        .join("style.json")
}

/// Load the persisted style. Falls back to [`Style::default`] on
/// missing/corrupt files. The `root` parameter is ignored — style is
/// global since L57.
pub fn load(root: &Path) -> Style {
    fs::read_to_string(config_path(root))
        .ok()
        .and_then(|s| serde_json::from_str::<Stored>(&s).ok())
        .map(|s| s.style)
        .unwrap_or_default()
}

/// Save the chosen style at `~/.tokenade/style.json`. The `root`
/// parameter is ignored (kept for API stability with pre-L57
/// callers).
pub fn save(root: &Path, style: Style) -> std::io::Result<()> {
    let path = config_path(root);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let s = serde_json::to_string_pretty(&Stored { style }).unwrap();
    fs::write(path, s)
}

/// L57 migrator. Promote a legacy per-project
/// `<root>/.tokenade/style.json` to the global path, then delete the
/// per-project file. If both exist, the global one wins (it's the
/// authoritative location since L57); the per-project copy is just
/// removed. Returns `Some(action)` on touch, `None` otherwise.
pub fn migrate_local_style(root: &Path) -> Option<String> {
    let local = root.join(".tokenade").join("style.json");
    if !local.is_file() {
        return None;
    }
    let global = config_path(root);
    let global_exists = global.is_file();

    if !global_exists {
        if let Some(dir) = global.parent() {
            let _ = fs::create_dir_all(dir);
        }
        if let Ok(content) = fs::read_to_string(&local) {
            let _ = fs::write(&global, content);
        }
    }
    if fs::remove_file(&local).is_ok() {
        if global_exists {
            Some(".tokenade/style.json (deleted local — global already authoritative)".into())
        } else {
            Some(".tokenade/style.json (promoted to ~/.tokenade/style.json)".into())
        }
    } else {
        None
    }
}

/// Convenience: prepend the active style's preamble to `prompt`. Empty
/// preamble (i.e. `Style::Off`) returns the prompt unchanged so the
/// caller can blindly pipe through this function.
pub fn apply(root: &Path, prompt: &str) -> String {
    let preamble = load(root).preamble();
    if preamble.is_empty() {
        prompt.to_string()
    } else {
        format!("{preamble}\n\n{prompt}")
    }
}

#[derive(Serialize, Deserialize)]
struct Stored {
    style: Style,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Tests that touch the persisted style file share global state
    /// (env var + a single file on disk). Serialize them with a gate
    /// so cargo's parallel test runner can't see one test's writes
    /// while another is asserting defaults.
    fn io_lock() -> &'static Mutex<()> {
        static LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn tmp_root(label: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "tokenade-style-{}-{}-{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// Acquire the IO lock and set `TOKENADE_STYLE_PATH` to a fresh
    /// per-test file. Returns the guard so the lock is held for the
    /// duration of the caller (drop the binding to release).
    fn isolated_style_path(label: &str) -> (std::sync::MutexGuard<'static, ()>, PathBuf) {
        let guard = io_lock().lock().unwrap_or_else(|p| p.into_inner());
        let p = std::env::temp_dir().join(format!(
            "tokenade-style-{label}-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_file(&p);
        // SAFETY: env vars are process-wide; the io_lock guarantees
        // only one test reads/writes the style path at a time.
        unsafe {
            std::env::set_var("TOKENADE_STYLE_PATH", &p);
        }
        (guard, p)
    }

    #[test]
    fn classify_routes_prompts_by_intent() {
        // Imperative dev tasks → coding.
        assert_eq!(classify("implémente le compactage Grep"), Style::Coding);
        assert_eq!(classify("fix the failing test in main.rs"), Style::Coding);
        assert_eq!(classify("refactor this and commit"), Style::Coding);
        // Questions / explanations → chat.
        assert_eq!(classify("explique ce que fait tokenade style"), Style::Chat);
        assert_eq!(
            classify("peut-on choisir le style automatiquement ?"),
            Style::Chat
        );
        assert_eq!(
            classify("what is the difference between A and B?"),
            Style::Chat
        );
        // Ambiguous / social → off (conservative: no injection).
        assert_eq!(classify("ok"), Style::Off);
        assert_eq!(classify("merci"), Style::Off);
        // resolve(): Auto defers to classify, fixed presets pass through.
        assert_eq!(Style::Auto.resolve("fix the bug"), Style::Coding);
        assert_eq!(Style::Chat.resolve("fix the bug"), Style::Chat);
        assert_eq!(Style::Off.resolve("fix the bug"), Style::Off);
    }

    #[test]
    fn parses_aliases() {
        assert_eq!(Style::parse("chat"), Some(Style::Chat));
        assert_eq!(Style::parse("c"), Some(Style::Chat));
        assert_eq!(Style::parse("CODING"), Some(Style::Coding));
        assert_eq!(Style::parse("d2"), Some(Style::Coding));
        assert_eq!(Style::parse("off"), Some(Style::Off));
        assert_eq!(Style::parse("auto"), Some(Style::Auto));
        // `default` / empty now map to Auto (the new default).
        assert_eq!(Style::parse("default"), Some(Style::Auto));
        assert_eq!(Style::parse(""), Some(Style::Auto));
        assert_eq!(Style::parse("garbage"), None);
    }

    #[test]
    fn off_default_does_not_inflate_prompt() {
        let (_g, _p) = isolated_style_path("off");
        let d = PathBuf::from("/ignored-since-L57");
        let out = apply(&d, "what does the auth module do?");
        assert_eq!(out, "what does the auth module do?");
    }

    #[test]
    fn chat_prepends_preamble_and_blank_line() {
        let (_g, _p) = isolated_style_path("chat");
        let d = PathBuf::from("/ignored-since-L57");
        save(&d, Style::Chat).unwrap();
        let out = apply(&d, "what is foo?");
        assert!(out.starts_with("[tokenade:style=chat]"));
        assert!(out.ends_with("\n\nwhat is foo?"));
    }

    #[test]
    fn save_load_roundtrip() {
        let (_g, _p) = isolated_style_path("rt");
        let d = PathBuf::from("/ignored-since-L57");
        save(&d, Style::Coding).unwrap();
        assert_eq!(load(&d), Style::Coding);
    }

    #[test]
    fn corrupt_file_falls_back_to_default() {
        let (_g, p) = isolated_style_path("corrupt");
        if let Some(dir) = p.parent() {
            let _ = fs::create_dir_all(dir);
        }
        fs::write(&p, "not json at all").unwrap();
        let d = PathBuf::from("/ignored-since-L57");
        assert_eq!(load(&d), Style::Auto);
    }

    #[test]
    fn missing_file_returns_default() {
        let (_g, _p) = isolated_style_path("missing");
        let d = PathBuf::from("/ignored-since-L57");
        assert_eq!(load(&d), Style::Auto);
    }

    #[test]
    fn preambles_are_short() {
        for s in [Style::Chat, Style::Coding] {
            let approx_tokens = s.preamble().len() / 4;
            assert!(
                approx_tokens < 80,
                "preamble for {} too long: ~{} tokens",
                s.as_str(),
                approx_tokens
            );
        }
    }

    #[test]
    fn migrate_local_promotes_when_global_missing() {
        let (_g, gpath) = isolated_style_path("migrate-promote");
        // Ensure global is absent.
        let _ = fs::remove_file(&gpath);
        let root = tmp_root("migrate-promote-root");
        let local = root.join(".tokenade").join("style.json");
        fs::create_dir_all(local.parent().unwrap()).unwrap();
        fs::write(
            &local,
            serde_json::to_string(&Stored { style: Style::Chat }).unwrap(),
        )
        .unwrap();
        let action = migrate_local_style(&root);
        assert!(action.is_some());
        assert!(action.unwrap().contains("promoted"));
        assert!(!local.exists());
        // Global now holds the promoted style.
        let d = PathBuf::from("/ignored-since-L57");
        assert_eq!(load(&d), Style::Chat);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn migrate_local_deletes_when_global_already_present() {
        let (_g, gpath) = isolated_style_path("migrate-delete");
        // Set global first.
        if let Some(dir) = gpath.parent() {
            let _ = fs::create_dir_all(dir);
        }
        fs::write(
            &gpath,
            serde_json::to_string(&Stored {
                style: Style::Coding,
            })
            .unwrap(),
        )
        .unwrap();
        let root = tmp_root("migrate-delete-root");
        let local = root.join(".tokenade").join("style.json");
        fs::create_dir_all(local.parent().unwrap()).unwrap();
        fs::write(
            &local,
            serde_json::to_string(&Stored { style: Style::Chat }).unwrap(),
        )
        .unwrap();
        let action = migrate_local_style(&root);
        assert!(action.unwrap().contains("deleted local"));
        assert!(!local.exists());
        // Global value preserved (Coding wins).
        let d = PathBuf::from("/ignored-since-L57");
        assert_eq!(load(&d), Style::Coding);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn migrate_local_noop_when_nothing_to_do() {
        let (_g, _p) = isolated_style_path("migrate-noop");
        let root = tmp_root("migrate-noop-root");
        assert!(migrate_local_style(&root).is_none());
        let _ = fs::remove_dir_all(&root);
    }
}
