//! What a blob of output IS, decided from its first lines.
//!
//! One predicate per shape, grouped by family (web, data/CLI, tabular/markup,
//! structured/logs) so the ladder in `detect` reads as four questions rather
//! than forty.

use super::*;

// The shape predicates move out; the ORDER of the chain stays here, and it is what
// carries the meaning — each exception is commented on the spot.
mod shapes;
pub(crate) use shapes::*;

/// Group A — anti-bot/web + early data/config formats (Challenge … CMake).
pub(super) fn detect_web_and_early_formats(
    text: &str,
    head: &str,
    trimmed_head: &str,
) -> Option<Kind> {
    // 1. Anti-bot challenge — must precede plain HTML detection because the
    //    challenge fingerprint also matches `<html>` content.
    if stealth::detect_challenge(head).is_some() {
        return Some(Kind::Challenge);
    }

    // 1b. SERP — Google/Bing/DDG/Brave/Kagi/Yandex results page. Must precede
    //     plain HTML so a SERP routes to the result-list compactor instead of
    //     the generic html_to_markdown which keeps the boilerplate.
    if serp::detect(text) {
        return Some(Kind::Serp);
    }

    // 2. HTML — DOCTYPE / <html> / common opening tags.
    if trimmed_head.starts_with("<!DOCTYPE")
        || trimmed_head.starts_with("<!doctype")
        || trimmed_head.starts_with("<html")
        || trimmed_head.starts_with("<HTML")
        || (trimmed_head.starts_with('<') && trimmed_head.contains("<body"))
    {
        return Some(Kind::Html);
    }

    // 3. Playwright accessibility-tree snapshot.
    if looks_like_a11y_snapshot(head) {
        return Some(Kind::A11yTree);
    }

    // 3a. OpenAPI / Swagger spec — JSON with `"openapi":"3.x"` or
    //     `"swagger":"2.0"` + a `"paths"` field. Checked BEFORE source map
    //     and generic JSON because specs ARE valid JSON but a dedicated
    //     compactor reduces them by 95%+ (just endpoints + schema names).
    if openapi::detect(text) {
        return Some(Kind::OpenApi);
    }

    // 3a''. Mail, calendars, contacts: each told by its own opening line(s).
    let personal: [(fn(&str) -> bool, Kind); 3] = [
        (email::detect, Kind::Email),
        (icalendar::detect, Kind::Calendar),
        (vcard::detect, Kind::Contacts),
    ];
    for (detect, kind) in personal {
        if detect(text) {
            return Some(kind);
        }
    }
    // 3a'. HTTP Archive (devtools capture): JSON `log.entries[].request`.
    if har::detect(text) {
        return Some(Kind::Har);
    }

    // 3b. Source map — JSON with `"version":3` + `"mappings"`. Checked BEFORE
    //     notebook and generic JSON because source maps ARE JSON but a
    //     dedicated compactor saves 99%+ that generic JSON can't.
    if sourcemap::detect(text) {
        return Some(Kind::SourceMap);
    }

    // 4. Jupyter notebook — JSON shape with `"cells":` at top level.
    //    `cells` is reliably in the head (it's near the start of every
    //    notebook), but `nbformat` typically lives at the END of the file
    //    AFTER the cells array. On notebooks with heavy base64 image
    //    outputs the cells array can be hundreds of KB long, pushing the
    //    nbformat marker far past the 4 KB head window. We search the
    //    whole text for nbformat to stay correct on real-world notebooks.
    if trimmed_head.starts_with('{') && head.contains("\"cells\"") && text.contains("\"nbformat\"")
    {
        return Some(Kind::JupyterNotebook);
    }

    // 4a-pre. Claude Code session transcript — distinctive structure
    //     (every line carries `sessionId` + a known role `type`). Checked
    //     BEFORE generic ndjson so transcripts get the bespoke compactor
    //     that surfaces conversation shape instead of "first 3 JSON
    //     lines + key list".
    if crate::transcript_view::detect(text) {
        return Some(Kind::ClaudeTranscript);
    }

    // 4a. JSON Lines (NDJSON) — ≥3 line-starts with `{` whose braces
    //     balance, AND those are ≥70 % of non-blank head lines. Checked
    //     BEFORE generic JSON so a multi-line cloud-log stream routes here
    //     rather than being mis-classified as a single JSON document.
    if ndjson::detect(text) {
        return Some(Kind::Ndjson);
    }

    // 4b-pre00. Bazel BUILD / WORKSPACE / *.bzl — checked very
    //     early because Bazel `name = "x"` attributes match TOML's
    //     `key = value` heuristic. Bazel's rule vocabulary
    //     (`cc_library(name=…)` etc.) is unambiguous.
    if crate::bazel::detect(text) {
        return Some(Kind::Bazel);
    }

    // 4b-pre01. HCL (Terraform `*.tf`, etc.) — `resource "type"
    //     "name" { … }` block style. Matches TOML's loose
    //     `key = value` heuristic on inner attributes, so checked
    //     here. The distinctive `<keyword> "label" {` shape can't
    //     be confused with TOML.
    if crate::hcl::detect(text) {
        return Some(Kind::Hcl);
    }

    // 4b-pre02. Protobuf `.proto` schema. Distinctive `syntax`,
    //     `package`, `message`, `service`, `rpc` keywords. Checked
    //     early because attribute lines like `string id = 1;` would
    //     otherwise be matched by TOML's `key = value` heuristic.
    if crate::proto::detect(text) {
        return Some(Kind::Proto);
    }

    // 4b-pre03. XML configs / schemas (`pom.xml`, `AndroidManifest`,
    //     `log4j2.xml`, `web.xml`, …) — distinctive `<?xml … ?>`
    //     prolog OR ≥3 `<element …>` lines. Hard-rejects HTML
    //     `<!DOCTYPE html>` so the HTML/markdown pipeline still
    //     claims those.
    // 4b-pre03a. JUnit XML test report — BEFORE the generic XML branch, which
    //     legitimately also matches. Measured 2026-08-26 across a corpus of real repos:
    //     of 279 `.xml` files between 4 KB and 3 MB, **257 are JUnit reports**
    //     carrying 21.6 MB, and the generic XML compactor — comments and
    //     whitespace only — returned every one of them whole. Same shape as
    //     the lockfile-before-TOML fix: the SPECIFIC detector must run before
    //     the generic one that also matches.
    if crate::junit::detect(text) {
        return Some(Kind::JUnit);
    }

    // TAP, before the markdown outline that reads its `# Subtest:` lines as
    // headings (see `tap::detect_by_content`).
    if crate::cmd::linux::test_runner_dispatcher::tap::detect_by_content(text) {
        return Some(Kind::Tap);
    }

    if crate::xml::detect(text) {
        return Some(Kind::Xml);
    }

    // 4b-pre04. LaTeX `.tex` source — distinctive `\documentclass`
    //     / `\begin{document}` / `\usepackage` preamble plus ≥2
    //     backslash commands. Checked early because LaTeX's
    //     `\section{}` and `\command{arg}` shapes don't conflict
    //     with TOML/INI but we don't want Markdown to claim a tex
    //     document that happens to contain `#` characters inside
    //     argument bodies.
    if crate::latex::detect(text) {
        return Some(Kind::LaTeX);
    }

    // 4b-pre05. BibTeX `.bib` bibliography — ≥2 `@article{` /
    //     `@book{` / `@misc{` / etc entry headers. Distinctive
    //     enough that no other format collides; checked separately
    //     from LaTeX because `.bib` files don't have a `\documentclass`
    //     preamble.
    if crate::bibtex::detect(text) {
        return Some(Kind::BibTeX);
    }

    // 4b-pre06. SQL source `.sql` — ≥2 distinct top-level SQL
    //     keywords (CREATE/INSERT/SELECT/etc) AND rejects psql
    //     result-set table borders (those go to Kind::SqlResult
    //     via the dedicated `sql.rs` compactor further down).
    if crate::sql_source::detect(text) {
        return Some(Kind::SqlSource);
    }

    // 4b-pre0. `.env` / env-dump format — checked BEFORE TOML
    //     because `.env` files (`DB_HOST=localhost`, `API_KEY=…`)
    //     match TOML's loose `key = value` heuristic but should
    //     route to the env compactor where SECRETS ARE REDACTED.
    //     Routing them through TOML would expose API keys / passwords
    //     in the model's context. Env detector requires uppercase
    //     `KEY=VALUE` AND no `[section]` headers, so genuine TOML
    //     (which has sections) still wins below.
    if looks_like_env_dump(head)
        && !text.lines().any(|l| {
            let t = l.trim_start();
            t.starts_with('[') && t.contains(']')
        })
    {
        return Some(Kind::EnvDump);
    }

    // 4b-pre. INI / `php.ini` / systemd unit / `mysql.cnf` —
    //     checked BEFORE TOML because both detectors match
    //     `[section]` + `key = value`, but INI handles `;` comments
    //     (which TOML doesn't strip). When a file has any `;`
    //     comments or systemd-style content, INI wins. When it's
    //     truly TOML, INI's `;` comments don't appear and TOML's
    //     stricter inline-table / array-of-tables detection still
    //     claims it.
    if crate::ini::detect(text) {
        // Tie-breaker: if TOML's detector ALSO claims this and the
        // input has NO `;` comments, TOML wins (TOML is the more
        // common Rust/Python case). A `;` anywhere outside strings
        // is a near-certain INI signal.
        let has_semi_comment = text.lines().any(|l| l.trim_start().starts_with(';'));
        if has_semi_comment || !crate::toml_compact::detect(text) {
            return Some(Kind::Ini);
        }
    }

    // 4a-bis. Lockfile (Cargo.lock / go.sum / yarn.lock / Pipfile.lock),
    //     checked BEFORE the TOML branch below and not after it.
    //
    //     A lockfile IS valid TOML, so the generic branch claimed it first and
    //     the note there — "lockfiles are handled by the `lockfile` compactor
    //     below and use their own detector" — was simply false. MEASURED
    //     2026-08-25 on a 135 KB `Cargo.lock`: TomlConfig returned 135 634
    //     bytes (0.999), the lockfile compactor 10 161 (0.075), with every
    //     package name kept either way. A 13× loss, and the same shape as
    //     before: the SPECIFIC detector must run before the generic one that
    //     legitimately also matches.
    //
    //     Safe to move up because this detector is narrow: it demands the
    //     `@generated by Cargo` header, or `[[package]]` TOGETHER WITH a
    //     `name = ` line. A hand-written `Cargo.toml` has `[package]`
    //     singular and no generated header, so it cannot be caught here.
    //     Still ahead of YAML, which is what its old position needed.
    if !matches!(lockfile::detect(head), lockfile::Flavour::Unknown) {
        return Some(Kind::Lockfile);
    }

    // 4b. TOML config file — `Cargo.toml`, `pyproject.toml`, plain
    //     hand-edited `.toml` config. Detect BEFORE generic JSON
    //     because TOML can have inline tables that briefly look JSON-y.
    if crate::toml_compact::detect(text) {
        return Some(Kind::TomlConfig);
    }

    // 4c. Dockerfile / Containerfile — recognised by ≥3 Docker
    //     directives in the first 50 lines (`FROM`, `RUN`, `COPY`,
    //     `CMD`, …). Distinctive enough that no other format is
    //     going to look like a Dockerfile, but we still check
    //     AFTER lockfile/toml/jsonl to keep the dispatch ordering
    //     deterministic for ambiguous inputs.
    if crate::dockerfile::detect(text) {
        return Some(Kind::Dockerfile);
    }

    // 4c3. GNU Makefile — tab-indented recipe lines + `target: deps`
    //     header is a very distinctive shape. Checked AFTER
    //     Dockerfile because some Dockerfile-like inputs could in
    //     theory contain `COPY: foo bar` lines, though in practice
    //     Docker directives + `target:` patterns never co-occur.
    if crate::makefile::detect(text) {
        return Some(Kind::Makefile);
    }

    // 4c4. CMake — ≥3 CMake commands (`add_executable`, `project`,
    //     `target_*`, `find_package`, …) within first 80 lines.
    //     The command vocabulary is distinctive enough that no
    //     other format collides.
    if crate::cmake::detect(text) {
        return Some(Kind::CMake);
    }

    None
}

