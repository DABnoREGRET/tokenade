//! **The SHAPES a detector consults.**
//!
//! Each function answers « does this text look like an X? », without ever
//! deciding which compactor to call — that decision is the order of the chain,
//! and it stays with the neighbour. The separation matters: an overly
//! greedy predicate placed at the head of the chain disarms all those after it, and the
//! only way to see it is to be able to read the chain without the six hundred
//! lines of predicates in the middle.

use super::super::*;

/// Access-log detector: ≥3 of the first 20 lines parse as combined log
/// format (IP + `[date]` + quoted request line + status code).
pub(crate) fn looks_like_access_log_doc(head: &str) -> bool {
    // Cheap fingerprint: line containing `] "` (closing bracket + space + open
    // quote of the request) is the canonical combined-format signature.
    head.lines()
        .take(20)
        .filter(|l| l.contains("] \"") && l.contains("\" "))
        .count()
        >= 3
}

/// Prometheus metrics detector: `# TYPE` line(s) present in the head.
/// Conservative — relies on the canonical TYPE comment that real `/metrics`
/// always emits.
pub(crate) fn looks_like_prometheus_metrics(head: &str) -> bool {
    // ≥2 canonical comment lines: a lone `# TYPE definitions` heading in a
    // markdown/script dump misfired the metrics compactor, which then
    // DROPPED every other `#` line and re-bucketed the prose.
    head.lines()
        .take(40)
        .filter(|l| l.starts_with("# TYPE ") || l.starts_with("# HELP "))
        .count()
        >= 2
}

/// Markdown that no weaker detector may take.
///
/// Deliberately much stricter than [`looks_like_markdown_doc`], which answers
/// "could this be markdown?" for the tail of the chain. This one answers "is
/// this so certainly a markdown DOCUMENT that claiming it is safer than
/// whatever else matched first?", so it wants **three real ATX headings** plus
/// one corroborating signal that no config, makefile or shell output produces:
/// headings at more than one level, a fenced block, or three inline links.
///
/// A Makefile's `# comment` lines are the collision that matters, and they are
/// all level one, never fenced, and carry no `](`.
pub(crate) fn looks_strongly_like_markdown(text: &str) -> bool {
    let mut levels = [false; 7];
    let mut headings = 0usize;
    let mut fenced = false;
    let mut links = 0usize;
    for line in text.lines().take(400) {
        let t = line.trim_start();
        // `#` is ASCII, so the count is a byte offset too.
        let hashes = t.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && t[hashes..].starts_with(' ') && t[hashes..].trim().len() > 1
        {
            headings += 1;
            levels[hashes] = true;
        }
        if t.starts_with("```") {
            fenced = true;
        }
        if line.contains("](") {
            links += 1;
        }
    }
    let distinct_levels = levels.iter().filter(|seen| **seen).count();
    headings >= 3 && (distinct_levels >= 2 || fenced || links >= 3)
}

/// Markdown detector: look for at least one of `#`-heading, fenced code, or
/// `- ` list marker in the first 40 lines. Conservative: a single prose
/// paragraph with no markers stays Unknown.
/// Does this line open on a timestamp? `[14:25:48.462] [INFO] …`,
/// `2026-06-05T01:01:04.710Z …`, `[2026.06.05-01.01.04:710][ 42]LogInit: …`.
///
/// NOT the same question as `opens_a_json_value`'s number rule, and
/// deliberately not built on it. That rule states what JSON grammar allows
/// after a number, which rejects a timestamp AND every other kind of prose;
/// this one asks whether a line is a log RECORD. Folding the first into the
/// second would narrow it from "not JSON" to "not a timestamp".
///
/// `ghactions::has_iso_timestamp` is a third question again — an ISO-8601
/// prefix at byte 0, exactly — and it does not see a bracketed clock at all.
pub(crate) fn line_opens_on_a_timestamp(line: &str) -> bool {
    let t = line.trim_start();
    let rest = t.strip_prefix('[').unwrap_or(t);
    let mut digits = 0usize;
    let mut dated_or_clocked = false;
    for c in rest.chars().take(24) {
        if c.is_ascii_digit() {
            digits += 1;
            continue;
        }
        if matches!(c, ':' | '-' | '.' | '/' | 'T' | ' ') {
            // A date says `-`, a clock says `:`. `.` and `/` alone say
            // neither: `[1/842] Building CXX object` is a progress counter.
            dated_or_clocked |= c == ':' || c == '-';
            continue;
        }
        break;
    }
    digits >= 4 && dated_or_clocked
}

