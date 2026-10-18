//! **The summary of a stashed help text is its index, not its head and tail.**
//!
//! A help text is handed over whole while it fits ([`crate::filter`] honours
//! `--help` unconditionally); past the delivery threshold — lowered by context
//! pressure — the stash takes it like any long output, and the generic summary
//! kept its first twenty and last ten lines. On a command table that is the
//! worst cut there is: measured 2026-10-02, a CLI's `--help | sed -n 1,70p`
//! came back as `(elided 40 lines …)`, the forty subcommands in the middle
//! gone, and the agent's next three calls were greps for the names it could no
//! longer see. The reader of a help text is looking a name up; the summary that
//! serves that keeps EVERY name and cuts the descriptions.

/// A row of an option or command table: an indented name column, two spaces or
/// more, then the description. Returns `(indent + name, description)`.
fn split_row(line: &str) -> Option<(&str, &str)> {
    let indent = line.len() - line.trim_start().len();
    if !(1..=10).contains(&indent) {
        return None;
    }
    let body = line[indent..].trim_end();
    // An OPTION alone on its line is a row too, its description on the more
    // indented lines below (clap's long help, GNU `sed`, `rustc`, `make`'s
    // long synonyms): measured 2026-10-02 over 63 real help texts, that form
    // kept `fd` (9 of 47 options indexed), `rg` (26 of 124), `sed` (11 of
    // 15) and `rustc` (15 of 27) out of the index.
    let (name, rest) = match body.find("  ") {
        Some(gap) => (&body[..gap], body[gap..].trim_start()),
        None if body.starts_with('-') => (body, ""),
        None => return None,
    };
    let first = name.chars().next()?;
    // A name, not a line of code: `nl -ba f.rs` writes `   400\t    let x =
    // f(a);  // note`, indented, with two spaces inside — and was indexed as a
    // help text (measured 2026-10-02, `nl -ba filelist.rs | sed -n 400,560p`
    // came back as `mode=help-index`). Option and command names use letters,
    // digits and `-_.,/=:<>[]|…` with single spaces; they never open on a
    // digit and never carry a tab, a quote, a parenthesis, a brace or a `;`.
    let looks_named = first == '-' || first == '[' || first == '<' || first.is_ascii_alphabetic();
    // An option name may also spell its argument with `#`, `+` or `*`
    // (`zstd -B#`, `-O+`): accepted after a leading `-` only.
    let option_extra = if first == '-' { "#+*" } else { "" };
    let name_chars = name.chars().all(|c| {
        c.is_alphanumeric() || c == ' ' || "-_.,/=:<>[]|…".contains(c) || option_extra.contains(c)
    });
    if !looks_named
        || !name_chars
        || name.split(' ').count() > 4
        || (rest.is_empty() && first != '-')
        || name.len() > 64
    {
        return None;
    }
    Some((&line[..indent + name.len()], rest))
}

/// The index of `text` when it reads as a help text — at least eight table rows,
/// and rows with their continuation lines making half the non-blank lines —
/// with each description cut to the widest length that keeps the whole under
/// `budget` characters. `None` otherwise, and the caller's ordinary summary runs.
/// Does `text` read as a help text? At least eight table rows, rows with their
/// continuation lines making half the non-blank lines — and NAMES that are
/// mostly distinct. A help table names each option once; `php artisan
/// route:list` is just as tabular, but its first column is `GET|HEAD` on every
/// other row, and an index of it kept nothing but the verb (found 2026-10-02 by
/// the `a_route_list` shape of `hostile_scale`).
pub fn looks_like_a_help_text(text: &str) -> bool {
    let mut rows = 0usize;
    let mut tabled = 0usize;
    let mut non_blank = 0usize;
    let mut in_row = false;
    let mut names = std::collections::HashSet::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            in_row = false;
            continue;
        }
        non_blank += 1;
        if let Some((name, _)) = split_row(line) {
            rows += 1;
            tabled += 1;
            in_row = true;
            names.insert(name.trim());
        } else if in_row && line.starts_with("   ") {
            tabled += 1;
        } else {
            in_row = false;
        }
    }
    rows >= 8 && tabled * 2 >= non_blank && names.len() * 10 >= rows * 8
}