/// Group B — generic data formats + CLI outputs (JSON … TabularCli).
/// The unified-diff markers that no other format can produce, checked
/// BEFORE every other detector.
///
/// A patch is a patch whatever language its payload is written in, and the
/// payload is what the rest of the chain reads. Measured 2026-08-26 over the
/// 256 patch files in the corpus: **30 never reached the diff compactor** —
/// a README.md patch is claimed by `looks_strongly_like_markdown`, a
/// TypeScript one by `hcl::detect`, and `dist/dev.cjs` by
/// `sql_source::detect`, all of which run ahead of rule 6 in
/// [`detect_data_and_cli`].
///
/// The SQL route is not merely a worse fold, it DELETES the headers: the
/// stripper treats `--` as a comment to end of line, so
/// `diff --git a/dist/dev.cjs b/dist/dev.cjs` is delivered as `diff` and
/// `--- a/dist/dev.cjs` as an empty line. Which file a hunk belongs to is
/// gone, and on a rename both names are. `git diff` on a repository of
/// markdown, SQL or Terraform files hits this on the CONTENT path every
/// time; the file path saved it only because `.patch` pins the hint.
///
/// Deliberately ANCHORED. All 30 misroutes carry the marker at byte 0. The
/// mid-body forms (`\ndiff --git `, a bare `\n@@ -` hunk header) stay at their
/// original late position — five markdown documents in the corpus show a diff
/// inside a fenced code block, and those are prose, not patches.
pub(super) fn unmistakable_unified_diff(head: &str) -> bool {
    head.starts_with("diff --git ") || (head.starts_with("--- ") && head.contains("\n+++ "))
}

