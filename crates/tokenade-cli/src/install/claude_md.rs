#![allow(unused_imports)]
//! Mechanical module split of install.rs (no behaviour change). Wired
//! back into the `install` namespace by mod.rs via `pub(crate) use <name>::*;`.
use super::*;
use crate::hooks::*;
use crate::wrap::*;
use crate::*;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) const GLOBAL_CLAUDE_MD_FENCE_START: &str = "<!-- tokenade:start -->";

/// Used by `tokenade uninstall` to find & remove the block surgically.
/// Wired in #552 — the strip path in `strip_global_claude_md_block`
/// matches `<start>…<end>` and rewrites the file without it. Kept as
/// a const so install and uninstall agree on the literal.
pub(crate) const GLOBAL_CLAUDE_MD_FENCE_END: &str = "<!-- tokenade:end -->";

pub(crate) const GLOBAL_CLAUDE_MD_BLOCK: &str = "\
<!-- tokenade:start -->
## Tokenade is installed

The `tokenade` CLI is available globally. Its usage rules are loaded from this machine's agent config — if the Tokenade instruction is already in the context, do not open another copy.
<!-- tokenade:end -->
";

/// Offer to append the canonical Tokenade-preference block to the user's
/// global `~/.claude/CLAUDE.md`. NEVER writes without `Decision::Yes`.
/// Idempotent: if the block is already present (looking for the start
/// fence), we skip silently.
/// #1036 — silent in-place refresh of the tokenade-fenced block in
/// `~/.claude/CLAUDE.md`. Extracted from `propose_global_claude_md_block`
/// so the auto-updater's post-upgrade hook can run it without ever
/// going through the prompt branch (which would only fire when fences
/// are absent — i.e. user never installed or uninstalled, NOT the
/// post-upgrade case).
///
/// Returns:
///   - `Some(true)` when fences are present and the block was rewritten,
///   - `Some(false)` when fences are present and the block was already up-to-date,
///   - `None` when fences are absent (caller decides what to do — the
///     install path prompts, the post-upgrade path does nothing).
///
/// Always silent on the up-to-date case (no `println` noise during
/// background runs); narrates on actual rewrites + dry-run plans.
pub(crate) fn refresh_global_claude_md_block_in_place(
    home: Option<&Path>,
    dry_run: bool,
) -> Option<bool> {
    let home = home?;
    let path = tokenade_core::platform::claude_config_dir_for(home).join("CLAUDE.md");
    let existing = fs::read_to_string(&path).ok()?;
    let start = existing.find(GLOBAL_CLAUDE_MD_FENCE_START)?;
    let end_marker = existing.find(GLOBAL_CLAUDE_MD_FENCE_END)?;
    if end_marker <= start {
        // Malformed (end before start) — treat as no fences, let caller
        // re-install cleanly via the prompt path.
        return None;
    }
    let end = end_marker + GLOBAL_CLAUDE_MD_FENCE_END.len();
    let current = &existing[start..end];
    if current.trim() == GLOBAL_CLAUDE_MD_BLOCK.trim() {
        return Some(false); // already up to date — silent
    }
    if dry_run {
        eprintln!(
            "    [dry-run] would refresh tokenade block in {} (content changed)",
            path.display()
        );
        return Some(false);
    }
    // Backup before rewrite, mirrors the append path in the caller.
    crate::diagnostics_cli::backup_file(&path);
    let new_content = format!(
        "{}{}{}",
        &existing[..start],
        GLOBAL_CLAUDE_MD_BLOCK.trim_end(),
        &existing[end..]
    );
    match crate::write_text_atomic(&path, new_content.as_ref()) {
        Ok(()) => {
            eprintln!(
                "    ✓ refreshed tokenade block in {} (content updated)",
                path.display()
            );
            Some(true)
        }
        Err(e) => {
            eprintln!("    ✗ refresh failed: {e}");
            Some(false)
        }
    }
}