/// A timestamped stream with no document structure is a LOG, whatever list
/// markers it carries.
///
/// `looks_like_markdown_doc` claims on two markers among `#`, a fence, `- `,
/// `* ` and `> `, and a build log supplies both from its diagnostics alone:
/// `- ./../ui/src/components/accordion.stories.tsx: CSF: default export must
/// be an object`. Measured 2026-08-26, a 29 018-byte storybook debug log was
/// claimed as markdown and folded 13% where the generic template pass gets 35%.
///
/// BOTH halves are required, and the corpus is why. Vetoing on the timestamps
/// alone costs six real `.md` files — `kubectl-logs.md`, `docker/logs.md`,
/// documentation ABOUT logs, which quotes them. Vetoing on "no heading and no
/// fence" alone costs 174, README files that open on a badge block and reach
/// their first heading past line 40. Together: 1 log claimed back, and **0 of
/// the 16 020 `.md` and 1 057 `.mdx` files in the corpus lost**.
pub(crate) fn is_a_timestamped_stream(head: &str) -> bool {
    // Asks `looks_like_log_line`, the same predicate `Kind::Logs` is claimed
    // on, rather than re-deciding from its own stamp counter. The two answers
    // must not be allowed to drift: this one decides that markdown may NOT
    // have the document, and that one decides who gets it instead.
    let stamped = head
        .lines()
        .take(40)
        .filter(|l| looks_like_log_line(l))
        .count();
    if stamped < 3 {
        return false;
    }
    !head.lines().take(40).any(|l| {
        let t = l.trim_start();
        let h = t.chars().take_while(|c| *c == '#').count();
        ((1..=6).contains(&h) && t[h..].starts_with(' ')) || t.starts_with("```")
    })
}

pub(crate) fn looks_like_markdown_doc(head: &str) -> bool {
    if is_a_timestamped_stream(head) {
        return false;
    }
    let mut markers = 0;
    for l in head.lines() {
        let t = l.trim_start();
        // ATX headings are 1-6 `#`. Listing only three levels meant a document
        // whose visible structure lives at h4+ was not markdown to us: MEASURED
        // 2026-07-22, a PDF rendered to markdown gives `## Attention Is All You
        // Need` then `#### Abstract`, so only ONE marker was counted, the doc
        // fell through to the LAST detector — `command_chain`, whose banner test
        // is `starts_with("###")` and therefore matches `####` — and a research
        // paper was labelled `api.rest.chain.txt`, a claim about the artefact
        // that is simply false and that reaches the stash FILENAME.
        let atx = {
            let hashes = t.chars().take_while(|c| *c == '#').count();
            (1..=6).contains(&hashes) && t[hashes..].starts_with(' ')
        };
        if atx
            || t.starts_with("```")
            || t.starts_with("- ")
            || t.starts_with("* ")
            || t.starts_with("> ")
        {
            markers += 1;
            if markers >= 2 {
                return true;
            }
        }
    }
    false
}

