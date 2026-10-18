//! Dispatcher for `pkgmgr`.
//! Extracted from `filter.rs` in the L108 refactor.

use crate::compress::generic;
use crate::compress::dedup_lines;
use crate::argv::{has_flag, has_any_flag, flag_value};
use crate::ansi::strip_ansi;

pub fn dispatch(argv: &[String], output: &str) -> String {
    if has_flag(argv, "--json") || flag_value(argv, "--output").is_some_and(|v| v == "json") {
        let (_, c, _) = crate::auto::compact(output);
        return c;
    }
    if has_any_flag(argv, &["-q", "--quiet", "--silent"]) {
        return dedup_lines(&strip_ansi(output));
    }
    // Per-tool shapes BEFORE the generic detector. `pip install -r` prints a
    // `Collecting …` line per package — hundreds of distinct lines that `auto`
    // has no reason to recognise, so a failed resolve came back at two thirds
    // of its raw size with the conflict buried in it. The compactor for this
    // shape already existed; nothing routed to it.
    let prog = argv
        .first()
        .map(|p| p.rsplit(['/', '\\']).next().unwrap_or(p).to_string())
        .unwrap_or_default();
    if matches!(prog.as_str(), "pip" | "pip3") {
        if let Some(o) = crate::cmd::linux::pip::try_compact(output) {
            return o;
        }
    }
    // `pacman -Ql` / `-Fl`: the files a package owns, every name kept.
    let lists_files = argv.iter().skip(1).any(|a| {
        a == "--list" || (a.starts_with('-') && !a.starts_with("--") && a.contains('l') && (a.contains('Q') || a.contains('F')))
    });
    if prog == "pacman" && lists_files {
        if let Some(o) = crate::cmd::linux::pacman_files::try_compact(output) {
            return o;
        }
    }
    // `pacman -Sl [repo]`: every package of the repository.
    if prog == "pacman" && argv.iter().skip(1).any(|a| a.starts_with('-') && !a.starts_with("--") && a.contains('S') && a.contains('l')) {
        if let Some(o) = crate::cmd::linux::pacman_sync_list::try_compact(output) {
            return o;
        }
    }
    // `pacman -Qi` / `-Si`: one block per package.
    let shows_info = argv.iter().skip(1).any(|a| {
        a.starts_with('-') && !a.starts_with("--") && a.contains('i') && (a.contains('Q') || a.contains('S'))
    });
    if prog == "pacman" && shows_info {
        if let Some(o) = crate::cmd::linux::pacman_info::try_compact(output) {
            return o;
        }
    }
    // Bundler prints `Using <gem> <version>` for every gem already satisfied —
    // hundreds of lines on any real Rails app, and the resolver conflict that
    // made you run it sits underneath.
    if matches!(prog.as_str(), "bundle" | "bundler") {
        if let Some(o) = fold_gem_lines(output) {
            return o;
        }
    }
    // Per-SUBCOMMAND modules, AFTER every hand-written per-tool block above.
    // Placed first at the start and that was wrong: `bundle install` composes
    // the key `bundle_install`, which resolves, shrinks, and returned before
    // `fold_gem_lines` ever ran — the composed guess beat the compactor
    // written for that exact shape. A guess belongs behind everything that
    // knows.
    //
    // Same finding as the `pip` block above, one
    // level down: `brew_outdated`, `brew_services`, `snap_list`, `dnf_search`
    // and friends all exist as modules and none of them had a route, because
    // this dispatcher only ever looked at the program. Measured 2026-09-02:
    // `tokenade filter brew outdated` on a 120-row listing returned the input
    // unchanged, while `tokenade filter brew_outdated` — the module name typed
    // as if it were a program — folded it by 48%.
    //
    // Composed rather than enumerated, and safe to guess: every `try_compact`
    // opens with `if !detect(text) { return None }`, so a subcommand that has
    // no module, or one whose output does not look like what it expects, costs
    // exactly one failed table lookup. The result is taken only when it
    // actually shrinks.
    if let Some(sub) = argv
        .iter()
        .skip(1)
        .find(|t| !t.starts_with('-'))
        .map(|t| t.as_str())
    {
        let key = format!("{prog}_{}", sub.replace('-', "_"));
        if let Some(o) = crate::cmd::dispatch_named(&key, output) {
            if o.len() < output.len() {
                return o;
            }
        }
    }
    let (_, c, _) = crate::auto::compact(output);
    if c.len() < output.len() {
        c
    } else {
        generic(output)
    }
}

/// `Using rails 7.1.3` — one per gem bundler did not have to touch.
///
/// Requires EXACTLY `<verb> <name> <version-ish>`: three whitespace-separated
/// words, the last starting with a digit. A message line has more words or no
/// version, so `Bundler could not find compatible versions for gem "rails"`
/// and everything under it survives whole.
fn is_gem_progress(line: &str) -> bool {
    let t = line.trim();
    let mut w = t.split_whitespace();
    let (Some(verb), Some(_name), Some(ver), None) = (w.next(), w.next(), w.next(), w.next())
    else {
        return false;
    };
    matches!(verb, "Using" | "Fetching" | "Installing")
        && ver.starts_with(|c: char| c.is_ascii_digit())
}

