//! **Help for the commands that EXPLORE**: `map`, `skeleton`, `query`,
//! `impact`, `semantic`, `disclose`, `expand-ref`, `mistake`.
//!
//! These are the ones an agent reads mid-work, not at install time.
//! So they carry a constraint the others don't: the help itself is paid
//! for in tokens, every time. A sixty-line help for a command called twenty
//! times per session costs more than what it explains.

use super::*;

pub(crate) fn semantic_help_text() -> &'static str {
    "tokenade semantic — ask the codebase a question in plain language.

USAGE:
  tokenade semantic \"<query>\"…  [-o <mode>] [--budget N] [--limit N]
  tokenade semantic \"<q1>\" \"<q2>\"   SEVERAL questions, one index build
  tokenade semantic where is auth wired up      (unquoted: ONE question)
  aliases: ask, sem

Hybrid retrieval: BM25 lexical scoring blended with dense embeddings,
weighted per query and aware of the framework in use. Returns the
passages that answer, so the raw files never enter your context. The
index is built and refreshed automatically; a directory that was never
indexed says so instead of answering \"no match\".

FLAGS:
  -o, --output-mode, --format <mode>
                  full [default] | paths | count | outline — ask for only
                  the slice you need. `paths` feeds a follow-up Read,
                  `count` answers \"is there any?\" in one line.
  --budget N      cap the answer at N tokens; the footer says how many
                  results it withheld, so a truncated answer never looks
                  complete.
  --limit N, -n N how many ranked hits to consider (default 10). Raising
                  it looks PAST the first screen; it does not re-rank.

NOTES:
  Flags are removed from the question before searching. That was not
  always true — `--limit` used to be searched FOR, which is why a
  nonsense symbol plus `--limit 3` could return matches.

  An unknown flag is refused rather than folded into the query, so a
  typo fails loudly instead of quietly changing what you asked.

  A multi-word question must be quoted. An unquoted word starting with
  a dash is read as a flag and refused.

  `no match for '<query>'` echoes the question exactly as it was
  understood — the fastest way to see what was actually searched."
}

pub(crate) fn disclose_help_text() -> &'static str {
    "tokenade disclose — progressive disclosure: park a big blob, ask it later.

USAGE:
  <cmd> | tokenade disclose stash [--source web|shell|mcp|graphql]
  tokenade disclose read <hash>
  tokenade disclose search <hash> <query>

`stash` reads the blob on STDIN. Below a size threshold it passes the
input through UNCHANGED and stashes nothing — small output is not worth
a round trip. Above it, the blob is written to the stash and you get a
FOLDED preview plus a banner naming the file, an estimated token count,
and the hash to ask with:

  [stashed: …/stash/<hash>.txt · ~50k tokens · … expand-ref <hash> …]

So the whole blob does not enter your context; a preview does, and the
hash is how you get the rest — a question at a time.

  --source <kind>   what produced the blob (web, shell, mcp, graphql).
                    Picks the right reader; omit it and the content is
                    sniffed instead.

`read <hash>` returns the stashed bytes. `search <hash> <query>` returns
only the passages that match, which is the point of stashing in the
first place — the whole blob never has to come back.

An unknown hash says so rather than returning nothing and exiting 0."
}

pub(crate) fn mistake_help_text() -> &'static str {
    "tokenade mistake — a persistent record of directions that did not work.

USAGE:
  tokenade mistake [list]
  tokenade mistake add <summary…> [--ref <commit>]
  git show … | tokenade mistake from-revert

`list` (the default, so a bare `tokenade mistake` lists) prints what has
been recorded. The value is across sessions: an agent that re-reads this
does not spend a second afternoon on an approach already abandoned.

  add <summary…>    record a direction in your own words.
  --ref <commit>    tie the record to the commit it came from.
  --json            (list) print the records as JSON.
  from-revert       read git output on STDIN and record a mistake ONLY
                    if it describes a revert. Safe to pipe blindly: a
                    non-revert is not recorded."
}