pub(crate) fn propose_global_claude_md_block(
    home: Option<&Path>,
    flags: tokenade_core::prompt::ConsentFlags,
) {
    let Some(home) = home else {
        return;
    };
    let path = tokenade_core::platform::claude_config_dir_for(home).join("CLAUDE.md");
    // Already-installed handling — #560 splice path now extracted to
    // `refresh_global_claude_md_block_in_place` (#1036) so the auto-
    // updater's post-upgrade hook can call it silently without going
    // through the prompt branch.
    // Fences present + handled (either no-op or refreshed) → done.
    // Fences absent → fall through to the append prompt.
    if refresh_global_claude_md_block_in_place(Some(home), flags.dry_run).is_some() {
        return;
    }
    // 1.1.19: the CLI lesson lives in this file. An install after the
    // post-upgrade refresh must not paste the short fence next to it —
    // including when the lesson was written WITHOUT a marker (stacking).
    if path
        .exists()
        .then(|| fs::read_to_string(&path).ok())
        .flatten()
        .is_some_and(|s| s.contains("<!-- tokenade-scaffold -->") || s.contains("tokenade map"))
    {
        return;
    }

    eprintln!();
    eprintln!("── Global Claude Code config ──");
    eprintln!(
        "Tokenade ships better-than-default tools, but Claude Code won't know to prefer them"
    );
    eprintln!(
        "unless your global `~/.claude/CLAUDE.md` says so. We can append the following block"
    );
    eprintln!("(fenced with `<!-- tokenade:start -->` / `<!-- tokenade:end -->` so it can be");
    eprintln!("removed cleanly later):");
    eprintln!();
    for line in GLOBAL_CLAUDE_MD_BLOCK.lines() {
        eprintln!("    {line}");
    }
    eprintln!();
    let question = format!(
        "Append the block above to {}? (Won't touch any existing content; backup taken.)",
        path.display()
    );
    let decision = tokenade_core::prompt::ask(&question, false, flags);
    if decision == tokenade_core::prompt::Decision::No {
        eprintln!("    skipped — to do it manually later: copy the block above into the file");
        return;
    }
    if flags.dry_run {
        eprintln!("    [dry-run] would append to {}", path.display());
        return;
    }
    // Make sure ~/.claude/ exists, back up if a file is already there,
    // append (creating the file if needed). Errors are reported but
    // never panic.
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if path.exists() {
        crate::diagnostics_cli::backup_file(&path);
    }
    // A CLAUDE.md that EXISTS but cannot be decoded (latin-1 prose, a bad
    // mode) must not read as empty here: everything below appends to
    // `new_content` and writes it back, so an empty read REPLACES the user's
    // whole global instructions file with our block — and then says
    // "✓ appended to". `write_atomic` keeps a pristine copy, so this was
    // recoverable, but silently and only for someone who knew to look.
    let mut new_content = match super::json_util::read_text_or_bail(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("    ✗ {e}");
            return;
        }
    };
    if !new_content.is_empty() && !new_content.ends_with('\n') {
        new_content.push('\n');
    }
    if !new_content.is_empty() {
        new_content.push('\n');
    }
    new_content.push_str(GLOBAL_CLAUDE_MD_BLOCK);
    match crate::write_text_atomic(&path, new_content.as_ref()) {
        Ok(()) => eprintln!("    ✓ appended to {}", path.display()),
        Err(e) => eprintln!("    ✗ write failed: {e}"),
    }
}

