//! User-defined command compactors (`~/.tokenade/filters/*.toml` and a
//! project-local `.tokenade/filters/*.toml`).
//!
//! A client's agent can author a declarative TOML filter for a command whose
//! output tokenade only compacts generically (or not at all), so future runs of
//! that command get a tight, command-specific view. Filters are DATA, not code —
//! a fixed set of safe text operations (regex line drops, substitutions,
//! head/tail, whole-output→summary collapse) — so loading one carries no code-
//! execution risk. They are read once at startup, take precedence over the
//! built-in compactors for the commands they match, and their savings flow into
//! `tokenade gain` / the dashboard under the filter's own `name`.
//!
//! Schema (one filter per file):
//! ```toml
//! name = "mytool-status"            # report label; also the filename
//! match_command = "^mytool\\s+status"   # regex tested against the command line
//! strip_ansi = true                 # drop ANSI colour escapes
//! strip_lines_matching = ["^DEBUG", "^\\s*$"]   # drop lines matching any regex
//! replace = [{ from = "verbose-(\\d+)", to = "v\\1" }]   # regex substitutions
//! head_lines = 60                   # keep first N lines …
//! tail_lines = 5                    # … and last M (so the outcome line survives)
//! max_lines = 200                   # hard cap on total lines kept
//! on_empty = "mytool: ok"           # if everything was stripped, emit this
//! match_output = [{ pattern = "BUILD SUCCESSFUL", summary = "build: ok" }]
//! # phases: a state machine for multi-phase output — keep different lines per
//! # phase, advance when a line matches `until`. Last phase has no `until`.
//! [[phases]]                        # phase 0: the compile spew
//! keep  = "error|warning:"          #   keep only errors/warnings here …
//! until = "Running|test result"     #   … until the test phase begins
//! [[phases]]                        # phase 1 (last): the test run
//! keep  = "FAILED|panicked|test result|assertion"
//! [[tests]]                         # validated by `tokenade add-compactor`
//! input = "DEBUG x\nResult: 3\n"
//! expect_contains = "Result: 3"
//! ```

mod draft;
mod trust;
pub use draft::*;
pub use trust::*;

use crate::Savings;
use regex::Regex;
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::OnceLock;

// One regex substitution.
thread_local! {
    /// Did the wrapped command FAIL? A declarative filter is written against
    /// the success shape of a command ("a clean build → build: ok"), and its
    /// whole-output replacement is exactly the operation that erases the reason
    /// a failing run failed. MEASURED: a filter summarising `mytool` to
    /// "mytool: ok" swallowed `ERROR: database migration 0042 failed` on an
    /// exit-1 run — the exit code survived, the reason did not, and an agent
    /// reads the summary, not the code. The built-in path keeps the error line
    /// whatever its position, so on failure we defer to it.
    static COMMAND_FAILED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Record whether the command whose output is about to be filtered failed.
/// Set per call by the wrapper; defaults to "succeeded" so every other caller
/// (tests, MCP, hook paths that have no status) behaves exactly as before.
pub fn set_command_failed(failed: bool) {
    COMMAND_FAILED.with(|c| c.set(failed));
}

pub fn command_failed() -> bool {
    COMMAND_FAILED.with(|c| c.get())
}

/// A field this format does not know is a REFUSAL, not a silent no-op.
///
/// The whole selling point of a declarative filter is that it is validated
/// before it is installed — every regex compiled, every `[[tests]]` case run.
/// A typo went through all of that untouched. MEASURED 2026-09-04:
/// `strip_line_matching` (singular) installed cleanly and folded nothing, and
/// `expect` instead of `expect_contains` installed cleanly with the author's
/// self-test never run — the gate they wrote, silently absent.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Replace {
    pub from: String,
    pub to: String,
}

/// A whole-output → one-line summary rule: when `pattern` matches anywhere in
/// the output, the entire output is replaced by `summary` (e.g. a clean build).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatchOutput {
    pub pattern: String,
    pub summary: String,
}

/// One phase of a `[[phases]]` state machine (for multi-phase output like
/// `cargo build` → `Compiling…` → `Running tests` → failures). While in this
/// phase, keep only lines matching `keep` (all lines if `keep` is absent); when
/// a line matches `until`, advance to the next phase. The last phase needs no
/// `until` — it runs to end. Lets a compactor drop the noisy build spew but
/// KEEP the failure/summary section, which a flat head/tail can't.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Phase {
    #[serde(default)]
    pub keep: Option<String>,
    #[serde(default)]
    pub until: Option<String>,
}