pub(super) fn detect_data_and_cli(text: &str, head: &str, trimmed_head: &str) -> Option<Kind> {
    // NOTE: INI / `.conf` detection lives FURTHER DOWN, after the
    // specific detectors (DNS / stack trace / terraform / YAML /
    // env-dump) have had a chance — the INI shape (`;`-comments,
    // `[section]` headers, `key = value`) is too easy to false-
    // match on dig output, Python tracebacks, or environment dumps
    // when checked at the top of dispatch. See `4z` below.

    // 5. Generic JSON or JSON array.
    //
    // Carve out two TOML shapes that would otherwise be hijacked as
    // JSON arrays:
    //   - `[[package]]` / `[[tool.poetry.dependencies]]` — TOML array of
    //     tables. Common in Cargo.lock, pyproject.toml, Pipfile.lock.
    //   - `[section]` followed by `key = "value"` — TOML table.
    // Both contain `"` so the original detector accepted them and
    // routed Cargo.lock through the JSON compactor.
    // A JSON lockfile is a LOCKFILE. Asked before the generic JSON shape, the
    // way `[[package]]` is carved out just below — same reason, other syntax.
    if trimmed_head.starts_with('{')
        && crate::lockfile::detect(text) == crate::lockfile::Flavour::PackageLock
    {
        return Some(Kind::Lockfile);
    }

    let trimmed_head_for_json = trimmed_head;
    let toml_arrayoftables = trimmed_head_for_json.starts_with("[[");
    if (trimmed_head_for_json.starts_with('{') || trimmed_head_for_json.starts_with('['))
        && (head.contains('"') || head.contains(": "))
        && !toml_arrayoftables
        && opens_a_json_value(trimmed_head_for_json)
    {
        return Some(Kind::Json);
    }

    // 6. Unified diff / patch file.
    //
    // The two ANCHORED markers moved to [`unmistakable_unified_diff`], checked
    // at the very TOP of `detect`. The two unanchored ones stay HERE, at their
    // original late position, and that placement is load-bearing: measured
    // 2026-08-26, five markdown documents in the corpus carry `diff --git`
    // inside a fenced code block (`sqz/BENCHMARKS.md` has 7 headings and 18
    // fences around one). Hoisting the mid-body form would rebuild those as
    // patches and throw the prose away — the exact failure the markdown
    // hoist above exists to prevent, one detector over.
    if head.contains("\ndiff --git ") || (head.contains("\n@@ -") && head.contains(" +")) {
        return Some(Kind::UnifiedDiff);
    }

    // 6a. `git log` / `git show` (without `-p`) — multiple `commit <40-hex>`
    //      headers, or one such header + Author/Date. Checked AFTER
    //      UnifiedDiff so `git log -p` correctly routes to the diff
    //      compactor which preserves patches.
    if gitlog::detect(text) {
        return Some(Kind::GitLog);
    }

    // 6b. DNS lookup output (`dig`, `nslookup`, `host`) — `;; ANSWER SECTION:`
    //     / `;; ->>HEADER<<-` / `; <<>> DiG` markers are uniquely dig; host(1)
    //     and nslookup have their own distinctive shapes.
    if dns::detect(text) {
        return Some(Kind::Dns);
    }

    // 6b. Lockfile moved UP, next to the TOML check — see there for why.

    // 7. Terraform / OpenTofu plan.
    if head.contains(": Refreshing state...")
        || (head.contains("Terraform will perform the following actions:")
            && head.contains(" will be "))
        || head.contains("\nPlan: ")
    {
        return Some(Kind::TerraformPlan);
    }

    // 8. `curl -v` transcript — at least one `* `, `> `, AND `< ` line.
    if head.contains("\n* ") && head.contains("\n> ") && head.contains("\n< ") {
        return Some(Kind::CurlVerbose);
    }

    // 8b. TypeScript compiler (`tsc`) diagnostics — the `error TS<digit>` /
    //     `warning TS<digit>` code is unique to tsc, so this is checked before
    //     the stack-trace and generic-logs detectors (a tsc run with a JS
    //     source frame could otherwise look stacktrace-ish). Folds the source
    //     frames + per-file table down to a one-line-per-error list.
    if crate::tsc::detect(text) {
        return Some(Kind::Tsc);
    }

    // 9. Stack trace — language detector handles Python/Rust/JS/Java/Go.
    if stacktrace::detect_lang(head) != stacktrace::Lang::Unknown {
        return Some(Kind::StackTrace);
    }

    // 9a1. Multi-container k8s logs — `[pod/name/container]` prefixed lines.
    //      Checked BEFORE searchout (which also uses `[`-style prefixes —
    //      but only `path:linenum` shape) and BEFORE the generic logs path.
    if containerlogs::detect(text) {
        return Some(Kind::ContainerLogs);
    }

    // 9b0. `cargo bench` / criterion output — `Benchmarking <name>:` or
    //      classic `test ... bench: N ns/iter`. Checked early so it
    //      doesn't fall through to generic tabular or logs detectors.
    if cargobench::detect(text) {
        return Some(Kind::CargoBench);
    }

    // 9b1. grep / ripgrep / git grep search output — `path:linenum:text`
    //      lines. Checked BEFORE disk + filelist so a `grep -r` dump on a
    //      codebase doesn't get mis-routed.
    if searchout::detect(text) {
        return Some(Kind::SearchOut);
    }

    // 9b. `du` disk-usage output — `<size>{whitespace}<path>` lines.
    //     Checked BEFORE filelist (path-only) so a `du` dump with a path
    //     column doesn't get mis-routed.
    if disk::detect(text) {
        return Some(Kind::Disk);
    }

    // 9c. Directory listing (`find`, `tree`, `ls -laR`). Checked BEFORE
    //     archive (some tar listings have similar path lines) so a `find`
    //     dump of a repo doesn't get misclassified.
    if filelist::detect(text) {
        return Some(Kind::FileList);
    }

    // 10. Archive listing — first non-empty line looks like a tar/zip header
    //     OR matches a tarball line shape on the first row.
    //     A DELIMITED TABLE is not a path listing, and its FIELDS are
    //     what make it look like one: a row of three URL columns satisfies
    //     `contains('/')` exactly as thoroughly as three filesystem paths
    //     would. Measured on a 300-row `.csv` of article/embed/title columns:
    //     63 818 bytes came back as `[tokenade:archive] plain-listing — 299
    //     entries`, 191 bytes, every title gone.
    //
    //     The question "is this a delimited table" already has a home
    //     (`detect_csv_or_tsv`, consulted at the CSV branch below). Asking it
    //     here rather than re-deciding is the point: a second copy of the rule
    //     would be the thing that drifts.
    //
    //     Only `Plain` yields, and that is deliberate. The other three formats
    //     are anchored on a literal header (`Length … Date … Time … Name`,
    //     `Date … Time … Attr … Name`) or on a permissions column, so nothing
    //     about them is inferred from punctuation. `Plain` is the one branch
    //     that reads a row's CONTENT. Yielding all four would lose the other
    //     way round: a `tar -tvf` of an archive whose names carry two commas
    //     apiece is a delimited table by this test and an archive in fact.
    let arch_fmt = archive::detect_format(head);
    let table_owns_it =
        matches!(arch_fmt, archive::Format::Plain) && detect_csv_or_tsv(head).is_some();
    if !matches!(arch_fmt, archive::Format::Unknown) && !table_owns_it {
        return Some(Kind::ArchiveListing);
    }

    // 10b. SQL query result — ASCII-bordered (`+---+`) or psql-style
    //      (`---+---`) table. Checked BEFORE tabular CLI because both can
    //      look superficially similar, but SQL uses pipes as separators.
    if !matches!(sql::detect(head), sql::Flavour::Unknown) {
        return Some(Kind::SqlResult);
    }

    // 10c. netstat / ss / lsof -i network listings — tabular-ish but a
    //      dedicated state-bucketing compactor beats generic tabular
    //      compression by 90 %+ on busy-server output.
    if netstat::detect(text) {
        return Some(Kind::Netstat);
    }

    // 10d. `vmstat` system monitoring — the canonical column header
    //      `r b swpd free buff cache si so bi bo in cs us sy id wa st`
    //      is uniquely vmstat. Aggregating per-column min/avg/max from
    //      a 1000+-row time-series beats line-by-line tabular handling.
    if vmstat::detect(text) {
        return Some(Kind::Vmstat);
    }

    // 10e. `tcpdump` packet capture — `HH:MM:SS.NNN IP src.port > dst.port`
    //      lines. Flow-aggregation gives 95%+ compaction on busy captures.
    if tcpdump::detect(text) {
        return Some(Kind::Tcpdump);
    }

    // 11. Tabular CLI — first non-empty line is a known header.
    //     tabular::compact passes unknown headers through, so we infer
    //     detection by checking whether compact returns something different.
    if first_line_is_known_tabular(head) {
        return Some(Kind::TabularCli);
    }

    // LAST. Blank-line-separated `key : value` records — `pacman -Qi`,
    //     `apt show`, `dpkg -s`. Deliberately the final arm: the shape is weak
    //     enough that YAML, TOML, INI, `.env` and every table format must get
    //     their chance first, and each of them is checked above.
    //
    //     The COMMAND path already folds these through
    //     `archetypes::kv_records` — measured 2026-08-25, `pacman -Qi` goes
    //     from 33 599 bytes to 4 039 through `wrap`. The same bytes arriving as
    //     CONTENT were not folded at all (ratio 1.000): the wrap/content
    //     parity gap. Closed by reusing that archetype, not by writing a second one.
    //
    //     What makes the shape worth folding, measured on the same output:
    //     40 records over 21 distinct keys spend 32.2 % of their bytes on
    //     repeated key NAMES.
    if kv_blocks_detect(text) {
        return Some(Kind::KvBlocks);
    }

    None
}