/// #552 — counterpart of `propose_global_claude_md_block`. Strip the
/// fenced tokenade block from `~/.claude/CLAUDE.md` so a clean
/// uninstall actually leaves no trace in the user's global agent
/// config. No-op when the file is absent or contains no fences. The
/// fences AND everything between them are removed; one preceding
/// blank line is also trimmed so we don't leave a hole in the file.
/// A backup is taken before the rewrite, mirroring the install path.
pub(crate) fn strip_global_claude_md_block(home: Option<&Path>, dry_run: bool) -> StripOutcome {
    let Some(home) = home else {
        return StripOutcome::NoHome;
    };
    let path = tokenade_core::platform::claude_config_dir_for(home).join("CLAUDE.md");
    let Ok(existing) = fs::read_to_string(&path) else {
        return StripOutcome::AbsentOrUnreadable;
    };
    let Some(start) = existing.find(GLOBAL_CLAUDE_MD_FENCE_START) else {
        return StripOutcome::NoBlock;
    };
    let Some(end_marker) = existing[start..].find(GLOBAL_CLAUDE_MD_FENCE_END) else {
        // Opening fence without a closing one — refuse to guess at
        // boundaries; safer to leave the file alone.
        return StripOutcome::MalformedFences;
    };
    let end = start + end_marker + GLOBAL_CLAUDE_MD_FENCE_END.len();
    // Trim one trailing newline that came right after the end fence
    // (so the file doesn't grow a blank gap), plus one preceding
    // blank line if the prefix ends with "\n\n" (so the visual
    // separator we added at install time goes away too).
    let mut new_end = end;
    if existing.as_bytes().get(new_end) == Some(&b'\n') {
        new_end += 1;
    }
    let mut new_start = start;
    // `.get(..)` (not `[..]`): the byte before the fence can be multi-byte
    // (e.g. `é\n<!-- tokenade:start -->`) — a raw slice landing mid-codepoint
    // panicked during `tokenade uninstall`.
    while new_start >= 2 && existing.get(new_start - 2..new_start) == Some("\n\n") {
        new_start -= 1;
    }
    let new_content = format!("{}{}", &existing[..new_start], &existing[new_end..]);
    if dry_run {
        return if new_content.trim().is_empty() {
            StripOutcome::WouldRemoveEmpty {
                path,
                bytes_removed: existing.len(),
            }
        } else {
            StripOutcome::WouldStrip {
                path,
                bytes_removed: existing.len() - new_content.len(),
            }
        };
    }
    // Backup, then atomic rewrite. Best-effort backup — never block
    // the strip on it.
    crate::diagnostics_cli::backup_file(&path);
    // The block WAS the whole file: `install` creates `~/.claude/CLAUDE.md`
    // when the machine has none, so stripping it back leaves a 0-byte file
    // that never existed before — litter from the one command whose job is to
    // leave no trace. Remove it instead. A file the user had written in
    // himself is not affected: anything but whitespace survives the strip and
    // takes the normal rewrite below, byte-identical to what he wrote. The
    // backup above holds the pre-strip content either way.
    if new_content.trim().is_empty() {
        return match fs::remove_file(&path) {
            Ok(()) => StripOutcome::RemovedEmpty {
                path,
                bytes_removed: existing.len(),
            },
            Err(e) => StripOutcome::WriteFailed(e.to_string()),
        };
    }
    match crate::write_text_atomic(&path, new_content.as_ref()) {
        Ok(()) => StripOutcome::Stripped {
            path,
            bytes_removed: existing.len() - new_content.len(),
        },
        Err(e) => StripOutcome::WriteFailed(e.to_string()),
    }
}

/// Discriminated outcome of `strip_global_claude_md_block` so the
/// caller can render a consistent message in install / uninstall /
/// dry-run paths without leaking io::Error types.
pub(crate) enum StripOutcome {
    /// The block was the whole file, so the file itself was removed rather
    /// than left behind empty. Distinct from `Stripped` because the two are
    /// different acts and the caller must not report one as the other.
    RemovedEmpty {
        path: std::path::PathBuf,
        bytes_removed: usize,
    },
    /// `RemovedEmpty` under `--dry-run`.
    WouldRemoveEmpty {
        path: std::path::PathBuf,
        bytes_removed: usize,
    },
    NoHome,
    AbsentOrUnreadable,
    NoBlock,
    MalformedFences,
    WouldStrip {
        path: PathBuf,
        bytes_removed: usize,
    },
    Stripped {
        path: PathBuf,
        bytes_removed: usize,
    },
    WriteFailed(String),
}

/// Outcome of writing the per-project CLAUDE.md scaffold.
pub(crate) enum ClaudeMdOutcome {
    Wrote(Vec<PathBuf>),
    Skipped(String),
    Failed(String),
}