/// A self-test embedded in the filter, run at validation time.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilterTest {
    pub input: String,
    #[serde(default)]
    pub expect_contains: Option<String>,
    #[serde(default)]
    pub expect_equals: Option<String>,
}

/// Raw, deserialized filter as written in the TOML file.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserFilter {
    pub name: String,
    pub match_command: String,
    /// Tie-break ONLY, between filters whose `match_command` matches the same
    /// command with an EQUALLY long span; higher wins. It never overrides a
    /// more specific (longer-span) match. Default 0.
    #[serde(default)]
    pub priority: i64,
    /// Marks a machine-generated starter (`add-compactor --from`). A draft is
    /// REFUSED at install until the author reviews it and removes the flag.
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub strip_ansi: bool,
    #[serde(default)]
    pub strip_lines_matching: Vec<String>,
    #[serde(default)]
    pub replace: Vec<Replace>,
    #[serde(default)]
    pub head_lines: Option<usize>,
    #[serde(default)]
    pub tail_lines: Option<usize>,
    #[serde(default)]
    pub max_lines: Option<usize>,
    #[serde(default)]
    pub on_empty: Option<String>,
    #[serde(default)]
    pub match_output: Vec<MatchOutput>,
    #[serde(default)]
    pub phases: Vec<Phase>,
    #[serde(default)]
    pub tests: Vec<FilterTest>,
}

/// A validated filter with every regex pre-compiled. Built via
/// [`UserFilter::compile`]; that step is also where bad regexes / empty fields
/// are rejected.
#[derive(Debug, Clone)]
pub struct CompiledFilter {
    pub name: String,
    pub priority: i64,
    pub draft: bool,
    pub match_re: Regex,
    strip_res: Vec<Regex>,
    replace: Vec<(Regex, String)>,
    match_output: Vec<(Regex, String)>,
    head_lines: Option<usize>,
    tail_lines: Option<usize>,
    max_lines: Option<usize>,
    on_empty: Option<String>,
    strip_ansi: bool,
    phases: Vec<CompiledPhase>,
}

/// A compiled `[[phases]]` entry: `keep`/`until` regexes pre-built.
#[derive(Debug, Clone)]
struct CompiledPhase {
    keep: Option<Regex>,
    until: Option<Regex>,
}

crate::regexes::lazy_regex!(fn ansi_re() = r"\x1b\[[0-9;?]*[ -/]*[@-~]");

/// Built-in reusable `{{token}}` expansions usable inside `match_command`.
/// They keep common multi-runner alternations DRY across filters — e.g. one
/// `match_command = "^{{runner}} .*\\bbuild\\b"` covers npx/bunx/pnpm dlx/yarn
/// dlx without repeating the alternation. Returns the raw regex fragment.
fn builtin_template(name: &str) -> Option<&'static str> {
    Some(match name {
        // JS package runners.
        "runner" => r"(?:npx|bunx|pnpm dlx|yarn dlx)",
        // Python module / tool runners.
        "py-runner" => r"(?:poetry run|uv run|python3? -m)",
        _ => return None,
    })
}

/// Names of the known templates, for error messages.
fn template_names() -> &'static str {
    "runner, py-runner"
}

/// Expand `{{token}}` templates in a `match_command` pattern BEFORE it reaches
/// the regex engine (so the `{{…}}` syntax never collides with regex `{n}`
/// repetition). A pattern with no `{{` is returned byte-identical (full retro-
/// compat). An UNKNOWN `{{token}}` is a hard error — silently expanding it to a
/// literal would produce a filter that never matches, which is worse than a
/// Compile a pattern that is applied to the WHOLE output, with `^` and `$`
/// meaning line boundaries.
///
/// One file format, one meaning for `^`. `strip_lines_matching` is applied line
/// by line, so an anchor there has always worked; `replace` and `match_output`
/// are applied to the whole blob, so the same anchor matched only at the very
/// start of the output — and did it SILENTLY. MEASURED 2026-09-04 on a filter
/// that installs and validates cleanly: `match_output.pattern = "^ALL SUITES
/// PASSED"` never fired on output whose last line is exactly that, and
/// `replace.from = "^PASS \\((\\d+)ms\\) "` rewrote nothing, while the same
/// two without the anchor both worked. The help says the pattern is tested
/// "anywhere in the output", which is what a reader takes an anchor to mean
/// line-wise.
///
/// `(?m)` written by hand still works — it is what this sets — and a pattern
/// with no anchor is unaffected.
fn line_anchored(pattern: &str) -> Result<Regex, regex::Error> {
    regex::RegexBuilder::new(pattern).multi_line(true).build()
}

