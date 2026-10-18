//! Feature commands: judge, read-mode, pack, review-context, mcp-stats,
//! mistake, disclose, turn-summary, receipt, cache-note, measure, execute
//! (+ their repo-walk/skill helpers). Extracted from main.rs.
#![allow(unused_imports)]
mod add_compactor;
mod execute;
mod judge_cli;
mod measure;
mod packing;
use crate::hooks::*;
use crate::wrap::*;
use crate::*;
pub(crate) use add_compactor::*;
pub(crate) use execute::*;
pub(crate) use judge_cli::*;
pub(crate) use measure::*;
pub(crate) use packing::*;

/// `tokenade read-mode` — show the active read-mode and the full ladder.
///
/// A discoverability surface for the `TOKENADE_READ_MODE` knob (see
/// `tokenade_core::read_mode`): the ladder is opt-in and otherwise invisible,
/// so this prints which rung is active, what each rung does, and how to set it.
/// `--json` emits the same facts machine-readably.
pub(crate) fn cmd_read_mode(rest: &[String]) -> i32 {
    if crate::help_routing::asks_for_help(rest) {
        println!(
            "tokenade read-mode [--json]\n\
             \n\
             Show the active read-mode and the ladder that tunes how aggressively\n\
             tokenade folds long listings / read output. Set it with the\n\
             TOKENADE_READ_MODE environment variable (opt-in; default = task).\n\
             \n\
             Ladder (terse → verbose):\n\
             \x20 aggressive  0.4× head budget — max brevity (cheap models / tight context)\n\
             \x20 task        1.0× — the balanced default (unset behaves identically)\n\
             \x20 reference   3.0× — keep the whole picture (configs, schemas, full diffs)\n\
             \x20 entropy     keep the most information-dense lines, in original order\n\
             \n\
             --json   machine-readable status."
        );
        return 0;
    }
    if let Err(code) = validate_simple_bool_flags(rest, "read-mode", &["--json"]) {
        return code;
    }
    use tokenade_core::read_mode::ReadMode;
    let active = tokenade_core::read_mode::active();
    let env_set = std::env::var("TOKENADE_READ_MODE").ok();
    let ladder = [
        ReadMode::Aggressive,
        ReadMode::Task,
        ReadMode::Reference,
        ReadMode::Entropy,
    ];

    if rest.iter().any(|a| a == "--json") {
        let rungs: Vec<serde_json::Value> = ladder
            .iter()
            .map(|m| {
                serde_json::json!({
                    "name": m.label(),
                    "head_scale": m.head_scale(),
                    "entropy": m.is_entropy(),
                    "active": *m == active,
                })
            })
            .collect();
        let out = serde_json::json!({
            "active": active.label(),
            "env_var": "TOKENADE_READ_MODE",
            "env_set": env_set,
            // A machine reader cannot see a sentence, and this is the fact the
            // sentence carries: the value was not one of the rungs.
            "env_recognised": env_set
                .as_deref()
                .map(|v| ReadMode::parse_checked(v).is_some()),
            "ladder": rungs,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&out).unwrap_or_else(|_| "{}".to_string())
        );
        return 0;
    }

    println!("tokenade read-mode");
    match &env_set {
        // A value the ladder does not know is echoed beside a mode it did not
        // choose, and that reads as agreement. `parse` folds a typo into the
        // default on purpose — no command should fail over a misspelled export
        // — so THIS is where it has to be said.
        Some(v) if ReadMode::parse_checked(v).is_none() => println!(
            "  active: {} (TOKENADE_READ_MODE={v} is not a rung — expected {}; the default stands)",
            active.label(),
            ReadMode::RUNG_SPELLINGS
        ),
        Some(v) => println!("  active: {} (TOKENADE_READ_MODE={v})", active.label()),
        None => println!(
            "  active: {} (TOKENADE_READ_MODE unset → default)",
            active.label()
        ),
    }
    println!("  ladder (terse → verbose):");
    let descr = |m: ReadMode| -> &'static str {
        match m {
            ReadMode::Aggressive => "max brevity — cheap models / tight context",
            ReadMode::Task => "balanced default — identical to pre-ladder behaviour",
            ReadMode::Reference => "keep the whole picture — configs, schemas, full diffs",
            ReadMode::Entropy => "keep the most information-dense lines, in original order",
        }
    };
    for m in ladder {
        let marker = if m == active { "→" } else { " " };
        let scale = if m.is_entropy() {
            "entropy".to_string()
        } else {
            format!("{:.1}×", m.head_scale())
        };
        println!("  {marker} {:<11} {:>7}  {}", m.label(), scale, descr(m));
    }
    println!("  set with: TOKENADE_READ_MODE=<mode>  (aliases: aggro/terse, ref/full, dense)");
    // The ladder scales a compactor's HEAD budget, and only the compactors
    // wired to it. MEASURED 2026-09-04: a 188 KB kernel log came back
    // byte-identical under all four rungs, through `read` and through
    // `filter` alike, while `git ls-files` moved 355 / 499 / 2 524 bytes
    // across aggressive / task / reference. A ladder printed with no reach
    // beside it reads as global, and the rung descriptions above ("configs,
    // schemas, full diffs") read as promises about every fold.
    //
    // Stated as something the reader can VERIFY rather than as a list that
    // would rot: a fold that honoured the mode says so in its own banner.
    println!(
        "  reach: a fold that honours the ladder stamps the mode in its banner \
         (`[tokenade:git-ls-files·aggressive]`). A banner with no mode was not scaled — \
         the ladder is wired into some compactors, not all of them."
    );
    0
}