/// Idempotent scaffold of a minimal startup-file layout for AI coding agents.
///
/// Writes (never overwrites):
///   - `CLAUDE.md`                  rules + framework hint
///   - `.claude/QUICK_START.md`     common project commands placeholder
///   - `.claude/ARCHITECTURE_MAP.md` where-things-live placeholder
///   - `.claude/COMMON_MISTAKES.md` top-5 bugs placeholder
///   - `.claudeignore`              keep old docs / session archives out of auto-load
///
/// Content is brand-neutral, opinionated about brevity, and adapted to the
/// detected framework. If any target already exists we leave it alone — we
/// never clobber the user's own rules.
///
/// `profile` selects an optional domain-tailored section appended to
/// the universal `CLAUDE.md` body. `None` = universal rules only,
/// `Some("coding"|"agents"|"analysis"|"benchmark")` adds the
/// matching section (a brevity-rules
/// `profiles/` directory). Unknown profile names fall through to
/// universal rules — agents shouldn't fail an install just because
/// the user typo'd the flag.
///
/// #554 — the parameterless back-compat shim
/// `write_claude_md_scaffold` that forwarded here with `None` was
/// removed; every caller now invokes this function directly.
pub(crate) fn write_claude_md_scaffold_with_profile(
    root: &Path,
    fw: framework::Framework,
    brevity: bool,
    profile: Option<&str>,
) -> ClaudeMdOutcome {
    let claude_dir = root.join(".claude");
    if let Err(e) = fs::create_dir_all(&claude_dir) {
        return ClaudeMdOutcome::Failed(format!("mkdir {}: {e}", claude_dir.display()));
    }

    let mut wrote: Vec<PathBuf> = Vec::new();
    let mut already: Vec<PathBuf> = Vec::new();

    let claude_md = match profile {
        Some(p) => {
            let mut base = claude_md_template(fw, brevity);
            base.push('\n');
            base.push_str(profile_block(p));
            base
        }
        None => claude_md_template(fw, brevity),
    };
    let files: &[(&str, String)] = &[
        ("CLAUDE.md", claude_md),
        (".claude/QUICK_START.md", quick_start_template(fw)),
        (".claude/ARCHITECTURE_MAP.md", architecture_map_template(fw)),
        (".claude/COMMON_MISTAKES.md", common_mistakes_template(fw)),
        (".claudeignore", claudeignore_template()),
    ];
    for (rel, content) in files {
        let path = root.join(rel);
        if path.exists() {
            already.push(path);
            continue;
        }
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Err(e) = crate::write_text_atomic(&path, content.as_ref()) {
            return ClaudeMdOutcome::Failed(format!("write {}: {e}", path.display()));
        }
        wrote.push(path);
    }
    if wrote.is_empty() {
        ClaudeMdOutcome::Skipped(format!(
            "all {} target files already exist (no overwrite)",
            already.len()
        ))
    } else {
        ClaudeMdOutcome::Wrote(wrote)
    }
}

/// The root CLAUDE.md template. Compact (target ~450 tokens), framework-aware.
/// Profile-specific CLAUDE.md sections.
/// Each profile EXTENDS the universal rules — we never replace, only
/// append, so users opting into `--profile coding` still get the
/// brevity rules above.
pub(crate) fn profile_block(profile: &str) -> &'static str {
    match profile {
        "coding" => "\n## Coding profile\n\n\
            - Return code first. Explanation only when non-obvious.\n\
            - Comments only where logic isn't self-evident.\n\
            - No docstrings/type annotations on code not being changed.\n\
            - No error handling for scenarios that cannot happen.\n\
            - Read the file before modifying it. Never edit blind.\n\
            - State the bug. Show the fix. Stop.\n\
            - No suggestions beyond the scope of the review.\n",
        "agents" => "\n## Agents profile (pipelines / bots / scheduled tasks)\n\n\
            - Structured output only — JSON, bullets, tables.\n\
            - No prose unless the downstream consumer is human.\n\
            - Every output must be parseable without post-processing.\n\
            - Execute the task. Do not narrate (\"Now I will…\").\n\
            - No confirmation prompts on clearly defined tasks. Use defaults.\n\
            - On failure: state what failed, why, what was attempted. Stop.\n\
            - Never invent file paths, API endpoints, function names. Return `null` / `\"UNKNOWN\"` when unsure.\n",
        "analysis" => "\n## Analysis profile (data / research / reporting)\n\n\
            - Lead with the finding. Context after.\n\
            - Tables and bullets over prose.\n\
            - Numbers include units. Never ambiguous values.\n\
            - Never state a number without source or derivation.\n\
            - If data is missing: say so. Do not estimate silently.\n\
            - Label inferences explicitly (\"Based on the trend…\").\n\
            - Never fabricate statistics or citations.\n",
        "benchmark" => "\n## Benchmark profile (token-to-green coding evals)\n\n\
            - Read existing files before writing code.\n\
            - Be concise in output. No closing fluff.\n\
            - Prefer editing over rewriting whole files.\n\
            - Test your code before declaring done.\n\
            - Keep solutions simple and direct.\n\
            - Deliver exactly what was requested. No extras.\n",
        _ => "",
    }
}

