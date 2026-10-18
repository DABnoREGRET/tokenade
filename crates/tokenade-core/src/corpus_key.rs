//! **Which command produced this capture?**
//!
//! The capture corpus names its files `<command>_<annotation>.txt` —
//! `tasklist_v_win`, `ps_aux_full`, `df_h_en`. The question « under which
//! program to measure this file » was written TWICE, in the bench and in the
//! test support, with the same header and the same rule: the longest
//! prefix the router knows.
//!
//! That rule has a blind spot. `build.rs` makes a dashed alias for
//! EVERY `<prog>_<sub>.rs` module — it notes itself that the alias is dead
//! code at runtime, « no false positive ». True for the product; false for
//! an instrument that uses it as a PROGRAM NAME. `tasklist_v_win`
//! was measured under `tasklist-v`, which is a command on no machine.
//!
//! MEASURED 2026-09-09 on the heaviest capture of the ranking:
//! **7 185 bytes delivered under `tasklist-v`, 2 862 under `tasklist`** — and 2 862
//! also under `tasklist /v`, the line a user really types. In the
//! other direction `macos/ps_aux` returned **1 686** under `ps-aux` versus **3 012**
//! under `ps aux`. The instrument that ranks 265 captures was therefore wrong in
//! both directions, and nothing said so.
//!
//! The remedy is not a better guess: it is to **refuse to
//! guess**. An ambiguous name — first part already known as a program, and
//! a longer prefix also known — must be DECLARED here. Otherwise
//! [`command_of`] returns an error naming the capture, just as a
//! single-stage instrument is made to refuse rather than be allowed to publish a
//! number.

/// The command line, for captures whose name does not give it.
///
/// Key: the file name without directory or extension. Value: the argv the
/// user types, as is — it is what goes through the real routing,
/// flags included (`ls -l` does not reach the same compactor as `ls`).
pub const DECLARED: &[(&str, &str)] = &[
    ("df_h", "df -h"),
    ("df_h_en", "df -h"),
    ("df_h_full", "df -h"),
    ("df_i", "df -i"),
    ("du_sh_etc", "du -sh /etc"),
    ("free_h", "free -h"),
    ("free_h_en", "free -h"),
    ("ls_l_etc", "ls -l /etc"),
    ("ls_R_user", "ls -R /Users/user"),
    ("ls_l_root", "ls -l /root"),
    ("ps_aux", "ps aux"),
    ("ps_aux_full", "ps aux"),
    ("sc_query", "sc query"),
    ("sc_query_full", "sc query"),
    ("sc_query_running", "sc query"),
    ("tasklist_svc_full", "tasklist /svc"),
    ("tasklist_v_win", "tasklist /v"),
];

/// The stem of a capture key: `macos/locale_de_DE/df_h` → `df_h`.
fn stem(key: &str) -> &str {
    key.split('/').next_back().unwrap_or(key)
}

/// The longest prefix of the name the router knows, dash as underscore.
///
/// Answers « which ROUTE to exercise with this capture », which is the question
/// of the corpus-driven guards. It is NOT « which command produced
/// it »: see [`command_of`], and this module's header for the gap
/// measured between the two.
pub fn program_of(key: &str) -> Option<String> {
    let parts: Vec<&str> = stem(key).split('_').collect();
    let mut best: Option<String> = None;
    for n in 1..=parts.len() {
        for cand in [parts[..n].join("-"), parts[..n].join("_")] {
            if crate::filter::knows_program(&cand) {
                best = Some(cand);
            }
        }
    }
    best
}

/// True if the name alone does not say which command produced the capture.
///
/// Two conditions, and both are needed: the first part is already a
/// known program (so the rest is an annotation or a subcommand), and
/// [`program_of`] merges anyway. A name known only WHOLE
/// (`sw_vers`, `vm_stat`, `apt_get`) is not ambiguous: there is nothing to
/// merge.
pub fn is_ambiguous(key: &str) -> bool {
    let parts: Vec<&str> = stem(key).split('_').collect();
    parts.len() > 1
        && crate::filter::knows_program(parts[0])
        && program_of(key).as_deref() != Some(parts[0])
}

/// The command that produced this capture.
///
/// `Ok(None)`: no known program in this name — the capture is not on the
/// command path, the caller skips it. `Err`: the name is AMBIGUOUS and not
/// declared, and the refusal names it; it is the only answer an instrument must
/// not replace with a guess.
pub fn command_of(key: &str) -> Result<Option<String>, String> {
    if let Some((_, cmd)) = DECLARED.iter().find(|(k, _)| *k == stem(key)) {
        return Ok(Some((*cmd).to_string()));
    }
    if is_ambiguous(key) {
        return Err(format!(
            "`{key}`: the name does not say which command produced it \
             (`{}` is a program, and the name merges a longer one). \
             Declare the command line in `corpus_key::DECLARED` — \
             an instrument that guesses ranks 265 captures on a number \
             nobody receives.",
            stem(key).split('_').next().unwrap_or("")
        ));
    }
    Ok(program_of(key))
}