/// `tokenade mcp-stats` — rollup over MCP server manifests + a token-aware
/// skills index. Shows how many servers are configured vs already routed
/// through tokenade, and what eager-loading every skill costs versus an
/// index-then-disclose menu.
pub(crate) fn cmd_mcp_stats(rest: &[String]) -> i32 {
    use tokenade_core::mcp_stats as ms;

    if crate::help_routing::asks_for_help(rest) {
        println!(
            "tokenade mcp-stats [--json]\n\n\
             Rollup over MCP server manifests + a token-aware skills index.\n\
             Shows configured-vs-wrapped servers and eager-load-vs-index skill cost."
        );
        return 0;
    }
    if let Err(code) = validate_simple_bool_flags(rest, "mcp-stats", &["--json"]) {
        return code;
    }
    let json_mode = rest.iter().any(|a| a == "--json");

    // Dry-wrap every known config to enumerate servers without mutating them:
    // (would-wrap + already-wrapped) = third-party MCP servers; already-wrapped
    // = the ones we route today.
    let reports = tokenade_core::mcp_wrap::wrap_all_known_configs_dry(&[]);
    let per_config: Vec<(usize, usize)> = reports
        .iter()
        .map(|r| {
            let already = r.skipped_already_wrapped.len();
            (r.wrapped.len() + already, already)
        })
        .collect();
    let roll = ms::rollup(&per_config);

    // Index every discovered skill's frontmatter.
    let mut idx = ms::SkillIndex::default();
    for path in collect_skill_files() {
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some((name, desc)) = ms::parse_skill_frontmatter(&body) {
            idx.entries.push(ms::skill_entry(name, desc));
        }
    }
    idx.entries.sort_by(|a, b| a.name.cmp(&b.name));

    if json_mode {
        let payload = serde_json::json!({
            "manifests": roll,
            "skills": {
                "count": idx.entries.len(),
                "eager_tokens": idx.eager_tokens(),
                "index_tokens": idx.index_tokens(),
                "savings": idx.savings(),
                "entries": idx.entries,
            },
        });
        println!("{}", serde_json::to_string_pretty(&payload).unwrap());
        return 0;
    }

    println!("tokenade mcp-stats");
    println!(
        "  MCP manifests: {} config(s), {} server(s), {} wrapped ({:.0}%), {} unwrapped",
        roll.configs,
        roll.servers_total,
        roll.servers_wrapped,
        roll.wrapped_fraction() * 100.0,
        roll.servers_unwrapped()
    );
    println!(
        "  skills: {} indexed · eager {} tok → index {} tok (saves {} tok on disclose)",
        idx.entries.len(),
        idx.eager_tokens(),
        idx.index_tokens(),
        idx.savings()
    );
    for e in &idx.entries {
        let desc: String = e.description.chars().take(60).collect();
        println!("    {:<24} {} tok  {}", e.name, e.tokens, desc);
    }
    0
}