pub(crate) fn claude_md_template(fw: framework::Framework, brevity: bool) -> String {
    let stack = fw.as_str();
    // Brevity preamble — distilled from sweep #2 readings and our
    // own benchmark observations. Each rule maps to a concrete failure
    // mode we measured: filler openers cost 8-20 tokens per response,
    // re-reading files costs 1-3k tokens per repeated Read, etc.
    let brevity_block = if brevity {
        "\n## Response style\n\n\
         - Default to terse. Skip filler openers (\"Sure!\", \"Great question!\") and closers (\"Hope this helps!\").\n\
         - No restating the question. Answer it. Tool first, result first.\n\
         - One-line code comments only when the WHY is non-obvious.\n\
         - Use Markdown sparingly — bullets when needed, prose otherwise.\n\
         - Match response length to the task: trivial → one sentence; complex → only as much as the task requires.\n\
         - No em dashes, smart quotes, or Unicode characters that break parsers. Plain ASCII unless content requires otherwise.\n\
         - No sycophancy. Do not say \"You're absolutely right!\" — disagree and explain when warranted.\n\n\
         ## Working style\n\n\
         - Read existing files before writing code. Never edit blind.\n\
         - Do not re-read files you have already read this session unless they may have changed.\n\
         - State assumptions explicitly. If uncertain, ask rather than guess.\n\
         - Push back when a simpler approach exists. Surface tradeoffs.\n\
         - Three similar lines is better than a premature abstraction.\n\
         - No speculative features. Deliver exactly what was requested.\n"
    } else {
        ""
    };
    format!(
        "# Project rules for AI coding agents\n\n\
Tech stack: {stack}\n\n\
## Conventions\n\n\
- **Fix root causes, not symptoms.** When a bug looks fixable by a small patch, ask whether the patch addresses the underlying mechanism or merely suppresses the visible failure. Spend a minute auditing the cause before reaching for the patch; only paper over a symptom when the real fix is genuinely out of scope.\n\
- Read code before changing it. Run tests after.\n\
- Edit existing files; don't create new ones unless required.\n\
- No commented-out code in commits.\n\
- Match the surrounding style; don't reformat untouched lines.\n\
- **For codebase questions, default to the tokenade CLI — `tokenade semantic \"…\"`, `tokenade query <name>`, `tokenade map`, or `tokenade skeleton <file>` — including from inside spawned subagents.** Fall back to `grep` / `find` / whole-file `Read` only when the query genuinely doesn't fit a structured shape (e.g. searching log files, exact-byte strings). This applies to any Agent / Task call you delegate.\n\
- For \"who calls X\" use `tokenade query <name>` to locate the symbol and its references from the index, not a grep.\n\
- For noisy commands, use `tokenade wrap '<cmd>'` so output is compressed.\n\
{brevity_block}\n\
## Layout\n\n\
- `.claude/QUICK_START.md`     — common project commands\n\
- `.claude/ARCHITECTURE_MAP.md` — where things live\n\
- `.claude/COMMON_MISTAKES.md`  — top 5 bugs we keep hitting\n\
- `.claudeignore`               — paths the agent should NOT auto-load\n"
    )
}