/// Reuse the env module's own detector (it requires ≥3 of the first 10
/// non-empty lines to look like `KEY=value`).
pub(crate) fn looks_like_env_dump(head: &str) -> bool {
    // A shebang means SCRIPT, not dump. The env compactor is a value
    // redactor, and a shell script's opening (`ROOT=…`, `LOG_FILE=…`,
    // `MODE=…`) reaches the three `KEY=value` lines below without trying:
    // measured 2026-08-26, 55 of the 59 files this claimed on the corpus are
    // `.sh`. The same rule lives in `env::looks_like_env`, which is what
    // actually stops the redaction; this one keeps the ROUTER honest, so the
    // Kind reported is not a format the compactor will decline.
    if head.trim_start().starts_with("#!") {
        return false;
    }
    // Cheap pre-filter: must contain `=` AND at least one all-uppercase
    // identifier-shaped token at start of line.
    if !head.contains('=') {
        return false;
    }
    // Require ≥3 of the first 8 non-empty lines to be literal
    // `KEY=value` with an uppercase shell-variable-shaped key.
    //
    // We used to delegate to `tokenade_env::compact` and treat
    // "compact changed the input" as a match, but `env::compact`
    // also accepts the `KEY VALUE` (single-space) form. That meant
    // anything starting with an uppercase keyword + space + value —
    // including Dockerfile directives (`FROM node:22-alpine`,
    // `WORKDIR /app`, `RUN npm ci`) — was claimed by env_dump
    // BEFORE the dockerfile detector could fire. Real env dumps
    // always use `KEY=value`, so requiring the literal `=` kills
    // the false-positive class with no real cost.
    head.lines()
        .filter(|l| !l.trim().is_empty())
        .take(8)
        .filter(|l| {
            let t = l.trim();
            t.find('=').is_some_and(|i| {
                let k = &t[..i];
                !k.is_empty()
                    && k.chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                    && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    && k.chars().any(|c| c.is_ascii_uppercase())
            })
        })
        .count()
        >= 3
}

pub(crate) fn first_line_is_known_tabular(head: &str) -> bool {
    tabular_command_hint(head).is_some()
}

/// Which COMMAND produced this table, when its header says so.
///
/// These fingerprints were already here as a boolean, and each one already
/// names a command implicitly — `USER … PID … COMMAND` is `ps`, `Filesystem …
/// Mounted` is `df`. Returning the name instead of a yes/no lets the CONTENT
/// path reach the per-command compactor the COMMAND path already uses.
///
/// That gap was measured 2026-08-25 on real output: `ps aux` folds to 0.069
/// through its own module and to 0.481 through the generic tabular compactor —
/// **6.9×** — and `lsof` to 0.181 against 0.436. Same bytes, same product,
/// two answers, decided only by whether tokenade happened to run the command
/// itself. Found by the command/content parity comparison.
/// Whether what follows the opening `{`/`[` can begin a JSON value.
///
/// Starting with a bracket and containing a quote or a `": "` anywhere in the
/// head was the whole test, which claims every build log that tags its lines.
/// Measured 2026-08-26 on a Maven reactor build: the first line is
/// `[INFO] Scanning for projects...` and `[INFO] Building jar: /w/…` supplies
/// the colon-space, so `Kind::Json` won and its compactor — which cannot parse
/// that text — returned all 53 871 bytes. tokenade HAS a `JvmBuild` compactor
/// that folds the same content to 561 bytes; it simply never got the chance.
///
/// A JSON value begins with `{`, `[`, `"`, a digit, `-`, or one of
/// `true`/`false`/`null`. `I` of `INFO` is none of those.
pub(crate) fn opens_a_json_value(trimmed: &str) -> bool {
    let mut cs = trimmed.char_indices();
    let Some((_, open)) = cs.next() else {
        return false;
    };
    debug_assert!(open == '{' || open == '[');
    let Some((i, c)) = cs.find(|(_, c)| !c.is_whitespace()) else {
        return false;
    };
    match c {
        // An empty container is still JSON.
        '}' | ']' => true,
        '{' | '[' | '"' => true,
        // `t`/`f`/`n` open `true`/`false`/`null` — and they also open most of
        // the English language. See [`literal_is_followed_by_json`].
        't' | 'f' | 'n' => literal_is_followed_by_json(&trimmed[i..]),
        '-' => number_is_followed_by_json(&trimmed[i..]),
        c if c.is_ascii_digit() => number_is_followed_by_json(&trimmed[i..]),
        _ => false,
    }
}