/// clear message at install time.
fn expand_command_templates(pattern: &str) -> Result<String, String> {
    if !pattern.contains("{{") {
        return Ok(pattern.to_string());
    }
    static TOK: OnceLock<Regex> = OnceLock::new();
    let re = TOK.get_or_init(|| Regex::new(r"\{\{\s*([A-Za-z0-9_-]+)\s*\}\}").unwrap());
    let mut unknown: Option<String> = None;
    let out = re
        .replace_all(pattern, |caps: &regex::Captures| {
            match builtin_template(&caps[1]) {
                Some(v) => v.to_string(),
                None => {
                    if unknown.is_none() {
                        unknown = Some(caps[1].to_string());
                    }
                    String::new()
                }
            }
        })
        .into_owned();
    if let Some(name) = unknown {
        return Err(format!(
            "`match_command` uses unknown template `{{{{{name}}}}}` (known: {})",
            template_names()
        ));
    }
    Ok(out)
}

impl UserFilter {
    /// Validate + compile this filter. Returns a clear error string on the
    /// first problem (empty name/match, un-compilable regex, no-op filter) so
    /// `tokenade add-compactor` can show the author exactly what to fix.
    pub fn compile(&self) -> Result<CompiledFilter, String> {
        if self.name.trim().is_empty() {
            return Err("`name` must not be empty".into());
        }
        if self.match_command.trim().is_empty() {
            return Err("`match_command` must not be empty".into());
        }
        let expanded = expand_command_templates(&self.match_command)?;
        let match_re = Regex::new(&expanded)
            .map_err(|e| format!("`match_command` is not a valid regex: {e}"))?;
        let mut strip_res = Vec::with_capacity(self.strip_lines_matching.len());
        for p in &self.strip_lines_matching {
            strip_res.push(
                Regex::new(p).map_err(|e| format!("`strip_lines_matching` entry {p:?}: {e}"))?,
            );
        }
        let mut replace = Vec::with_capacity(self.replace.len());
        for r in &self.replace {
            let re =
                line_anchored(&r.from).map_err(|e| format!("`replace.from` {:?}: {e}", r.from))?;
            replace.push((re, r.to.clone()));
        }
        let mut match_output = Vec::with_capacity(self.match_output.len());
        for m in &self.match_output {
            let re = line_anchored(&m.pattern)
                .map_err(|e| format!("`match_output.pattern` {:?}: {e}", m.pattern))?;
            match_output.push((re, m.summary.clone()));
        }
        let mut phases = Vec::with_capacity(self.phases.len());
        for (i, p) in self.phases.iter().enumerate() {
            let keep = match &p.keep {
                Some(k) => {
                    Some(Regex::new(k).map_err(|e| format!("`phases[{i}].keep` {k:?}: {e}"))?)
                }
                None => None,
            };
            let until = match &p.until {
                Some(u) => {
                    Some(Regex::new(u).map_err(|e| format!("`phases[{i}].until` {u:?}: {e}"))?)
                }
                None => None,
            };
            phases.push(CompiledPhase { keep, until });
        }
        // Reject a filter that can never do anything — it would just add a
        // matching cost with no benefit and confuse the author.
        let does_something = self.strip_ansi
            || !strip_res.is_empty()
            || !replace.is_empty()
            || !match_output.is_empty()
            || !phases.is_empty()
            || self.head_lines.is_some()
            || self.tail_lines.is_some()
            || self.max_lines.is_some();
        if !does_something {
            return Err(
                "filter has no operations — set at least one of strip_ansi, \
                 strip_lines_matching, replace, match_output, phases, head_lines, \
                 tail_lines, max_lines"
                    .into(),
            );
        }
        let compiled = CompiledFilter {
            name: self.name.trim().to_string(),
            priority: self.priority,
            draft: self.draft,
            match_re,
            strip_res,
            replace,
            match_output,
            head_lines: self.head_lines,
            tail_lines: self.tail_lines,
            max_lines: self.max_lines,
            on_empty: self.on_empty.clone(),
            strip_ansi: self.strip_ansi,
            phases,
        };
        // Run embedded self-tests against the compiled filter.
        for (i, t) in self.tests.iter().enumerate() {
            // A case with no assertion is not a test. The install is gated on
            // these, so one that checks nothing is a gate the author believes
            // they wrote and does not have — the same silence a misspelled
            // field used to buy.
            if t.expect_contains.is_none() && t.expect_equals.is_none() {
                return Err(format!(
                    "test #{i} asserts nothing: give it `expect_contains` or `expect_equals`"
                ));
            }
            let got = compiled.apply(&t.input);
            if let Some(want) = &t.expect_contains {
                if !got.contains(want) {
                    return Err(format!(
                        "test #{i} failed: output does not contain {want:?}\n--- got ---\n{got}"
                    ));
                }
            }
            if let Some(want) = &t.expect_equals {
                if got.trim_end() != want.trim_end() {
                    return Err(format!(
                        "test #{i} failed: output != expected\n--- got ---\n{got}\n--- want ---\n{want}"
                    ));
                }
            }
        }
        Ok(compiled)
    }
}