/// `.claude/QUICK_START.md` — placeholder commands per framework.
pub(crate) fn quick_start_template(fw: framework::Framework) -> String {
    use framework::Framework as F;
    let cmds = match fw {
        F::NextJs | F::Remix | F::Astro => "npm install\nnpm run dev\nnpm test\nnpm run build",
        F::Nuxt => "npm install\nnpm run dev\nnpm test\nnpm run build",
        F::SvelteKit | F::Svelte => "npm install\nnpm run dev\nnpm test\nnpm run build",
        F::NestJs => "npm install\nnpm run start:dev\nnpm test\nnpm run build",
        F::Express | F::Fastify => "npm install\nnpm run dev\nnpm test",
        F::React | F::Vue => "npm install\nnpm run dev\nnpm test",
        F::Django => {
            "pip install -r requirements.txt\npython manage.py runserver\npython manage.py test"
        }
        F::Flask => "pip install -r requirements.txt\nflask run\npytest -q",
        F::FastApi => "pip install -r requirements.txt\nuvicorn app.main:app --reload\npytest -q",
        F::Rails => "bundle install\nbin/rails server\nbin/rails test",
        F::Sinatra => "bundle install\nbundle exec ruby app.rb\nbundle exec rspec",
        F::SpringBoot => "./mvnw spring-boot:run\n./mvnw test",
        F::Laravel => "composer install\nphp artisan serve\nphp artisan test",
        F::Symfony => "composer install\nsymfony serve\nphp bin/phpunit",
        F::AspNet => "dotnet restore\ndotnet run\ndotnet test",
        F::Phoenix => "mix deps.get\nmix phx.server\nmix test",
        F::Axum | F::Actix | F::Rocket => "cargo run\ncargo test --workspace",
        F::Unknown => "# Add your project's common commands here.",
    };
    format!(
        "# Quick start\n\nCommands the agent runs most often.\n\n```sh\n{cmds}\n```\n\n\
Add project-specific scripts here so the agent doesn't have to discover them.\n"
    )
}

/// `.claude/ARCHITECTURE_MAP.md` — minimal placeholder; run `tokenade map` to populate.
pub(crate) fn architecture_map_template(fw: framework::Framework) -> String {
    format!(
        "# Architecture map\n\n\
One-paragraph orientation: who calls what, where the entry points live.\n\n\
Detected framework: **{}**.\n\n\
Suggested next step: run `tokenade map` and paste the most useful lines below.\n\n\
## Entry points\n\n\
- TODO\n\n\
## Hot paths\n\n\
- TODO\n",
        fw.as_str()
    )
}

/// `.claude/COMMON_MISTAKES.md` — empty by design; grows with the team's scars.
pub(crate) fn common_mistakes_template(fw: framework::Framework) -> String {
    let starter = match fw {
        framework::Framework::Django | framework::Framework::Rails | framework::Framework::Laravel => "1. ORM N+1 queries when serialising lists. Use `select_related`/`includes`/`with`.\n",
        framework::Framework::NextJs => "1. Server-component vs client-component boundaries. Marking everything `\"use client\"` defeats RSC.\n",
        framework::Framework::React | framework::Framework::Vue => "1. Inline object/array props cause re-renders. Memoise or move out of render.\n",
        framework::Framework::SpringBoot => "1. Forgetting `@Transactional` on multi-step writes. Inconsistent state on failure.\n",
        _ => "1. (Add the bugs your team keeps hitting here.)\n",
    };
    format!(
        "# Common mistakes\n\n\
Top 5 bugs we keep hitting. Add to this list whenever a regression bites you.\n\n\
{starter}2. TODO\n3. TODO\n4. TODO\n5. TODO\n"
    )
}

/// `.claudeignore` — paths the agent should NEVER auto-load.
pub(crate) fn claudeignore_template() -> String {
    "# Don't let the agent auto-load these (saves startup tokens).\n\
node_modules/\n\
.next/\n\
dist/\n\
build/\n\
target/\n\
__pycache__/\n\
.venv/\n\
vendor/\n\
.tokenade/cache/\n\
.claude/sessions/\n\
.claude/completions/\n\
docs/archive/\n"
        .to_string()
}