/// A number is the ONE opener ordinary text imitates. A bracketed timestamp —
/// `[14:25:48.462] [INFO] storybook v10.2.10` — opens exactly like a JSON array
/// of numbers, and every other opener (`{`, `[`, `"`, `t`, `f`, `n`) is a
/// character prose does not put there.
///
/// JSON says what may follow a number: whitespace, a separator, a close, or
/// nothing. A `:` may not, and that single character is the whole difference
/// between a log line and an array. Measured 2026-08-26 on a 29 018-byte
/// storybook debug log: `Kind::Json` claimed it, its compactor could not parse
/// it and returned the input, and the content path served all 29 018 bytes.
pub(crate) fn number_is_followed_by_json(from_number: &str) -> bool {
    json_scalar_boundary(from_number.trim_start_matches(|c: char| {
        c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E')
    }))
}

/// What JSON allows AFTER a scalar: whitespace, a separator, a close, or
/// nothing.
///
/// One function because it is one rule in the grammar. It was spelled inline
/// for numbers only, and the literal openers below need the identical rule —
/// a second copy is how the two drift.
pub(crate) fn json_scalar_boundary(after: &str) -> bool {
    match after.chars().next() {
        None => true,
        Some(c) => c.is_whitespace() || matches!(c, ',' | ']' | '}'),
    }
}

/// `true`, `false` and `null` are the other three openers ordinary text
/// imitates, and the comment above was wrong to group them with `{`, `[` and
/// `"` as "a character prose does not put there". Prose opens with `t`, `f`
/// and `n` constantly, and a bracket-delimited banner is prose:
///
/// ```text
/// [tokenade folded 70 line(s) total: recover the verbatim bytes with …]
/// [note: the run finished, 3 of 4 shards green]
/// [failed: could not reach the registry]
/// ```
///
/// All three were typed `Kind::Json` — the first is tokenade's OWN banner. The
/// cost is the one measured on the storybook log above: the JSON compactor
/// claims the document, cannot parse it, returns its input, and the compactor
/// that WOULD have folded it never runs.
///
/// Same treatment as the number opener, and for the same reason: a real
/// literal is followed by what JSON allows after a scalar, and `tokenade`,
/// `note:` and `failed:` are not.
pub(crate) fn literal_is_followed_by_json(from_literal: &str) -> bool {
    ["true", "false", "null"]
        .iter()
        .find_map(|lit| from_literal.strip_prefix(lit))
        .is_some_and(json_scalar_boundary)
}

pub(crate) fn tabular_command_hint(head: &str) -> Option<&'static str> {
    let first = head.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    if first.contains("CONTAINER ID") && first.contains("IMAGE") {
        return Some("docker");
    }
    if first.starts_with("NAME") && first.contains("READY") && first.contains("STATUS") {
        return Some("kubectl");
    }
    if (first.starts_with("USER") && first.contains("PID") && first.contains("COMMAND"))
        || (first.starts_with("UID") && first.contains("PID") && first.contains("CMD"))
    {
        return Some("ps");
    }
    if first.starts_with("COMMAND") && first.contains("PID") && first.contains("NAME") {
        return Some("lsof");
    }
    if first.starts_with("Filesystem") && first.contains("Mounted") {
        return Some("df");
    }
    if first.starts_with("REPOSITORY") && first.contains("TAG") {
        return Some("docker");
    }
    if first.starts_with("Proto") && first.contains("Local Address") {
        return Some("netstat");
    }
    None
}

pub(crate) fn is_yaml_key_line(line: &str) -> bool {
    let l = line.trim_start();
    if l.is_empty() || l.starts_with('#') {
        return false;
    }
    let Some((key, after)) = l.split_once(':') else {
        return false;
    };
    if key.is_empty()
        || !key
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        return false;
    }
    // The char IMMEDIATELY after the colon must be whitespace, end-of-
    // line, or a quote — that's the YAML invariant `key: value`. Without
    // this check, `10:00:01Z` (timestamps) and `127.0.0.1:8080` (host:
    // port) are accepted as YAML keys, which mis-routes every
    // structured app log to the YAML compactor.
    if after.is_empty() {
        return true;
    }
    let next = after.chars().next().unwrap();
    if !(next.is_whitespace() || next == '"' || next == '\'' || next == '|' || next == '>') {
        return false;
    }
    // Real YAML keys are USUALLY alphabetic at the start, not purely
    // digits — pure-digit keys are valid YAML but vanishingly rare in
    // configs, while ISO dates / IPv4 / timestamps start with digits
    // and were the source of every false-positive we caught.
    if key.starts_with(|c: char| c.is_ascii_digit()) {
        return false;
    }
    true
}