/// Group C — CSV/TSV, key:value block formats, YAML & markdown front-matter
/// (CSV … YAML).
pub(super) fn detect_tabular_and_markup(
    text: &str,
    head: &str,
    trimmed_head: &str,
) -> Option<Kind> {
    // 12. CSV / TSV — header row with multiple commas/tabs and consistent
    //     field counts on the next line.
    if let Some(kind) = detect_csv_or_tsv(head) {
        return Some(kind);
    }

    // 12b1. `systemctl status` block format — `Loaded:` + `Active:` indented
    //       keys are uniquely systemd. Checked BEFORE kubectl describe (both
    //       use indented key:value shapes, but the key vocabularies are
    //       disjoint) and BEFORE YAML.
    if systemd::detect(text) {
        return Some(Kind::Systemd);
    }

    // 12c. `kubectl describe` block format — looks YAML-ish (key:value
    //      pairs + indented sections) but has the distinctive Events table
    //      + k8s-specific keys (`QoS Class:`, `Controlled By:`, etc.).
    //      Checked BEFORE YAML because describe output is NOT valid YAML
    //      (multi-line annotations break the YAML structure) and a dedicated
    //      compactor extracts events + container state cleanly.
    if kubectl::detect(text) {
        return Some(Kind::KubectlDescribe);
    }

    // 12c2. Kubernetes manifest YAML (`helm template`, `kubectl get -o yaml`,
    //       `kustomize build`, raw multi-doc manifests) — text IS valid YAML
    //       but a dedicated compactor extracts the resource list + per-kind
    //       summary (replicas, images, ports, data keys, hosts) far better
    //       than the generic YAML compactor. Detection requires BOTH
    //       `apiVersion:` AND `kind:` as column-0 keys.
    if k8smanifest::detect(text) {
        return Some(Kind::K8sManifest);
    }

    // 12d. Ansible playbook output — `PLAY [name]`, `TASK [name]`,
    //      `PLAY RECAP` markers are uniquely Ansible. Checked BEFORE generic
    //      logs since `ok: [host]` / `changed: [host]` lines could otherwise
    //      look like timestamped log records.
    if ansible::detect(text) {
        return Some(Kind::Ansible);
    }

    // 12e. GitHub Actions workflow logs — `##[group]`, `##[error]`,
    //      `::error::`, `::warning::` directives + ISO-8601 timestamp prefix.
    //      Checked BEFORE YAML/logs since timestamped lines could otherwise
    //      be misclassified as YAML keys or generic syslog records.
    if ghactions::detect(text) {
        return Some(Kind::GhActions);
    }

    // 12f. Package-manager install output — `pip` (`Collecting `+`Downloading `
    //      / `Successfully installed`), `npm` (`npm WARN`/`npm ERR!`/audit
    //      summary), `yarn`, `pnpm`. Checked BEFORE generic logs since the
    //      verbose install output looks superficially like timestamped logs.
    if pkginstall::detect(text) {
        return Some(Kind::PkgInstall);
    }

    // 13. YAML — `key:` at start of multiple lines, with optional `---`
    //     document separator. Conservative: we require either `---` or at
    //     least 3 colon-terminated keys.
    //
    // Carve-out: a Markdown file with Jekyll/Hugo/MDX YAML front-matter
    // STARTS with `---` and a few `key: value` lines, then closes with
    // another `---`, then the body is Markdown (`# Heading`, fences,
    // lists). The YAML detector used to grab these and route them
    // through the YAML compactor — which mangles the prose. We detect
    // the front-matter pattern explicitly and let the markdown branch
    // below handle the whole file.
    let is_md_with_frontmatter = trimmed_head.starts_with("---") && {
        let mut iter = head.lines();
        iter.next(); // opening ---
                     // Within the next 20 lines we expect a closing `---` AND
                     // beyond it at least one markdown marker (`#` heading or
                     // fenced code).
        let mut closed_at: Option<usize> = None;
        for (i, l) in iter.by_ref().enumerate().take(20) {
            if l.trim() == "---" {
                closed_at = Some(i);
                break;
            }
        }
        closed_at.is_some()
            && head.lines().any(|l| {
                let t = l.trim_start();
                t.starts_with("# ") || t.starts_with("## ") || t.starts_with("```")
            })
    };
    // Front-matter + at least one markdown marker IS markdown — no
    // need to defer the decision to `looks_like_markdown_doc` below
    // (which requires 2 markers and would miss our case).
    if is_md_with_frontmatter {
        return Some(Kind::Markdown);
    }
    // YAML detection. Two acceptance paths:
    //   - explicit doc separator `---` at top, OR
    //   - yaml-key lines dominate the head: >= 5 absolute AND >= 70 %
    //     of non-blank / non-comment lines (was 3 / 50 % pre-#439).
    //
    // The tightening kills a steady stream of 0%-saved auto:yaml
    // events on script outputs / log dumps that happen to have a few
    // `word:` lines. Real YAML configs run at 90-100% key-line density
    // and easily clear both thresholds; non-yaml content rarely does.
    // 24h production: 17 auto:yaml events at b_avg=2167, sav_pct=0%
    // before this fix — the 15 610-token node script with only 3
    // incidental `key: value` lines being the headline offender.
    let head_lines: Vec<&str> = head.lines().take(20).collect();
    if trimmed_head.starts_with("---") {
        // C3 audit: a single "---" prefix committed ANY body (build reports,
        // horizontal rules) to the YAML restructurer. The marker is only
        // trusted when the body actually looks YAML-ish.
        let keys = head_lines
            .iter()
            .skip(1)
            .filter(|l| is_yaml_key_line(l))
            .count();
        let cand = head_lines
            .iter()
            .skip(1)
            .filter(|l| {
                let t = l.trim_start();
                !t.is_empty() && !t.starts_with('#') && t != "---"
            })
            .count();
        if cand == 0 || keys * 10 >= cand * 5 {
            return Some(Kind::Yaml);
        }
        // fall through: not YAML despite the marker
    }
    let yaml_keys = head_lines.iter().filter(|l| is_yaml_key_line(l)).count();
    let candidate_lines = head_lines
        .iter()
        .filter(|l| {
            let t = l.trim_start();
            !t.is_empty() && !t.starts_with('#')
        })
        .count();
    if yaml_keys >= 5 && candidate_lines > 0 && yaml_keys * 10 >= candidate_lines * 7 {
        return Some(Kind::Yaml);
    }

    None
}