fn fold_gem_lines(output: &str) -> Option<String> {
    const MIN: usize = 20;
    let mut kept: Vec<&str> = Vec::new();
    let mut folded = 0usize;
    for line in output.lines() {
        if is_gem_progress(line) {
            folded += 1;
        } else {
            kept.push(line);
        }
    }
    if folded < MIN {
        return None;
    }
    let mut out = format!("[tokenade:bundle] {folded} gem(s) resolved without change\n");
    out.push_str(kept.join("\n").trim_end());
    Some(out.trim_end().to_string())
}

#[cfg(test)]
mod tests {

    #[test]
    fn bundler_folds_its_gem_list_but_keeps_the_conflict() {
        let mut input: String = (0..500).map(|i| format!("Using gem{i} 1.0.{i}\n")).collect();
        input.push_str("Bundler could not find compatible versions for gem \"rails\":\n");
        input.push_str("  In Gemfile:\n    rails (~> 7.1)\n");
        let out = dispatch(&argv(&["bundle", "install"]), &input);
        assert!(out.contains("500 gem(s) resolved"), "got:\n{out}");
        assert!(out.contains("could not find compatible versions"), "the conflict survives:\n{out}");
        assert!(out.contains("rails (~> 7.1)"), "and its detail:\n{out}");
    }

    #[test]
    fn a_message_line_is_never_taken_for_gem_progress() {
        for line in [
            "Bundler could not find compatible versions",
            "Using the local copy",
            "Installing nokogiri 1.16.0 with native extensions",
        ] {
            assert!(!super::is_gem_progress(line), "must not fold: {line}");
        }
        assert!(super::is_gem_progress("Using rails 7.1.3"));
    }
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn json_flag_routes_to_auto() {
        // `dnf --json` / `apt list --json` style.
        let body = r#"{"packages":[{"name":"foo","version":"1.0","arch":"x86_64"},{"name":"bar","version":"2.0","arch":"noarch"}]}"#;
        let out = dispatch(&argv(&["dnf", "list", "--json"]), body);
        assert!(out.contains("packages") || out.contains("foo"));
    }

    #[test]
    fn output_json_value_routes_to_auto() {
        // `dnf --output json` (with value, not just flag).
        let body = r#"{"packages":[{"name":"x","version":"1"}]}"#;
        let out = dispatch(&argv(&["dnf", "list", "--output", "json"]), body);
        assert!(out.contains("packages") || out.contains("name"));
    }

    #[test]
    fn quiet_flag_strips_and_dedups() {
        // -q / --quiet / --silent: strip ANSI + dedup runs.
        let body = "\x1b[33mwarning\x1b[0m\n\
                    repeated\nrepeated\nrepeated\n\
                    installed pkg-1.0\n";
        for flag in &["-q", "--quiet", "--silent"] {
            let out = dispatch(&argv(&["apt", flag, "install", "pkg"]), body);
            assert!(!out.contains("\x1b["), "{flag} must strip ANSI");
            assert_eq!(
                out.matches("repeated").count(),
                1,
                "{flag} must dedup; got: {out:?}"
            );
            assert!(out.contains("installed pkg"));
        }
    }

    #[test]
    fn default_auto_wins_on_structured_input() {
        // No flags, content IS JSON → auto's JSON compactor wins on
        // length and returns the structured compaction.
        let body = format!(
            "{{\"packages\":[{}]}}\n",
            (0..50).map(|i| format!("{{\"id\":{i},\"name\":\"p{i}\"}}")).collect::<Vec<_>>().join(",")
        );
        let out = dispatch(&argv(&["dnf", "list"]), &body);
        // JSON compactor folds the array into a lossless TOON block.
        assert!(out.contains("packages") && out.contains("toon"));
    }

    #[test]
    fn default_falls_back_to_generic_on_prose() {
        // No flags, prose-y content with repeats → auto can't shrink →
        // dispatcher uses generic (which dedups).
        let body = "Reading package lists...\n\
                    Building dependency tree...\n\
                    Reading state information...\n\
                    Reading state information...\n\
                    Reading state information...\n\
                    The following NEW packages will be installed:\n  foo\n";
        let out = dispatch(&argv(&["apt", "install", "foo"]), body);
        assert!(!out.is_empty());
        // generic's dedup collapses the 3-line "Reading state" run.
        assert_eq!(
            out.matches("Reading state information").count(),
            1,
            "generic fallback must dedup; got: {out:?}"
        );
        assert!(out.contains("NEW packages") || out.contains("foo"));
    }
}