pub(super) fn help_index(text: &str, budget: usize) -> Option<String> {
    if !looks_like_a_help_text(text) {
        return None;
    }
    let render = |width: usize| -> String {
        let mut out = String::new();
        let mut in_row = false;
        let mut dropped = 0usize;
        let flush = |out: &mut String, dropped: &mut usize| {
            if *dropped > 0 {
                out.push_str(&format!(
                    "      … ({} continuation line(s) cut)\n",
                    *dropped
                ));
                *dropped = 0;
            }
        };
        for line in text.lines() {
            if let Some((name, desc)) = split_row(line) {
                flush(&mut out, &mut dropped);
                in_row = true;
                out.push_str(name);
                let n = desc.chars().count();
                if n == 0 {
                    // A name on its own line: its description is below.
                } else if width > 0 && n <= width + width / 4 {
                    out.push_str("  ");
                    out.push_str(desc);
                } else if width > 0 {
                    out.push_str("  ");
                    out.extend(desc.chars().take(width));
                    out.push('…');
                }
                out.push('\n');
            } else if in_row && line.starts_with("   ") && !line.trim().is_empty() {
                dropped += 1;
            } else {
                flush(&mut out, &mut dropped);
                in_row = false;
                let n = line.chars().count();
                if n > 160 {
                    out.extend(line.chars().take(160));
                    out.push('…');
                } else {
                    out.push_str(line);
                }
                out.push('\n');
            }
        }
        flush(&mut out, &mut dropped);
        out.trim_end().to_string()
    };
    [96, 72, 48, 32, 16]
        .into_iter()
        .map(render)
        .find(|s| s.chars().count() <= budget)
        .or_else(|| Some(render(0)))
}

#[cfg(test)]
mod tests {
    use super::help_index;

    fn help(n: usize) -> String {
        let mut s =
            String::from("Reads the networks.\n\nUsage: tool [OPTIONS] <COMMAND>\n\nCommands:\n");
        for i in 0..n {
            s.push_str(&format!(
                "  cmd{i:03}       {}\n",
                "does a long and detailed thing ".repeat(1 + i % 5)
            ));
        }
        s.push_str("\nOptions:\n      --json  Emit JSON\n  -h, --help  Print help\n");
        s
    }

    /// Every name of a 120-command table survives, under the budget — the
    /// head-and-tail preview kept twenty lines and lost ninety of them.
    #[test]
    fn every_name_survives_and_the_whole_fits() {
        let text = help(120);
        let idx = help_index(&text, 4_000).expect("a command table is a help text");
        for i in 0..120 {
            assert!(idx.contains(&format!("  cmd{i:03}")), "cmd{i:03} lost");
        }
        assert!(idx.contains("Usage: tool"), "the usage line is kept");
        assert!(idx.contains("--help"));
        assert!(
            idx.chars().count() <= 4_000,
            "{} chars",
            idx.chars().count()
        );
        assert!(text.len() > 4_000, "the fixture must need the cut");
    }

    /// Prose and logs are not tables: two spaces inside a sentence do not make
    /// a row, and an unindented line never does.
    /// `nl -ba filelist.rs | sed -n 400,560p`: numbered code, indexed
    /// like a help text.
    #[test]
    fn numbered_source_is_not_a_help_text() {
        let src: String = (400..560)
            .map(|i| {
                format!("{i:>6}\t        let value_{i} = compute(input, &opts);  // step {i}\n")
            })
            .collect();
        assert!(help_index(&src, 4_000).is_none());
        let plain: String = (0..120)
            .map(|i| format!("    let value_{i} = compute(input);  // step {i}\n"))
            .collect();
        assert!(
            help_index(&plain, 4_000).is_none(),
            "indented code with a trailing comment"
        );
    }

    /// The option alone on its line, the description below (coreutils 9.8,
    /// clap, `sed`, `rustc`): 63 real help texts, `fd` 9/47 options indexed.
    #[test]
    fn an_option_alone_on_its_line_is_a_row() {
        let mut help = String::from("Usage: ls [OPTION]... [FILE]...\n\n");
        for i in 0..40 {
            help.push_str(&format!(
                "  -{c}, --opt-{i}\n         what option {i} does, at some length\n",
                c = (b'a' + (i % 26) as u8) as char
            ));
        }
        let idx = help_index(&help, 4_000).expect("a help text");
        for i in 0..40 {
            assert!(
                idx.contains(&format!("--opt-{i}\n")),
                "--opt-{i} lost:\n{idx}"
            );
        }
    }

    /// `php artisan route:list`: tabular, but the first column is the
    /// HTTP verb, repeated.
    #[test]
    fn a_route_list_is_not_a_help_text() {
        let routes: String = (0..60)
            .map(|i| {
                format!(
                    "  {:<10}api/r{i}/{{id}} … r{i}.show › Api\\R{i}Controller@show\n",
                    ["GET|HEAD", "POST", "DELETE"][i % 3]
                )
            })
            .collect();
        assert!(help_index(&routes, 4_000).is_none());
    }

    #[test]
    fn a_log_is_not_a_help_text() {
        let log: String = (0..200)
            .map(|i| format!("2026-10-02 12:00:{i:02} INFO  worker  started job {i}\n"))
            .collect();
        assert!(help_index(&log, 4_000).is_none());
        let prose: String = (0..50)
            .map(|i| format!("Paragraph {i} goes on.  Then it continues for a while.\n"))
            .collect();
        assert!(help_index(&prose, 4_000).is_none());
    }
}