pub(crate) fn detect_csv_or_tsv(head: &str) -> Option<Kind> {
    let mut lines = head.lines().filter(|l| !l.trim().is_empty());
    let first = lines.next()?;
    let second = lines.next()?;
    let commas_first = first.matches(',').count();
    let commas_second = second.matches(',').count();
    let tabs_first = first.matches('\t').count();
    let tabs_second = second.matches('\t').count();
    // CSV needs ≥2 commas in both lines and a matching count, with NO
    // surrounding tabs (so we don't false-positive a numeric log line).
    //
    // Reject content where the first line contains `(`, `)`, or `=` —
    // these are dead giveaways of code / syscall traces / log records
    // with bracketed key=value pairs and never appear in a real CSV
    // header row. Without this guard, `strace` output
    // (`openat(1, "/path", O_RDONLY) = 101`) was mis-classified.
    // Both lines checked: prose sentences with matching comma counts slipped
    // past a first-line-only guard into the 30-row/80-char CSV mutilation.
    let looks_like_code = [first, second]
        .iter()
        .any(|l| l.contains('(') || l.contains('='));
    if commas_first >= 2 && commas_first == commas_second && tabs_first == 0 && !looks_like_code {
        // C3 audit: mirror of the Tsv body-wide fix — two coincidentally
        // comma-matched lines must not commit a MIXED body to the CSV
        // rewriter (30-row/80-char mutilation).
        let (mut same, mut total) = (0usize, 0usize);
        for l in head.lines().take(400) {
            if l.trim().is_empty() {
                continue;
            }
            total += 1;
            if l.matches(',').count() == commas_first {
                same += 1;
            }
        }
        if total < 3 || same * 100 / total >= 80 {
            return Some(Kind::Csv);
        }
        return None;
    }
    if tabs_first >= 2 && tabs_first == tabs_second {
        // A4 fleet catch: a session transcript (prose+SQL) whose first two
        // lines coincidentally shared a tab count was "compacted" as TSV,
        // re-escaping SQL into pseudo-CSV — corruption, not elision. Demand
        // BODY-WIDE consistency: >=80% of non-empty lines carry the same
        // tab count before calling something a TSV table.
        let mut same = 0usize;
        let mut total = 0usize;
        for l in head.lines().take(400) {
            if l.trim().is_empty() {
                continue;
            }
            total += 1;
            if l.matches('\t').count() == tabs_first {
                same += 1;
            }
        }
        if total < 3 || same * 100 / total >= 80 {
            // Tiny docs (<3 lines) keep the first-two-lines rule; longer
            // bodies must be consistently tabular.
            return Some(Kind::Tsv);
        }
        return None;
    }

    None
}

/// Blank-line-separated records of PADDED `key : value` lines.
///
/// Narrow on purpose, in three ways, because this runs last and would
/// otherwise be a net that catches prose:
///
///  * at least four records, so a single stanza is not a stream;
///  * every counted record has three or more `key : value` lines;
///  * consecutive records SHARE most of their key set — that is what makes it
///    a record stream rather than a paragraph that happens to contain colons.
pub(crate) fn kv_blocks_detect(text: &str) -> bool {
    let head: &str = text.get(..text.len().min(16384)).unwrap_or(text);
    let keys_of = |rec: &str| -> Vec<String> {
        rec.lines()
            .filter_map(|l| {
                // `Name            : 7zip` — the padding before the colon is
                // what separates this family from YAML's `name: value`.
                let (k, v) = l.split_once(':')?;
                let k = k.trim_end();
                if k.is_empty() || k != k.trim_start() || !l.contains(" :") {
                    return None;
                }
                if !k
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == ' ' || c == '-' || c == '_')
                {
                    return None;
                }
                let _ = v;
                Some(k.to_string())
            })
            .collect()
    };
    let records: Vec<Vec<String>> = head
        .split(
            "

",
        )
        .map(|r| r.trim_matches('\n'))
        .filter(|r| !r.trim().is_empty())
        .map(keys_of)
        .collect();
    let solid: Vec<&Vec<String>> = records.iter().filter(|k| k.len() >= 3).collect();
    if solid.len() < 4 {
        return false;
    }
    // Key sets must agree: compare each record with the first.
    let first: std::collections::HashSet<&String> = solid[0].iter().collect();
    let agreeing = solid
        .iter()
        .skip(1)
        .filter(|k| {
            let shared = k.iter().filter(|x| first.contains(x)).count();
            shared * 10 >= k.len() * 6
        })
        .count();
    agreeing * 10 >= (solid.len() - 1) * 8
}