/// `tokenade mistake` — a small persistent "bad direction" memory. Records
/// reverts (and manual notes) so a future session doesn't re-propose an idea
/// that was already tried and undone.
pub(crate) fn cmd_mistake(rest: &[String]) -> i32 {
    use tokenade_core::mistakes;

    let sub = rest.first().map(String::as_str);
    match sub {
        Some("list") | None => {
            // The unknown-flag sweep, one level down: `mistake list --zzbogus` listed
            // anyway at exit 0. `--json` is the only flag this branch reads,
            // and every other spelling was discarded in silence — including
            // `--jsonl` or `--json=pretty`, which a script would then parse as
            // the human table. Nothing here mutates, so the cost is a wrong
            // answer rather than a wrong action; it is still the answer the
            // rest of the CLI refuses to give.
            if let Err(c) = crate::install_cli::validate_flags_and_positionals(
                rest.iter()
                    .filter(|a| a.as_str() != "list")
                    .cloned()
                    .collect::<Vec<_>>()
                    .as_slice(),
                "mistake list",
                &["--help", "-h", "--json"],
                0,
                "",
            ) {
                return c;
            }
            let json_mode = rest.iter().any(|a| a == "--json");
            let items = mistakes::load();
            if json_mode {
                println!("{}", serde_json::to_string_pretty(&items).unwrap());
                return 0;
            }
            if items.is_empty() {
                println!("tokenade mistake: no mistakes recorded yet.");
                return 0;
            }
            println!("tokenade mistake — {} recorded", items.len());
            for m in &items {
                let r = m
                    .reference
                    .as_deref()
                    .map(|r| format!(" ({r})"))
                    .unwrap_or_default();
                println!("  [{}] {}{r}", m.kind, m.summary);
            }
            0
        }
        Some("add") => {
            let args = &rest[1..];
            let reference = get_flag(args, "--ref").map(str::to_string);
            // Summary = all positionals (everything not part of --ref).
            let mut summary_parts = Vec::new();
            let mut i = 0;
            while i < args.len() {
                if args[i] == "--ref" {
                    i += 2;
                    continue;
                }
                summary_parts.push(args[i].clone());
                i += 1;
            }
            let summary = summary_parts.join(" ");
            if summary.trim().is_empty() {
                eprintln!("usage: tokenade mistake add <summary…> [--ref <commit>]");
                return 2;
            }
            let ts = tokenade_core::platform::now_unix_secs();
            let m = mistakes::Mistake::new(ts, "manual", summary).with_reference(reference);
            if mistakes::record(&m).is_err() {
                eprintln!("tokenade mistake: failed to write ledger");
                return 1;
            }
            println!("recorded mistake: {}", m.summary);
            0
        }
        Some("from-revert") => {
            // Read git output on stdin; record a mistake iff it was a revert.
            let mut input = String::new();
            if crate::bounded_input::read_stdin_lossy(&mut input).is_err() {
                return 1;
            }
            match mistakes::revert_summary(&input) {
                Some(summary) => {
                    let ts = tokenade_core::platform::now_unix_secs();
                    let m = mistakes::Mistake::new(ts, "revert", summary);
                    if mistakes::record(&m).is_err() {
                        eprintln!("tokenade mistake: failed to write ledger");
                        return 1;
                    }
                    println!("recorded {}", m.summary);
                    0
                }
                None => {
                    println!("no revert detected — nothing recorded.");
                    0
                }
            }
        }
        Some(other) => {
            eprintln!("tokenade mistake: unknown subcommand `{other}` (list | add | from-revert)");
            2
        }
    }
}

