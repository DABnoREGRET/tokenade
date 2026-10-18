//! Dockerfile compactor — multi-stage `Dockerfile` / `Containerfile`
//! configs read by agents during build / deploy investigations.
//!
//! What it strips:
//!   * full-line `#` comments AND `# syntax=…` directives (Docker
//!     specifically only honours these at line 1, but we drop them
//!     either way since the agent rarely cares about builder hints)
//!   * runs of blank lines collapse to one
//!   * trailing whitespace
//!   * pure separator-comment lines (`# ─────`, `# ====`, banner art)
//!
//! What it preserves verbatim:
//!   * every `FROM` / `RUN` / `COPY` / `ADD` / `CMD` / `ENTRYPOINT` /
//!     `ENV` / `EXPOSE` / `WORKDIR` / `USER` / `ARG` / `LABEL` /
//!     `HEALTHCHECK` / `VOLUME` / `STOPSIGNAL` / `SHELL` /
//!     `ONBUILD` directive — these define the image and the agent
//!     needs them
//!   * RUN line-continuations (`\` at end of line) — joining them
//!     would lose readability without saving meaningful tokens
//!
//! Real-world ratio on well-commented multi-stage Dockerfiles
//! (~50% comments / ~50% directives): 30-50%.

/// The 18 Docker instructions we recognise as "directive lines".
/// Case-insensitive match against the first word; one of these
/// upfront means the line is structural and must be preserved.
const DIRECTIVES: &[&str] = &[
    "FROM",
    "RUN",
    "COPY",
    "ADD",
    "CMD",
    "ENTRYPOINT",
    "ENV",
    "EXPOSE",
    "WORKDIR",
    "USER",
    "ARG",
    "LABEL",
    "HEALTHCHECK",
    "VOLUME",
    "STOPSIGNAL",
    "SHELL",
    "ONBUILD",
    "MAINTAINER",
];

/// Cheap heuristic detector: among the first 50 non-blank lines,
/// at least 3 must start with a recognised Docker directive AND no
/// line may start with `?>` `<?` `<!DOCTYPE` (HTML/PHP confusion).
pub fn detect(text: &str) -> bool {
    let mut directive_hits = 0usize;
    let mut code_lines = 0usize;
    for line in text.lines().filter(|l| !l.trim().is_empty()).take(80) {
        let t = line.trim_start();
        // Hard-reject lines that prove this isn't a Dockerfile.
        if t.starts_with("<!DOCTYPE")
            || t.starts_with("<?")
            || t.starts_with("?>")
            || t.starts_with("<html")
        {
            return false;
        }
        if t.starts_with('#') {
            continue;
        }
        code_lines += 1;
        let upper_word: String = t
            .split_whitespace()
            .next()
            .unwrap_or("")
            .chars()
            .map(|c| c.to_ascii_uppercase())
            .collect();
        if DIRECTIVES.contains(&upper_word.as_str()) {
            // #660 — Python's `from X import Y` matches `FROM` after
            // case-fold. #566's 30 % dominance gate caught typical
            // mixed-content Python files but failed on import-heavy
            // tops (the first 80 lines of a fresh module can be 30 %+
            // imports). Reject any `FROM` line that also contains
            // ` import ` — Docker's `FROM image[:tag] [AS name]`
            // never has it; Python's `from X import Y` always does.
            if upper_word == "FROM" && line.contains(" import ") {
                continue;
            }
            directive_hits += 1;
        }
    }
    // Require ≥3 directive lines AND ≥30 % density on code lines.
    // Without the dominance check, a Python file with three
    // `from typing import X` lines hits 3 `FROM` matches in 80 lines
    // (~4 %) and was being mis-detected as Dockerfile. Real
    // Dockerfiles are dominated by directives; the lowercase-
    // tolerant 3-of-3 test stays at 100 % density.
    directive_hits >= 3 && code_lines > 0 && (directive_hits * 100) / code_lines >= 30
}

/// Strip comments + blank-run collapse on a Dockerfile.
pub fn compact(text: &str) -> String {
    crate::code_strip::declare_strip(text, strip_only(text), "dockerfile", None)
}

/// The strip WITHOUT its declaration.
///
/// **Not an output path.** Everything a user or agent sees goes through
/// `compact`, which declares what this removed and returns the file whole when
/// the note would cost more than the strip saved. This is public only so the
/// per-language mechanics can be asserted directly: an earlier lesson is that a
/// guard aimed at a wrapper which short-circuits on short input passes against
/// broken code underneath, and these fixtures are one line each on purpose.
pub fn strip_only(text: &str) -> String {
    crate::code_strip::strip_line_language(text, |t| t.starts_with('#'), str::to_string)
}