pub(crate) fn map_help_text() -> &'static str {
    "tokenade map — the shape of a codebase, without reading it.

USAGE:
  tokenade map [<path>] [-o full|paths|count] [--json]

Prints a compact structure map: directories, what lives in them, and how
big each part is. Use it when you do not yet know WHERE code lives; if
you already know the path, open the file instead — that is cheaper than
any summary.

  <path>                        narrow the map to one directory or file.
                                Counts and listings are the narrowed set,
                                and the header names the scope. Applies to
                                --json too.
  -o, --output-mode, --format   full [default] | paths | count
  --json                        machine-readable form of the same map"
}

pub(crate) fn skeleton_help_text() -> &'static str {
    "tokenade skeleton — signatures only, bodies replaced by markers.

USAGE:
  tokenade skeleton <file…> [-o full|paths|count] [--budget N]

Takes MANY files in one call — `tokenade skeleton a.rs b.rs c.rs` — which
costs one round-trip instead of N. Each body becomes a `// … N lines …`
marker, so the shape of a file arrives without its contents.

  --budget N   cap the output at N tokens (default 20k). The footer says
               what was replaced, so a trimmed skeleton never reads as a
               complete file.
  -o, --output-mode, --format   full [default] | paths | count"
}

pub(crate) fn query_help_text() -> &'static str {
    "tokenade query — locate a symbol without opening files.

USAGE:
  tokenade query <symbol…> [--file <path>…] [-o full|paths|count|outline]\n                           [--context N] [--json]

Answers \"where is this defined, and who calls it\" from the index.
Several symbols per call.

  --file <path>       answer about that file only (repeatable). A path
                      suffix is enough: `docs.rs`, `src/docs.rs` and the full
                      relative path all name the same file, and a directory
                      scopes everything under it. For a name that matches half
                      the repository, this is the difference between thirty
                      rows and the three you meant.
  --context N, -C N   include N surrounding source lines, so the answer
                      arrives inline instead of needing a follow-up grep
  -o, --output-mode, --format
                      full [default] | paths | count | outline —
                      `paths` feeds a Read, `count` answers \"any?\"
  --json              machine-readable hits

An unknown flag or a stray positional is refused rather than silently
dropped (#680) — a typo fails loudly instead of changing the question."
}

pub(crate) fn impact_help_text() -> &'static str {
    "tokenade impact — what breaks if you change this file.

USAGE:
  tokenade impact <file…> [-o full|paths|count] [--json]

Lists the dependents and the symbols they use, so the blast radius of an
edit is known before making it. Several files per call.

  -o, --output-mode, --format   full [default] | paths | count
  --json                        machine-readable report

A file the indexer never parsed says so, instead of reporting \"no
dependents\" — an empty answer and an unknown file are different facts."
}

pub(crate) fn expand_ref_help_text() -> &'static str {
    "tokenade expand-ref — ask a folded blob for the part you need.

USAGE:
  tokenade expand-ref <hash…> [--prompt \"q1, q2\"] [--grep <regex>…] [--raw]

When a compactor folds something, its banner prints a hash. This is the
recovery path: instant, no re-execution, no bypass.

  --prompt \"q1, q2\"       ask several questions in ONE call and get back
                          only the passages that answer — the cheapest
                          option in tokens. Also spelled `--question`, `-q`
  --grep <regex>          matching lines (literal if the regex does not
                          compile), repeatable; prints `N matching
                          line(s) of M total` so a narrow match never
                          looks like the whole document
  --ignore-case, -i       fold case on both sides of `--grep`. A literal
                          miss says whether the term is absent or merely
                          spelled differently, and names this flag when it
                          is the second one
  --raw, --full           the bytes, verbatim — bounded, and it says how to
                          continue. `--from-line N` resumes, `--budget N`
                          moves the ceiling, `--budget 0` removes it

Reach for `--prompt` or `--grep` first. `--raw` puts the stored bytes back
in context, which is what the fold existed to avoid — so it stops at a
budget and hands you the exact command to continue from where it stopped."
}