/// `tokenade disclose` — progressive-disclosure-as-files. Stash a large blob
/// to a content-addressed file and get back a short reference; read it or
/// search inside it on demand instead of carrying it in context.
pub(crate) fn cmd_disclose(rest: &[String]) -> i32 {
    use tokenade_core::disclose;

    let sub = rest.first().map(String::as_str);
    match sub {
        Some("stash") => {
            let hint = match get_flag(&rest[1..], "--source") {
                Some("web") => disclose::SourceHint::web(),
                Some("shell") => disclose::SourceHint::Shell,
                Some("mcp") => disclose::SourceHint::Mcp,
                Some("graphql") => disclose::SourceHint::GraphQl,
                _ => disclose::SourceHint::Unknown,
            };
            let mut input = String::new();
            if crate::bounded_input::read_stdin_lossy(&mut input).is_err() {
                return 1;
            }
            // maybe_stash returns the original text when below threshold, or a
            // summary + reference when it stashed — either way, print it.
            println!("{}", disclose::maybe_stash(input, hint));
            0
        }
        Some("read") => {
            let Some(hash) = rest.get(1) else {
                eprintln!("usage: tokenade disclose read <hash>");
                return 2;
            };
            // `read_stash_by_hash` returns `(content, extension)`, in that
            // order — the `search` arm two blocks down reads it correctly as
            // `.map(|(c, _)| c)`. This arm named the first element `_meta` and
            // printed the second, so `disclose read <hash>` answered with the
            // stash file's EXTENSION and exit code 0: `json` for a 43k-token
            // JSON blob, `tab.txt` for a table, `mk` for a log. The unknown-hash
            // path was correct throughout, so the only broken case was the one
            // that looks like it worked.
            match disclose::read_stash_by_hash(hash) {
                Some((body, _ext)) => {
                    print!("{body}");
                    0
                }
                None => {
                    eprintln!("tokenade disclose: no stash for hash `{hash}`");
                    1
                }
            }
        }
        Some("search") => {
            let (Some(hash), Some(query)) = (rest.get(1), rest.get(2)) else {
                eprintln!("usage: tokenade disclose search <hash> <query>");
                return 2;
            };
            // Same retrieval core as `expand-ref --prompt` / `search-stash`:
            // small stash served whole, larger one answered by the dense RAG
            // (multi-question aware). Was BM25 line-scoring; the dense path
            // wins and keeping two would let them drift.
            let blob = tokenade_core::dedup_ref::lookup(
                &std::env::current_dir().unwrap_or_default(),
                hash,
            )
            .or_else(|| disclose::read_stash_by_hash(hash).map(|(c, _)| c));
            match blob.and_then(|t| {
                tokenade_core::gain::answer_over_blob_asked(
                    &tokenade_core::redact::redact_for_model(&t),
                    query.as_str(),
                    "disclose",
                )
            }) {
                Some(hits) => {
                    println!("{hits}");
                    0
                }
                None => {
                    eprintln!("tokenade disclose: no matches (or unknown hash `{hash}`)");
                    1
                }
            }
        }
        _ => {
            eprintln!("usage: tokenade disclose <stash|read|search> …");
            2
        }
    }
}

pub(crate) fn cmd_turn_summary(rest: &[String]) -> i32 {
    use tokenade_core::turn_summary as ts;

    if crate::help_routing::asks_for_help(rest) {
        println!(
            "tokenade turn-summary [--max N] [<file>|-]\n\n\
             Extractive summary of a conversation turn: strips filler, keeps the\n\
             top-N most content-bearing sentences in document order. Default N={}.\n\
             \n\
             Lines stating a rule are pinned first and never dropped, so a turn\n\
             made of rules returns up to {} sentences however small N is. That\n\
             floor is the only thing that exceeds --max, and the output says so\n\
             when it does.",
            ts::DEFAULT_MAX_SENTENCES,
            ts::MAX_PINNED_CONSTRAINTS
        );
        return 0;
    }

    let max = match get_flag(rest, "--max") {
        None => ts::DEFAULT_MAX_SENTENCES,
        Some(v) => match v.parse::<usize>() {
            Ok(n) if n > 0 => n,
            // #642-class: silently falling back to the default made the
            // user think their value applied.
            _ => {
                eprintln!("tokenade turn-summary: --max needs a positive integer, got '{v}'");
                return 2;
            }
        },
    };

    // Positional file (anything not a flag/flag-value) or stdin.
    let mut file: Option<&str> = None;
    let mut i = 0;
    while i < rest.len() {
        if rest[i] == "--max" {
            i += 2;
            continue;
        }
        if rest[i].starts_with("--") {
            eprintln!("tokenade turn-summary: unexpected flag `{}`", rest[i]);
            return 2;
        }
        file = Some(rest[i].as_str());
        i += 1;
    }

    let input = match file {
        Some("-") | None => {
            use std::io::IsTerminal;
            if std::io::stdin().is_terminal() {
                eprintln!("usage: tokenade turn-summary [--max N] <file>|-");
                return 2;
            }
            let mut s = String::new();
            if crate::bounded_input::read_stdin_lossy(&mut s).is_err() {
                return 1;
            }
            s
        }
        Some(path) => match crate::bounded_input::read_file_lossy(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("tokenade turn-summary: cannot read {path}: {e}");
                return 1;
            }
        },
    };

    let (summary, kept) = ts::summarize_counted(&input, max);
    let before = tokenade_core::tokenizer::estimate(&input);
    let after = tokenade_core::tokenizer::estimate(&summary);
    println!("{summary}");
    // A cap that is quietly exceeded is worse than no cap: the reader trusts
    // it the next time. Constraint lines are pinned before the budget is
    // spent and never dropped, so `--max 1` on a turn full of rules returns
    // ten sentences — as does the DEFAULT of five, which is how this went
    // unnoticed. Say it, and only when it happened.
    if kept > max {
        println!(
            "[tokenade:turn-summary] {kept} sentence(s) for --max {max}: constraint \
             lines are pinned first and never dropped, up to {}. The rest of the \
             budget is salience-ranked.",
            ts::MAX_PINNED_CONSTRAINTS
        );
    }
    if before > after {
        tokenade_core::gain::record(
            "turn-summary",
            "turn-summary",
            "conversation-turn",
            tokenade_core::Savings::new(before, after),
        );
    }
    0
}

