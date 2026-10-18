//! Feature dump for recalibrating [`tokenade_core::tokenizer::estimate`].
//!
//! Emits one CSV row per real file — structural counts plus the exact
//! cl100k_base token count — so a candidate model can be fitted and scored
//! offline instead of by recompiling. `#[ignore]`d: a calibration aid, not a
//! gate. The gate is `estimator_gain_fidelity.rs`.
//!
//! `cargo test -p tokenade-core --features bpe --test estimator_calibration_dump
//!  -- --ignored --nocapture`
#![cfg(feature = "bpe")]

use std::path::{Path, PathBuf};

fn walk(root: &Path, exts: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(root) else {
        return;
    };
    for e in rd.filter_map(|e| e.ok()) {
        let p = e.path();
        let name = p
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if name.starts_with('.') || name == "target" || name == "node_modules" {
            continue;
        }
        if p.is_dir() {
            walk(&p, exts, out);
        } else if exts.iter().any(|x| name.ends_with(x)) {
            out.push(p);
        }
    }
}

/// Structural counts a candidate estimator could plausibly key on.
struct Feat {
    chars: usize,
    letters: usize,
    digits: usize,
    punct: usize,
    space: usize,
    newline: usize,
    words: usize,
    bracket: usize,
    nonlatin: usize,
    letter_runs: usize,
    digit_runs: usize,
    punct_runs: usize,
    case_breaks: usize,
    /// Σ over letter runs of ceil(len/4) — a "long identifiers split" proxy.
    letter_run_q4: usize,
    /// Σ over letter runs of max(1, len/7) — a "short words are one token" proxy.
    letter_run_q7: usize,
    /// Σ over runs of identical punctuation of ceil(len/3).
    punct_run_q3: usize,
    /// Σ over indentation runs (leading spaces of a line) of ceil(len/4).
    indent_q4: usize,
    space_runs: usize,
    /// Σ over whitespace runs of ceil(len/8) — cl100k has multi-space tokens,
    /// so column padding costs far less than one token per character.
    space_q8: usize,
    space_q16: usize,
    /// Σ over letter runs of max(0, len − 8): the tail of long identifiers and
    /// base64 blobs, where one-token-per-run stops being true.
    letters_over8: usize,
}

fn features(t: &str) -> Feat {
    let mut f = Feat {
        chars: 0,
        letters: 0,
        digits: 0,
        punct: 0,
        space: 0,
        newline: 0,
        words: t.split_whitespace().count(),
        bracket: 0,
        nonlatin: 0,
        letter_runs: 0,
        digit_runs: 0,
        punct_runs: 0,
        case_breaks: 0,
        letter_run_q4: 0,
        letter_run_q7: 0,
        punct_run_q3: 0,
        indent_q4: 0,
        space_runs: 0,
        space_q8: 0,
        space_q16: 0,
        letters_over8: 0,
    };

    let chars: Vec<char> = t.chars().collect();
    f.chars = chars.len();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_alphabetic() {
            let s = i;
            while i < chars.len() && chars[i].is_ascii_alphabetic() {
                if i > s && chars[i].is_ascii_uppercase() && chars[i - 1].is_ascii_lowercase() {
                    f.case_breaks += 1;
                }
                i += 1;
            }
            let len = i - s;
            f.letters += len;
            f.letter_runs += 1;
            f.letter_run_q4 += len.div_ceil(4);
            f.letter_run_q7 += len.div_ceil(7);
            f.letters_over8 += len.saturating_sub(8);
        } else if c.is_ascii_digit() {
            let s = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            f.digits += i - s;
            f.digit_runs += 1;
        } else if c.is_whitespace() {
            let s = i;
            while i < chars.len() && chars[i].is_whitespace() {
                if chars[i] == '\n' {
                    f.newline += 1;
                }
                i += 1;
            }
            let len = i - s;
            f.space += len;
            f.space_runs += 1;
            f.space_q8 += len.div_ceil(8);
            f.space_q16 += len.div_ceil(16);
        } else if c.is_alphabetic() {
            // Non-ASCII letter: counted apart, the script tiers already own it.
            f.nonlatin += 1;
            i += 1;
        } else {
            let s = i;
            while i < chars.len() && chars[i] == c {
                i += 1;
            }
            let len = i - s;
            f.punct += len;
            f.punct_runs += 1;
            f.punct_run_q3 += len.div_ceil(3);
            if "{}[]()\":,".contains(c) {
                f.bracket += len;
            }
        }
    }

    for line in t.lines() {
        let ind = line.len() - line.trim_start_matches(' ').len();
        if ind > 0 {
            f.indent_q4 += ind.div_ceil(4);
        }
    }
    f
}

#[test]
#[ignore = "calibration dump, read by hand"]
fn dump_calibration_csv() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();

    let mut files = Vec::new();
    walk(
        &root.join("crates"),
        &[".rs", ".toml", ".md", ".txt", ".log", ".json", ".html"],
        &mut files,
    );
    walk(&root.join("docs"), &[".md"], &mut files);
    for f in ["README.md", "PATCHNOTES.md", "CLAUDE.md", "DESIGN-NOTES.md"] {
        let p = root.join(f);
        if p.exists() {
            files.push(p);
        }
    }
    // The compacted outputs — the shape the current tiers get wrong.
    let sides = std::env::var_os("TOKENADE_CALIB_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("tokenade-calib"))
        .join("sides");
    walk(&sides, &[".after", ".before"], &mut files);
    files.sort();

    println!("path,chars,letters,digits,punct,space,newline,words,bracket,nonlatin,letter_runs,digit_runs,punct_runs,case_breaks,lrq4,lrq7,prq3,indq4,space_runs,space_q8,space_q16,letters_over8,heur,exact");
    for p in files {
        let Ok(t) = std::fs::read_to_string(&p) else {
            continue;
        };
        if t.len() < 400 || t.len() > 4_000_000 {
            continue;
        }
        let f = features(&t);
        let rel = p
            .strip_prefix(&root)
            .unwrap_or(&p)
            .to_string_lossy()
            .to_string();
        println!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            rel.replace(',', ";"),
            f.chars,
            f.letters,
            f.digits,
            f.punct,
            f.space,
            f.newline,
            f.words,
            f.bracket,
            f.nonlatin,
            f.letter_runs,
            f.digit_runs,
            f.punct_runs,
            f.case_breaks,
            f.letter_run_q4,
            f.letter_run_q7,
            f.punct_run_q3,
            f.indent_q4,
            f.space_runs,
            f.space_q8,
            f.space_q16,
            f.letters_over8,
            tokenade_core::tokenizer::estimate(&t),
            tokenade_core::tokenizer::estimate_exact(&t),
        );
    }
}