impl CompiledFilter {
    /// Apply this filter's operations to a command's output, in order:
    /// whole-output summary → strip ANSI → substitutions → line drops →
    /// on_empty → head/tail → max_lines.
    pub fn apply(&self, output: &str) -> String {
        // 1. Whole-output collapse on a success/signature match.
        for (re, summary) in &self.match_output {
            if re.is_match(output) {
                return summary.clone();
            }
        }
        // 2. ANSI.
        let mut text = if self.strip_ansi {
            ansi_re().replace_all(output, "").into_owned()
        } else {
            output.to_string()
        };
        // 3. Substitutions.
        for (re, to) in &self.replace {
            text = re.replace_all(&text, to.as_str()).into_owned();
        }
        // 4. Line drops.
        if !self.strip_res.is_empty() {
            text = text
                .lines()
                .filter(|l| !self.strip_res.iter().any(|re| re.is_match(l)))
                .collect::<Vec<_>>()
                .join("\n");
        }
        // 4b. Phase state machine: walk lines, keeping only those matching the
        // current phase's `keep`; a line matching the phase's `until` advances
        // to the next phase (evaluated under the current phase, so the boundary
        // line is kept per the phase it appears in). Keeps the failure/summary
        // section a flat head/tail would drop.
        if !self.phases.is_empty() {
            let mut i = 0usize;
            let last = self.phases.len() - 1;
            let mut kept: Vec<&str> = Vec::new();
            for line in text.lines() {
                let ph = &self.phases[i];
                let keep = ph.keep.as_ref().map(|re| re.is_match(line)).unwrap_or(true);
                if keep {
                    kept.push(line);
                }
                if i < last {
                    if let Some(until) = &ph.until {
                        if until.is_match(line) {
                            i += 1;
                        }
                    }
                }
            }
            text = kept.join("\n");
        }
        // 5. Everything gone → confirmation line, if the author set one.
        if text.trim().is_empty() {
            if let Some(msg) = &self.on_empty {
                return msg.clone();
            }
        }
        // 6. head + tail (keep the final outcome line).
        if self.head_lines.is_some() || self.tail_lines.is_some() {
            let lines: Vec<&str> = text.lines().collect();
            let n = lines.len();
            let h = self.head_lines.unwrap_or(0);
            let t = self.tail_lines.unwrap_or(0);
            if h + t < n {
                let mut out: Vec<String> = Vec::with_capacity(h + t + 1);
                out.extend(lines[..h].iter().map(|s| s.to_string()));
                out.push(format!("… [{} lines omitted]", n - h - t));
                if t > 0 {
                    out.extend(lines[n - t..].iter().map(|s| s.to_string()));
                }
                text = out.join("\n");
            }
        }
        // 7. Hard line cap (head).
        if let Some(max) = self.max_lines {
            let lines: Vec<&str> = text.lines().collect();
            if lines.len() > max {
                let mut out: Vec<String> = lines[..max].iter().map(|s| s.to_string()).collect();
                out.push(format!("… [{} more lines]", lines.len() - max));
                text = out.join("\n");
            }
        }
        text
    }

    /// Apply + measure the token saving (before = raw, after = compacted).
    pub fn apply_with_savings(&self, output: &str) -> (String, Savings) {
        let before = crate::tokenizer::estimate(output);
        let out = self.apply(output);
        let after = crate::tokenizer::estimate(&out);
        (out, Savings::new(before, after))
    }
}

#[cfg(test)]
include!("user_filters_tests.rs");
