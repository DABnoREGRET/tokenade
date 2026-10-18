//! **The top-level commands of a script the chain scanner refuses.**
//!
//! [`super::split_chain_segments`] gives up on control flow, heredocs and
//! comments — on purpose: it decides what may be wrapped, and a half-understood
//! script must stay one unit. But routing and labelling still need to know
//! which command does the work, and with the scanner out of the picture they
//! read the first words of the whole text. Measured 2026-10-05 over this
//! machine's transcripts (128 451 real Bash commands): 1 070 named `for`,
//! 745 `{`, 507 `if`, 171 `break;`, 59 `#`, and a heredoc's Python body read
//! as the program (`open(p,"w").write(s)`, 108; a comment's first word, `Clean`).
//!
//! This reader only answers "where does each top-level command start and end":
//! a newline ends one unless a quote, a block (`for`…`done`, `if`…`fi`,
//! `case`…`esac`, `{`…`}`, `(`…`)`), a heredoc body or a trailing `|`, `&&`,
//! `||`, `\` carries it on.

/// The top-level commands of `cmd`, in order, trimmed; comment lines dropped.
pub(super) fn top_level_groups(cmd: &str) -> Vec<&str> {
    let bytes = cmd.as_bytes();
    let mut out = Vec::new();
    let (mut sq, mut dq) = (false, false);
    let mut paren = 0i32;
    let mut blocks = 0i32;
    let mut heredocs: Vec<String> = Vec::new();
    let mut start = 0usize;
    let mut word_start: Option<usize> = None;
    let mut i = 0usize;
    let flush_word = |from: Option<usize>, to: usize, blocks: &mut i32| {
        if let Some(w) = from.map(|f| &cmd[f..to]) {
            match w {
                "for" | "while" | "until" | "select" | "if" | "case" | "{" => *blocks += 1,
                "done" | "fi" | "esac" | "}" | "done;" | "fi;" | "esac;" | "};" => *blocks -= 1,
                _ => {}
            }
        }
    };
    while i < bytes.len() {
        let c = bytes[i];
        if sq {
            sq = c != b'\'';
            i += 1;
            continue;
        }
        if dq {
            if c == b'\\' {
                i += 2;
                continue;
            }
            dq = c != b'"';
            i += 1;
            continue;
        }
        let word_char = !c.is_ascii_whitespace() && !matches!(c, b'\'' | b'"' | b'(' | b')');
        if word_char && word_start.is_none() {
            // A comment runs to the end of its line.
            if c == b'#' {
                let end = cmd[i..].find('\n').map_or(bytes.len(), |n| i + n);
                i = end;
                continue;
            }
            word_start = Some(i);
        }
        if !word_char {
            flush_word(word_start.take(), i, &mut blocks);
        }
        match c {
            b'\\' => i += 1,
            b'\'' => sq = true,
            b'"' => dq = true,
            b'(' if blocks == 0 || paren > 0 => paren += 1,
            b')' if paren > 0 => paren -= 1,
            b'<' if cmd[i..].starts_with("<<") && !cmd[i..].starts_with("<<<") => {
                let rest = cmd[i + 2..].trim_start_matches('-').trim_start();
                let delim: String = rest
                    .trim_start_matches(['\'', '"'])
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                if !delim.is_empty() {
                    heredocs.push(delim);
                }
                i += 1;
            }
            b'\n' => {
                // Heredoc bodies start on the next line: skip each to its
                // delimiter, which then ends the line the `<<` was on.
                for delim in std::mem::take(&mut heredocs) {
                    let mut at = i + 1;
                    while at < bytes.len() {
                        let end = cmd[at..].find('\n').map_or(bytes.len(), |n| at + n);
                        let line = &cmd[at..end];
                        at = end;
                        if line.trim() == delim {
                            break;
                        }
                        at += 1;
                    }
                    i = at.min(bytes.len());
                }
                let before = cmd[start..i].trim_end();
                let continues =
                    before.ends_with('|') || before.ends_with("&&") || before.ends_with('\\');
                if paren <= 0 && blocks <= 0 && !continues {
                    push(&mut out, &cmd[start..i]);
                    start = i + 1;
                    blocks = 0;
                }
            }
            _ => {}
        }
        i += 1;
    }
    flush_word(word_start.take(), bytes.len().min(i), &mut blocks);
    if start < bytes.len() {
        push(&mut out, &cmd[start..]);
    }
    out
}

fn push<'a>(out: &mut Vec<&'a str>, group: &'a str) {
    let g = group.trim();
    if !g.is_empty() && !g.starts_with('#') {
        out.push(g);
    }
}

/// A function definition (`p(){ … }`, `function p { … }`) runs nothing.
pub(super) fn defines_a_function(group: &str) -> bool {
    let head = group.split_whitespace().next().unwrap_or("");
    head == "function" || head.ends_with("()") || head.contains("(){")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_newline_ends_a_command_unless_something_carries_it_on() {
        let cmd = "cd /repo\n# check\necho \"=== a ===\"\nfor f in *.toml; do\n  n=$(grep -c x $f)\n  printf '%s\\n' \"$n\"\ndone\ncat a |\n  wc -l\nx='one\ntwo'";
        assert_eq!(
            top_level_groups(cmd),
            [
                "cd /repo",
                "echo \"=== a ===\"",
                "for f in *.toml; do\n  n=$(grep -c x $f)\n  printf '%s\\n' \"$n\"\ndone",
                "cat a |\n  wc -l",
                "x='one\ntwo'",
            ]
        );
    }

    #[test]
    fn a_heredoc_body_belongs_to_its_command() {
        let cmd = "cd /repo\npython3 - <<'PY'\n# Clean the comment.\np='a'; open(p,'w').write(s)\nfor x in y:\nPY\nls";
        assert_eq!(
            top_level_groups(cmd),
            ["cd /repo", "python3 - <<'PY'\n# Clean the comment.\np='a'; open(p,'w').write(s)\nfor x in y:\nPY", "ls"]
        );
    }

    #[test]
    fn blocks_nest_and_case_patterns_close_no_paren() {
        let cmd = "if [ -d x ]; then\n  case $a in\n    b) ls ;;\n  esac\nfi\n{ ls; echo; } | head\n( cd x\n  make )";
        assert_eq!(
            top_level_groups(cmd),
            [
                "if [ -d x ]; then\n  case $a in\n    b) ls ;;\n  esac\nfi",
                "{ ls; echo; } | head",
                "( cd x\n  make )"
            ]
        );
    }

    #[test]
    fn function_definitions_are_recognised() {
        assert!(defines_a_function("p(){ printf x; }"));
        assert!(defines_a_function("az() {"));
        assert!(defines_a_function("function f {"));
        assert!(!defines_a_function("python3 run.py"));
    }
}