/// `tokenade receipt` — build (or verify) a signed, tamper-evident receipt of
/// the token savings recorded in the local ledger. Stays 100% local: building
/// a receipt only reads the ledger and the per-install signing key; nothing is
/// sent anywhere. `--verify <file>` checks a receipt's signature instead.
pub(crate) fn cmd_receipt(rest: &[String]) -> i32 {
    use tokenade_core::receipt;

    if crate::help_routing::asks_for_help(rest) {
        println!(
            "tokenade receipt [--json] [--verify <file>]\n\n\
             Aggregate the local savings ledger into a small body (events, tokens\n\
             before/after/saved) and sign it with a per-install Ed25519 key so the\n\
             number is tamper-evident and portable. Output is human-readable by\n\
             default, or JSON with --json.\n\n\
             --verify <file>   verify a previously-saved receipt JSON and report\n\
                               whether its body is intact (exit 0 ok, 1 tampered).\n\n\
             Fully local: generating a receipt never contacts the network."
        );
        return 0;
    }
    if let Err(c) = crate::code_intel::refuse_all_positionals(
        rest,
        "receipt",
        &["--json", "--help", "-h"],
        &["--verify"],
    ) {
        return c;
    }

    // Verify path.
    if let Some(path) = get_flag(rest, "--verify") {
        let raw = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("tokenade receipt: cannot read {path}: {e}");
                return 1;
            }
        };
        let parsed: receipt::SignedReceipt = match serde_json::from_str(&raw) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("tokenade receipt: {path} is not a valid receipt: {e}");
                return 1;
            }
        };
        match receipt::verify(&parsed) {
            Ok(()) => {
                println!("ok — signature valid, body intact");
                println!("{}", receipt::render(&parsed));
                0
            }
            Err(e) => {
                eprintln!("FAILED — {e}");
                1
            }
        }
    } else {
        // Build path.
        let signed = match receipt::build() {
            Ok(r) => r,
            Err(e) => {
                eprintln!("tokenade receipt: cannot build receipt: {e}");
                return 1;
            }
        };
        if rest.iter().any(|a| a == "--json") {
            println!("{}", receipt::to_json(&signed));
        } else {
            println!("{}", receipt::render(&signed));
        }
        0
    }
}

/// `tokenade cache-note` — prompt-cache-aware framing of token savings: makes
/// explicit that net savings land on fresh, uncacheable output, while a cached
/// re-read bills ~flat regardless of size.
pub(crate) fn cmd_cache_note(rest: &[String]) -> i32 {
    use tokenade_core::cache_frame;

    if crate::help_routing::asks_for_help(rest) {
        println!(
            "tokenade cache-note [--saved N] [--fresh F]\n\n\
             Prompt-cache-aware framing of savings. --saved is gross tokens folded\n\
             (default: lifetime total from the gain ledger); --fresh is the fraction\n\
             that was uncacheable output, in [0,1] (default 1.0)."
        );
        return 0;
    }
    // Manual flag validation: two value flags, no positionals.
    let value_flags = ["--saved", "--fresh"];
    let mut vi = 0;
    while vi < rest.len() {
        if value_flags.contains(&rest[vi].as_str()) {
            vi += 2;
            continue;
        }
        eprintln!(
            "tokenade cache-note: unexpected argument `{}` (see --help)",
            rest[vi]
        );
        return 2;
    }

    let saved = match get_flag(rest, "--saved").map(|v| v.parse::<u64>()) {
        Some(Ok(n)) => n,
        Some(Err(_)) => {
            eprintln!("tokenade cache-note: --saved must be a non-negative integer");
            return 2;
        }
        // Default to the REALISTIC lifetime (harness-cap bounded) so the
        // cache-note framing never quotes an unbounded raw total; fall back to
        // the naive figure only if the realistic accessor has no data.
        None => tokenade_core::gain::lifetime_saved_realistic()
            .or_else(tokenade_core::gain::lifetime_saved)
            .unwrap_or(0),
    };
    let fresh = get_flag(rest, "--fresh")
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(1.0);

    println!("{}", cache_frame::disclosure(saved, fresh));
    0
}