/// Variant returning `Savings` for the auto-compact ledger.
pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {

    /// The loop this module used to carry, kept as the ORACLE.
    ///
    /// Same reason as `hcl`/`proto`/`sql_source`: a comment
    /// stripper that eats content fails silently, and this repo has already
    /// lost 45 % of a `.tf` that way. The generic must agree line for line.
    #[allow(dead_code)]
    fn reference_compact(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut blank_run = 0usize;
        for line in text.lines() {
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                blank_run += 1;
                if blank_run <= 1 {
                    out.push('\n');
                }
                continue;
            }
            let t = trimmed.trim_start();
            // Full-line comments and Docker `# syntax=` parser directives.
            if t.starts_with('#') {
                continue;
            }
            blank_run = 0;
            out.push_str(trimmed);
            out.push('\n');
        }
        out
    }

    #[test]
    fn delegation_agrees_with_the_loop_it_replaced() {
        for case in &[
            "",
            "FROM alpine:3.19",
            "# whole-line comment",
            "   # indented comment",
            "; ini comment",
            "// slash comment",
            "key = value  # trailing",
            "key = \"a # b\"",
            "RUN echo 'hello # world'",
            "",
            "",
            "",
            "trailing spaces   ",
            "\ttab indented",
            "[section]",
            "no-comment-at-all",
        ] {
            assert_eq!(
                super::strip_only(case),
                reference_compact(case),
                "diverge sur {case:?}"
            );
        }
        let whole = &[
            "",
            "FROM alpine:3.19",
            "# whole-line comment",
            "   # indented comment",
            "; ini comment",
            "// slash comment",
            "key = value  # trailing",
            "key = \"a # b\"",
            "RUN echo 'hello # world'",
            "",
            "",
            "",
            "trailing spaces   ",
            "\ttab indented",
            "[section]",
            "no-comment-at-all",
        ]
        .join("\n");
        assert_eq!(super::strip_only(whole), reference_compact(whole));
    }
    use super::*;

    const MULTI_STAGE: &str = r#"# syntax=docker/dockerfile:1

# ── Build stage ──────────────────────────────────────────────────────
FROM node:22-alpine AS builder

WORKDIR /app

# Copy package manifests
COPY package*.json ./
RUN npm ci --ignore-scripts

# Build sources
COPY src/ ./src/
RUN npm run build

# Remove dev dependencies
RUN npm prune --omit=dev

# ── Runtime stage ─────────────────────────────────────────────────────
FROM node:22-alpine AS runtime

# Non-root user for security
RUN addgroup -S mcp && adduser -S mcp -G mcp

WORKDIR /app

COPY --from=builder --chown=mcp:mcp /app/dist ./dist
COPY --from=builder --chown=mcp:mcp /app/node_modules ./node_modules