/// Cargo build output, and nothing that merely QUOTES it.
///
/// Three conditions, and the corpus set each one. Over 166 212 files:
///
/// * `   Compiling <name> v<digit>` **anchored at column 0** — cargo's exact
///   format, emitted by nothing else. 37 files carry two or more.
/// * A cargo TERMINATOR (`    Finished `, `error: could not compile`, a
///   `generated N warnings` summary). Cargo finishes what it starts; a README
///   quoting three lines does not. Drops 8.
/// * NO markdown structure in the head. The terminator alone still kept three
///   `.md` files — `Pare/docs/tool-schemas/cargo/build.md` and friends,
///   documentation ABOUT cargo that shows its output. This is a known
///   shape and the same conjunction answers it.
///
/// Result: **23 kept, all `.txt` and `.log`, all genuine cargo output; 14
/// dropped, and every `.md`, `.toml`, `.go`, `.py` and `.rs` among them.**
pub(crate) fn looks_like_cargo_build(text: &str) -> bool {
    // Takes the WHOLE text, not the 4 KiB head, because cargo puts its
    // terminator at the END. Measured the hard way: the first version of this
    // predicate read `head` and never fired at all — a 19 456-byte build log
    // has ~120 `Compiling` lines inside 4 096 bytes and its `    Finished`
    // line 15 000 bytes further on. The corpus scan that set the rule read 400
    // LINES; the detector read 4 096 BYTES. The same window mismatch, made
    // twice in one sitting.
    let mut compiling = 0usize;
    let mut document = false;
    let mut terminator = false;
    let is_terminator = |line: &str| {
        line.starts_with("    Finished ")
            || line.starts_with("error: could not compile ")
            || (line.starts_with("warning: `")
                && line.contains(" generated ")
                && line.contains("warning"))
    };
    // The tail is where cargo signs off; 20 lines covers the summary block.
    let mut tail: std::collections::VecDeque<&str> = std::collections::VecDeque::with_capacity(21);
    for line in text.lines() {
        if tail.len() == 20 {
            tail.pop_front();
        }
        tail.push_back(line);
    }
    terminator |= tail.iter().any(|l| is_terminator(l));
    for line in text.lines().take(400) {
        if let Some(rest) = line.strip_prefix("   Compiling ") {
            let mut it = rest.split(' ');
            if let (Some(_name), Some(ver)) = (it.next(), it.next()) {
                if ver
                    .strip_prefix('v')
                    .is_some_and(|d| d.starts_with(|c: char| c.is_ascii_digit()))
                {
                    compiling += 1;
                }
            }
        }
        terminator |= is_terminator(line);
        let t = line.trim_start();
        let h = t.chars().take_while(|c| *c == '#').count();
        if ((1..=6).contains(&h) && t[h..].starts_with(' ')) || t.starts_with("```") {
            document = true;
        }
    }
    compiling >= 2 && terminator && !document
}