#[cfg(test)]
mod tests {

    /// **Every captured fixture has a recorded richness.** The richness ratchet
    /// lives in the `corpus` test target, which a run of `--lib` never builds:
    /// 19 fixtures went in over a day of rounds with that target red and no one
    /// the wiser. This half of it reads only file names, so it sits here,
    /// where targeted runs pass. Same enumeration as `tests/support/corpus.rs`.
    #[test]
    fn every_fixture_has_a_recorded_richness() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let baseline =
            std::fs::read_to_string(root.parent().unwrap().join("richness_baseline.tsv")).unwrap();
        let recorded: std::collections::HashSet<&str> = baseline
            .lines()
            .filter_map(|l| l.split('\t').next())
            .collect();
        let mut missing = Vec::new();
        let mut seen = 0usize;
        for sub in ["linux", "macos", "windows", "formats"] {
            let Ok(entries) = std::fs::read_dir(root.join(sub)) else {
                continue;
            };
            let mut paths = Vec::new();
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    paths.extend(
                        std::fs::read_dir(&p)
                            .into_iter()
                            .flatten()
                            .flatten()
                            .map(|e| e.path()),
                    );
                } else {
                    paths.push(p);
                }
            }
            for p in paths {
                if sub != "formats" && p.extension().and_then(|e| e.to_str()) != Some("txt") {
                    continue;
                }
                if std::fs::read_to_string(&p).map_or(true, |b| b.trim().is_empty()) {
                    continue;
                }
                seen += 1;
                let key = p
                    .strip_prefix(&root)
                    .unwrap()
                    .with_extension("")
                    .to_string_lossy()
                    .replace('\\', "/");
                if !recorded.contains(key.as_str()) {
                    missing.push(key);
                }
            }
        }
        assert!(seen > 300, "only {seen} fixture(s) found");
        assert!(missing.is_empty(), "fixtures with no recorded richness: {missing:?}\nRe-bless: TOKENADE_RICHNESS_BLESS=1 cargo test -p tokenade-core --test corpus richness");
    }

    /// A declared capture returns ITS line, not the merged name.
    #[test]
    fn a_declared_capture_returns_its_command_line() {
        assert_eq!(
            super::command_of("windows/tasklist_v_win"),
            Ok(Some("tasklist /v".to_string()))
        );
        // And the declaration wins over the stem alone: `ps_aux` is
        // `ps aux`, two words, not `ps` — that is what separated 1 686
        // bytes from 3 012.
        assert_eq!(
            super::command_of("macos/ps_aux"),
            Ok(Some("ps aux".to_string()))
        );
    }

    /// An UNdeclared ambiguous name is refused, and the refusal names it.
    #[test]
    fn an_undeclared_fusion_is_refused_by_name() {
        let err = super::command_of("windows/tasklist_v_nowhere").unwrap_err();
        assert!(err.contains("tasklist_v_nowhere"), "{err}");
        assert!(err.contains("DECLARED"), "{err}");
    }

    /// A name without ambiguity passes without declaration.
    #[test]
    fn an_unambiguous_name_needs_no_declaration() {
        // `pacman_q`: `pacman` is known, `pacman-q` is not, so nothing
        // is merged and the name suffices.
        assert_eq!(
            super::command_of("linux/pacman_q"),
            Ok(Some("pacman".to_string()))
        );
        // `sw_vers` is known only whole.
        assert_eq!(
            super::command_of("macos/sw_vers"),
            Ok(Some("sw_vers".to_string()))
        );
    }
    /// **Every capture in the corpus must resolve.**
    ///
    /// It is the guard that replaces vigilance: a capture added tomorrow
    /// with an ambiguous name makes the instrument REFUSE, naming it, instead of
    /// filing it under an invented command.
    #[test]
    fn every_capture_in_the_corpus_resolves() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let mut checked = 0usize;
        let mut refused = Vec::new();
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "txt") {
                    let key = p
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    checked += 1;
                    if let Err(note) = super::command_of(&key) {
                        refused.push(note);
                    }
                }
            }
        }
        assert!(checked > 200, "the sweep only read {checked} capture(s)");
        assert!(refused.is_empty(), "{refused:#?}");
    }
}