/// Group D — structured outputs, metrics, markdown, journals & logs
/// (GraphQL … CommandChain).
pub(super) fn detect_structured_and_logs(text: &str, head: &str) -> Option<Kind> {
    // 13a. GraphQL SDL — `directive @`, `schema {`, or multiple `type X {` /
    //      `enum X {` blocks. Checked AFTER YAML (YAML uses `:` + indentation,
    //      not curly braces) and AFTER all JSON-derived formats.
    if graphql::detect(text) {
        return Some(Kind::GraphQl);
    }

    // 13aa. strace / ltrace — the `syscall(...) = retval` shape is unique
    //       to ptrace output. Checked BEFORE logs (some traces look like
    //       structured logs) and BEFORE stack traces (function-call lines
    //       superficially resemble syscall lines, but stack frames don't
    //       have ` = ` after the paren).
    if strace::detect(text) {
        return Some(Kind::Strace);
    }

    // 13ab. JVM build-tool output (Maven / Gradle / SBT) — `[INFO] Building`,
    //       `> Task :`, `BUILD SUCCESS{FUL,}` / `BUILD FAIL{URE,ED}`. Checked
    //       BEFORE generic logs since build output looks superficially like
    //       structured log records.
    if jvmbuild::detect(text) {
        return Some(Kind::JvmBuild);
    }

    // 13b. Prometheus metrics — checked BEFORE env (some metric names use
    //      uppercase identifiers that could be misread as env vars) and
    //      before logs.
    if looks_like_prometheus_metrics(head) {
        return Some(Kind::PrometheusMetrics);
    }

    // 14a. Environment dump — KEY=VALUE lines dominate. Checked BEFORE logs
    //      since env lines could otherwise be mistaken for "log records".
    if looks_like_env_dump(head) {
        return Some(Kind::EnvDump);
    }

    // 14b. Markdown — has heading/list/fence markers in the first 40 lines.
    //      Checked AFTER everything structural so YAML front-matter blocks
    //      don't route here, and BEFORE `Unknown` so a doc with no other
    //      structural cue still gets compacted.
    // The WHOLE text, not `head`, and that is the fix. The competitor
    // of this detector is `command_chain::detect` at the very bottom, which reads the whole
    // text and whose `is_section_header` accepts `## X` and `### X` — ordinary
    // markdown headings. The two compete for the same document on different
    // amounts of evidence, and the loser is then chosen by a
    // window. MEASURED on `pdf_mid.pdf`, a scientific paper: its four
    // headings are on lines 188, 434, 471 and 757, the abstract fits on a single
    // 1.5 KB line, and `head` stops at 16 384 bytes — so this detector
    // saw ONE where its competitor saw four, and the paper went to the
    // stash under the name `…api.rest.chain.txt`.
    if looks_like_markdown_doc(text) {
        return Some(Kind::Markdown);
    }

    // 13a. Generic dash-underline column table — a header line directly
    //      followed by a `---- ----  -----` rule of space-separated dash
    //      groups, with data rows beneath. PowerShell `Format-Table`
    //      (Get-Process, Get-Service, Get-NetAdapter, … — hundreds of cmdlets
    //      with no dedicated module), many SQL/REST CLIs and custom tools all
    //      emit this shape; the dash rule is OS- and language-agnostic, so a
    //      single generic fold covers them without a per-command whitelist.
    //      Placed AFTER markdown/csv/yaml (whose cues — pipes, commas, `key:`
    //      — are disjoint from a space-delimited dash rule, so none of them
    //      can steal a real dash table, and a markdown doc that happens to
    //      contain a dash line is already claimed by markdown above) and
    //      BEFORE the generic log / Unknown fallback (so a long Get-* table
    //      gets a structural header+N-rows fold instead of weak line dedup).
    if tabular::detect_dash_table(head) {
        return Some(Kind::DashTable);
    }

    // 13b2. systemd journal / dmesg — checked BEFORE access logs since the
    //       journalctl date prefix could otherwise be mistaken for an access
    //       log timestamp shape on some lines.
    if !matches!(journal::detect(head), journal::Flavour::Unknown) {
        return Some(Kind::Journal);
    }

    // 13c. Access logs (nginx / Apache combined format) — checked BEFORE
    //      generic logs since both look like timestamped lines but access
    //      logs have a very specific IP + `[date]` + `"METHOD path"` shape.
    if looks_like_access_log_doc(head) {
        return Some(Kind::AccessLog);
    }

    // 13z. Cargo build output. The compactor for it has existed all along and
    //      was reachable ONLY from the command table — `("cargo", "build")` —
    //      so a build log arriving as CONTENT (a `.txt`, an MCP result, a
    //      pasted blob) never met it. Measured 2026-08-27 on a 19 456-byte
    //      cold build: `wrap 'cat …'` delivers 2 407 bytes, `read` of the same
    //      bytes under a neutral extension delivers **all 19 457**. Same
    //      shape as before.
    if looks_like_cargo_build(text) {
        return Some(Kind::CargoBuild);
    }

    // 13z'. A bundler's asset table: one `dist/… 12.34 kB │ gzip: …` row per
    //      emitted file. No route knew it — `slidev build` (2026-10-05, 60 rows)
    //      went out whole — and every Vite-based build prints it.
    if crate::cmd::linux::vite_assets::detect(text) {
        return Some(Kind::BundleAssets);
    }

    // 13z''. A GoogleTest binary: named by its project, so only its content
    //       routes it (`mla_base_lib_test_multi_thread`, 373 k on the fleet).
    if crate::cmd::linux::gtest::detect(text) {
        return Some(Kind::GTest);
    }

    // 14. Log file — timestamps + log levels.
    if head
        .lines()
        .take(5)
        .filter(|l| looks_like_log_line(l))
        .count()
        >= 3
    {
        return Some(Kind::Logs);
    }

    // 14-bis. Minified / bundled source. Placed HERE, at the very end of the
    //     chain, for the same reason the Bash-chain detector below is last: a
    //     bundle is defined by the absence of line structure, which every
    //     format-specific detector above reads better than this one does. A
    //     minified HTML page opens on `<`, a one-line JSON payload on `{`, and
    //     both reach their own compactor before this is asked.
    //     Search hits come first: `grep -rn x | head` whose few hits include
    //     one line of a bundle is ALL long-line bytes and code-shaped, and
    //     was summarised as that bundle with every source hit dropped.
    if crate::cmd::linux::grep::hits_with_a_long_line(text) {
        return Some(Kind::SearchOut);
    }
    if crate::minified::detect(text) {
        return Some(Kind::Minified);
    }

    // L77 — multi-section Bash chains. Run LAST (just before
    // Unknown) so format-specific detectors still win when the
    // body of a single section happens to also be log / json /
    // etc. shaped.
    if crate::command_chain::detect(text) {
        return Some(Kind::CommandChain);
    }

    None
}

#[cfg(test)]
include!("detectors_tests.rs");