/// A Playwright accessibility-tree snapshot, by its own fingerprint.
///
/// The rule used to be `[ref=e` AND `- generic`, and the second half is
/// incidental: `generic` is Playwright's name for an unnamed wrapper, so a page
/// built out of semantic elements — a list of links, a table, a feed — has
/// none. MEASURED 2026-09-04 on a 53,084-byte tree of 500 list items carrying
/// 500 refs and no `generic` node: content detection declined it, the generic
/// repetition pass claimed it instead, and the answer came back as
/// `- link "Item <N1>" [ref=e<N2>]:  [×500 lines]` — **500 refs in, zero out**.
/// A ref is what `browser_click` targets; the a11y compactor keeps every one on
/// purpose ("dropping them would silently break interaction"), and the route
/// with no command name — every MCP tool result, every plugin-bridge result,
/// which is exactly how a browser snapshot arrives — destroyed them while
/// looking complete.
///
/// So the fingerprint is the refs plus the tree they sit in, and `- generic`
/// stays a sufficient signal rather than a required one. Markdown is claimed
/// earlier in the chain, so an ordinary bullet list never reaches this.
pub(crate) fn looks_like_a11y_snapshot(head: &str) -> bool {
    // Enough of each to be a tree rather than a mention of one.
    const MIN_SIGNALS: usize = 3;
    if head.matches("[ref=e").count() < MIN_SIGNALS {
        return false;
    }
    if head.contains("- generic") {
        return true;
    }
    head.lines().filter(|l| is_a11y_node_line(l)).count() >= MIN_SIGNALS
}

/// One node of an accessibility tree: indented, `- `, then a role word.
///
/// The role is lowercase ASCII (`link`, `listitem`, `button`, `heading`); a
/// markdown bullet opening on a word would match too, which is why the caller
/// asks for the refs first.
pub(crate) fn is_a11y_node_line(line: &str) -> bool {
    let t = line.trim_start();
    let Some(rest) = t.strip_prefix("- ") else {
        return false;
    };
    let role: String = rest
        .chars()
        .take_while(|c| c.is_ascii_lowercase())
        .collect();
    role.len() >= 3 && rest.len() > role.len()
}

pub(crate) fn looks_like_log_line(line: &str) -> bool {
    // A timestamp AND a level word. Many container / structured loggers wrap
    // the timestamp in brackets — `[2026-05-28T10:00:01Z] INFO ...` — so
    // `line_opens_on_a_clock` peels a leading `[` before looking.
    //
    // It used to demand a DATE: `bytes[4] == b'-' && bytes[7] == b'-'`, the
    // `YYYY-MM-DD` shape. A great many loggers print the time only, because
    // the day is the file's. Measured 2026-08-26 on a 29 018-byte storybook
    // debug log whose every line reads `[14:25:48.462] [INFO] …`: this
    // declined all five, so `Kind::Logs` never claimed it and the markdown
    // detector — two markers among `#`, a fence, `- `, `* `, `> ` — took it on
    // the strength of its own diagnostics (`- ./../ui/src/components/
    // accordion.stories.tsx: CSF: default export must be an object`).
    //
    // The level word carries the weight, and it always did: a bare clock
    // reading proves nothing, a clock reading followed by `[WARN]` is a log
    // record in any format.
    if !line_opens_on_a_timestamp(line) {
        return false;
    }
    ["INFO", "WARN", "ERROR", "DEBUG", "TRACE", "FATAL"]
        .iter()
        .any(|lvl| line.contains(lvl))
}

/// The `Kind`s a JSON document can BE while remaining valid JSON.
///
/// This membership was written TWICE: here, as a ladder of
/// detectors that answer before the generic JSON shape, and in
/// `refine_hint_by_content` as a list of `match` arms. The two
/// had to agree, and nothing said so — adding `Lockfile` required
/// touching both, in two different spellings. One single
/// question, one single name.
pub(crate) fn is_a_json_specialisation(k: Kind) -> bool {
    matches!(
        k,
        Kind::OpenApi
            | Kind::Har
            | Kind::SourceMap
            | Kind::JupyterNotebook
            | Kind::Ndjson
            | Kind::ClaudeTranscript
            | Kind::Lockfile
    )
}

/// Count lines of the form `=== title ===` (exactly three `=` each side).
pub(crate) fn section_marker_lines(text: &str) -> usize {
    text.lines()
        .filter(|l| {
            let t = l.trim();
            t.len() >= 9
                && t.starts_with("=== ")
                && t.ends_with(" ===")
                && !t[4..t.len() - 4].contains("===")
                && !t.starts_with("====")
                && !t.ends_with("====")
        })
        .count()
}