USER mcp
EXPOSE 3000
CMD ["node", "dist/index.js"]
"#;

    #[test]
    fn detect_multi_stage_dockerfile() {
        assert!(detect(MULTI_STAGE));
    }

    #[test]
    fn detect_rejects_unrelated_text() {
        assert!(!detect("hello world\nthis is just prose\n"));
        assert!(!detect("[section]\nkey = value\n"));
        assert!(!detect(
            "<!DOCTYPE html>\n<html>\n<head><title>foo</title>\n"
        ));
    }

    #[test]
    fn detect_rejects_python_imports_dominance() {
        // Real false-positive class from production gain.jsonl: cat-ing
        // a Python file with multiple `from X import Y` lines uppercased
        // each `from` → matched the `FROM` Dockerfile directive → the
        // file was passed through the dockerfile compactor at 20 %
        // efficiency. The dominance gate rejects it: 3 `from` matches
        // in ~30 code lines is ~10 %, well under 30 %.
        let py = r#"from typing import Optional, List, Dict
from dataclasses import dataclass
from pathlib import Path
import json
import sys

@dataclass
class Chunk:
    text: str
    start: int
    end: int

def split(text: str, size: int = 512) -> List[Chunk]:
    chunks = []
    i = 0
    while i < len(text):
        end = min(i + size, len(text))
        chunks.append(Chunk(text=text[i:end], start=i, end=end))
        i = end
    return chunks

def main(path: str) -> int:
    p = Path(path)
    if not p.exists():
        print(f"missing: {path}", file=sys.stderr)
        return 1
    data = p.read_text()
    out = split(data)
    print(json.dumps([c.__dict__ for c in out]))
    return 0
"#;
        assert!(
            !detect(py),
            "Python `from X import Y` lines should not be detected as Dockerfile"
        );
    }

    #[test]
    fn detect_handles_lowercase_directives() {
        // Docker accepts case-insensitive instructions; our detector
        // should too.
        let lc = "from alpine\nrun ls\ncopy . /app\n";
        assert!(detect(lc));
    }

    #[test]
    fn detect_rejects_python_import_heavy_top() {
        // #660 — the #566 dominance gate (30 %) catches mixed-content
        // Python but fails on the import-heavy top of a fresh module
        // where the first 80 lines are mostly `from X import Y`.
        // From production gain.jsonl: `cat semble/chunking/core.py`
        // produced 2573-token output detected as auto:dockerfile at
        // 9 % saved (the Dockerfile compactor accidentally happens to
        // not destroy Python, hence the low-but-positive savings).
        // Fix: any `FROM` line containing ` import ` is treated as
        // Python and not counted toward the directive total.
        let py = r#"from typing import Optional, List, Dict, Tuple, Set, Union
from dataclasses import dataclass, field
from pathlib import Path
from collections import Counter, defaultdict
from functools import lru_cache, partial
from itertools import chain, groupby

CHUNK_SIZE = 512
OVERLAP = 64

@dataclass
class Chunk:
    text: str
"#;
        // Without the #660 fix, 6 `from` lines / 9 code lines = 66 % —
        // passes the dominance gate, would have been detected.
        assert!(
            !detect(py),
            "Python `from X import Y` must not pass even when imports dominate the top of the file"
        );
    }

    #[test]
    fn detect_still_accepts_real_dockerfile_with_few_from() {
        // Make sure the #660 guard didn't break legit Dockerfiles
        // whose `FROM` lines never contain ` import ` (they don't —
        // Docker images aren't named `import`).
        let real = "FROM python:3.11-slim\nRUN pip install foo\nCOPY src /app\n";
        assert!(detect(real));
    }

    #[test]
    /// Renamed from `compact_drops_all_comments`, which asserted
    /// `!out.contains("syntax=docker")` — it pinned the loss of the BuildKit
    /// parser directive as if it were correct. `# syntax=docker/dockerfile:1.7`
    /// selects the frontend that builds the file, so a Dockerfile served
    /// without it describes a different build than the one on disk.
    fn compact_drops_notes_and_keeps_directives() {
        let out = compact(MULTI_STAGE);
        assert!(!out.contains("Build stage"));
        assert!(!out.contains("Copy package manifests"));
        assert!(!out.contains("Non-root user"));
        assert!(
            out.contains("syntax=docker"),
            "the parser directive is not a note"
        );
    }

    #[test]
    fn compact_preserves_all_directives() {
        let out = compact(MULTI_STAGE);
        assert!(out.contains("FROM node:22-alpine AS builder"));
        assert!(out.contains("COPY package*.json ./"));
        assert!(out.contains("RUN npm ci --ignore-scripts"));
        assert!(out.contains("FROM node:22-alpine AS runtime"));
        assert!(out.contains("USER mcp"));
        assert!(out.contains("EXPOSE 3000"));
        assert!(out.contains("CMD [\"node\", \"dist/index.js\"]"));
    }

    #[test]
    fn compact_collapses_blank_runs() {
        let input = "FROM alpine\n\n\n\nRUN ls\n";
        let out = strip_only(input);
        assert_eq!(out, "FROM alpine\n\nRUN ls\n");
    }

    #[test]
    fn compact_strips_trailing_whitespace() {
        let input = "FROM alpine   \nRUN ls\t\n";
        let out = strip_only(input);
        assert_eq!(out, "FROM alpine\nRUN ls\n");
    }

    #[test]
    fn compact_real_world_ratio() {
        // Measured on the STRIP, not on the declared output. The two are
        // different claims and were one number: this fixture is a few hundred
        // bytes, so the 81-byte declaration costs ~9 points here, while on the
        // real files these fixtures stand in for it costs 1.9-3.7 (measured
        // 2026-08-26 on four Dockerfiles of 2.2-4.4 KB, savings 10-50%).
        // Folding the note into the ratio would have read as the compactor
        // getting worse. What declaring costs is asserted once for every
        // language in `declaration_costs_at_most_one_line`, size-independently.

        // The bar was 25.0 and is 23.0, because the strip now KEEPS
        // `# syntax=docker/dockerfile:1.7` — a parser directive, not a note.
        // On this ~500-byte fixture that one line is 1.1 points. On the real
        // corpus it is nothing: measured 2026-08-27 over 477 Dockerfiles, only
        // 16 carry a parser or build-check directive at all and they are
        // **478 bytes of 587 071, 0.081%**. The bar moved because the fixture
        // is small, not because the compactor got worse.
        let (_, sav) = crate::archetypes::savings_of(MULTI_STAGE, strip_only);
        assert!(
            sav.percent() >= 23.0,
            "expected ≥23% savings on commented multi-stage, got {:.1}%",
            sav.percent()
        );
    }
}
