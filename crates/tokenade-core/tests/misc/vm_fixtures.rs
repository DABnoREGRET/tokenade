// Locale test names embed locale codes like `de_DE`, `es_MX` which
// are camel-case-ish by convention even in snake_case test fns.
// Allow the deviation crate-wide so future locale fixtures don't
// keep tripping clippy.
#![allow(non_snake_case)]

//! Integration tests against real VM output captures.
//!
//! Fixtures under `tests/fixtures/{windows,macos}/<cmd>.txt` were
//! captured by SSH-ing into real Windows + macOS virtual machines.
//! Re-capture them with `scripts/capture-vm-fixtures.py`.
//!
//! ## What these tests prove
//!
//! 1. **Routing** — every fixture, identified by the longest
//!    program-name prefix of its filename, finds a matching compactor
//!    through `dispatch_named` (or its `dispatch_auto` fallback).
//! 2. **Locale tolerance** — the Windows VM is configured with a
//!    French UI locale (`Suffixe DNS propre…`, `Adresse IPv6`,
//!    `Nom de la tâche`). A regression that hard-codes English
//!    keywords would fail here.
//! 3. **Info preservation** — `MUST_CONTAIN` declares the key tokens
//!    that *must* survive compaction. A compactor that strips too
//!    aggressively gets flagged.
//!
//! ## Filename convention
//!
//! Fixtures are named `<program>[_<variant>...].txt`. The variant
//! suffix encodes flag combinations (`ls_l`, `ls_la`, `ls_lh_etc`,
//! `top_l1_full`, …). The mapper at [`resolve_prog`] walks the
//! underscore-separated parts right-to-left, dropping one suffix at
//! a time until `dispatch_named` returns `Some`. So `top_l1_full`
//! tries `top_l1_full → top_l1 → top` and succeeds on `top`.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use tokenade_core::cmd::{dispatch_compact, dispatch_named};

fn fixture_dir(os: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests");
    p.push("fixtures");
    p.push(os);
    p
}

/// Percent of bytes removed by compaction (0 when nothing was cut).
fn saved_pct(before: &str, after: &str) -> f64 {
    let b = before.len();
    if b == 0 {
        return 0.0;
    }
    100.0 * (b.saturating_sub(after.len())) as f64 / b as f64
}

/// Per-fixture MINIMUM reduction (`min_saved_percent`). A compactor that
/// silently degrades to identity still shrinks by 0%, passing a bare
/// `compacted <= body` check; this floor catches that for fixtures we KNOW are
/// noisy. Keyed by `<os>/<stem>`. Absent → only the no-growth check applies.
fn min_saved_percent(os: &str, stem: &str) -> Option<f64> {
    Some(match (os, stem) {
        ("windows", "schtasks_full") => 40.0,
        ("windows", "sc_query_full") => 40.0,
        ("windows", "tasklist") => 25.0,
        _ => return None,
    })
}

/// Resolve the program name from a fixture stem. Strategy: try the
/// stem verbatim first, then progressively drop one underscore-
/// separated suffix at a time and re-probe with the fixture body.
/// Uses `dispatch_compact` (not `dispatch_named`) so that recognised-
/// but-short content (where `fold` returns `None`) still counts as
/// "routed". Returns the first prefix that gets a `Some`, or the stem
/// itself if nothing resolves.
fn resolve_prog(stem: &str, body: &str) -> (String, bool) {
    let mut candidate = stem.to_string();
    loop {
        if dispatch_compact(&candidate, body).is_some() {
            return (candidate, true);
        }
        let Some(idx) = candidate.rfind('_') else {
            return (stem.to_string(), false);
        };
        candidate.truncate(idx);
        if candidate.is_empty() {
            return (stem.to_string(), false);
        }
    }
}

/// Tokens that MUST survive compaction. Populated per-fixture. When
/// an entry is missing, only routing is checked (no qualitative
/// assertion). Locale-sensitive: tokens here should be language-
/// neutral (numbers, hex, IP addresses, paths) so the same map works
/// for EN/FR/DE/ES.
fn must_contain(os: &str) -> BTreeMap<&'static str, Vec<&'static str>> {
    let mut m = BTreeMap::new();
    if os == "macos" {
        // sw_vers always emits the macOS version number — must survive.
        m.insert("sw_vers", vec!["macOS"]);
        m.insert("sw_vers_full", vec!["ProductVersion"]);
        m.insert("sw_vers_product", vec!["."]); // version dot
                                                // uname -a contains kernel name + version.
        m.insert("uname", vec!["Darwin"]);
        m.insert("uname_a", vec!["Darwin"]);
        m.insert("uname_s", vec!["Darwin"]);
        // hostname is a non-empty single token.
        m.insert("hostname", vec![]);
        // diskutil list shows /dev/disk* entries — keep at least one.
        m.insert("diskutil_list", vec!["/dev/disk"]);
        m.insert("diskutil_list_full", vec!["/dev/disk"]);
        // pmset shows hibernatemode/sleep settings — must keep a known key.
        m.insert("pmset_g", vec![]); // generic — just needs to route
                                     // df shows the root mount with a percentage somewhere.
        m.insert("df_h", vec!["/"]);
        m.insert("df_h_full", vec!["/"]);
        // ifconfig contains lo0/en0 markers + inet.
        m.insert("ifconfig", vec!["lo0"]);
        m.insert("ifconfig_lo", vec!["lo0"]);
        m.insert("ifconfig_a_full", vec!["lo0"]);
        // ps output has at least the user column.
        m.insert("ps_aux", vec!["PID"]);
        m.insert("ps_aux_full", vec!["PID"]);
        // launchctl list table starts with "PID" header.
        m.insert("launchctl_list", vec![]);
        m.insert("launchctl_list_full", vec![]);
        // system_profiler hardware data has either Model / Chip / Memory.
        m.insert("system_profiler", vec![]);
        // top has a Processes: line near the top.
        m.insert("top_l1", vec!["Processes"]);
        m.insert("top_l1_full", vec!["Processes"]);
        // ping reports packet loss summary.
        m.insert("ping", vec!["icmp_seq"]);
        m.insert("ping_local", vec!["icmp_seq"]);
        m.insert("ping_localhost", vec!["icmp_seq"]);
    } else if os == "windows" {
        // ipconfig has "Suffixe DNS" (FR) or "DNS suffix" (EN). Use IP
        // address shape instead — locale-invariant.
        m.insert("ipconfig", vec!["IPv"]);
        m.insert("ipconfig_all", vec!["IPv"]);
        // hostname is a single token.
        m.insert("hostname", vec![]);
        // tasklist has columns with PID numbers and memory in 'Ko'/'K'.
        m.insert("tasklist", vec![]);
        m.insert("tasklist_full", vec![]);
        m.insert("tasklist_v_win", vec![]);
        // systeminfo contains a Microsoft Windows version.
        m.insert("systeminfo_full", vec!["Microsoft"]);
        m.insert("systeminfo", vec!["Microsoft"]);
        // wmic os contains Caption=Microsoft / Version=10.
        m.insert("wmic_os_full", vec!["Microsoft"]);
        m.insert("wmic_os", vec!["Microsoft"]);
        // sc query lists at least one SERVICE_NAME.
        m.insert("sc_query", vec!["SERVICE_NAME"]);
        m.insert("sc_query_full", vec!["SERVICE_NAME"]);
        // dir output has paths with backslash.
        m.insert("dir", vec![]);
        m.insert("dir_c", vec![]);
        // netstat shows TCP/UDP entries.
        m.insert("netstat", vec![]);
        m.insert("netstat_an_win", vec![]);
        // schtasks has at least one "\\" path-like token or any task.
        m.insert("schtasks", vec![]);
        m.insert("schtasks_full", vec![]);
        // PowerShell get-* cmdlets have header rows.
        m.insert("ps_get_service", vec![]);
        m.insert("ps_get_process", vec![]);
        m.insert("ps_get_psversion", vec!["PSVersion"]);
    }
    m
}

fn run_dir(os: &str) {
    let dir = fixture_dir(os);
    let entries: Vec<_> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
        .flatten()
        .filter(|e| e.path().extension().map(|x| x == "txt").unwrap_or(false))
        .collect();
    assert!(!entries.is_empty(), "no fixtures captured for {os}");

    let must = must_contain(os);
    let mut total = 0usize;
    let mut empty_skipped = 0usize;
    let mut unrouted: Vec<String> = Vec::new();
    let mut info_violations: Vec<String> = Vec::new();
    let mut routed = 0usize;

    for ent in entries {
        let path = ent.path();
        let stem = path.file_stem().unwrap().to_string_lossy().to_string();
        // Lossy, as real input arrives: a console capture in its OEM code page
        // (`get_childitem_recurse_frfr.txt`, `Répertoire` mangled) is a
        // fixture of exactly that, not a broken file.
        let raw = fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let body = String::from_utf8_lossy(&raw).into_owned();
        if body.trim().is_empty() {
            empty_skipped += 1;
            continue;
        }
        total += 1;
        let (prog, ok) = resolve_prog(&stem, &body);
        if !ok {
            unrouted.push(format!(
                "{stem}.txt ({} bytes) — no program prefix matched",
                body.len()
            ));
            continue;
        }
        routed += 1;
        let compacted = dispatch_compact(&prog, &body).expect("resolve_prog said Some");
        // Qualitative assertion: required tokens must survive.
        if let Some(needles) = must.get(stem.as_str()) {
            for needle in needles {
                if !compacted.contains(needle) {
                    info_violations.push(format!(
                        "{stem}.txt (routed via {prog}): output lost \"{needle}\""
                    ));
                }
            }
        }
    }

    println!(
        "{os}: {routed}/{total} fixtures routed ({empty_skipped} empty), {} info violations",
        info_violations.len()
    );
    if !unrouted.is_empty() {
        println!("UNROUTED {os}:");
        for u in &unrouted {
            println!("  - {u}");
        }
    }
    if !info_violations.is_empty() {
        println!("INFO VIOLATIONS {os}:");
        for v in &info_violations {
            println!("  - {v}");
        }
    }
    assert!(
        unrouted.is_empty(),
        "{} {os} fixtures unrouted",
        unrouted.len()
    );
    assert!(
        info_violations.is_empty(),
        "{} {os} info-preservation violations",
        info_violations.len()
    );
}

#[test]
fn windows_vm_fixtures_route_through_dispatch_named() {
    run_dir("windows");
}

#[test]
fn macos_vm_fixtures_route_through_dispatch_named() {
    run_dir("macos");
}

/// Returns the must-contain map for a locale subdir. Same fixture
/// stems as the root macOS dir but the asserted tokens are tightened
/// to language-specific months / glyphs to prove that:
///   (a) the compactor doesn't mangle UTF-8 (Cyrillic, umlauts, ñ),
///   (b) the locale-sensitive substring survives compaction.
fn locale_must_contain(os: &str, locale: &str) -> BTreeMap<&'static str, Vec<&'static str>> {
    let mut m = BTreeMap::new();
    match (os, locale) {
        ("macos", "de_DE") => {
            m.insert("ls_l_etc", vec!["Okt"]);
            m.insert("ls_lah_etc", vec!["Okt"]);
            m.insert("locale_cmd", vec!["de_DE"]);
        }
        ("macos", "es_ES") => {
            m.insert("date", vec!["sábado"]);
            m.insert("locale_cmd", vec!["es_ES"]);
        }
        ("macos", "ru_RU") => {
            m.insert("date", vec!["суббота"]); // Saturday in Russian
            m.insert("ls_l_etc", vec!["окт"]);
            m.insert("locale_cmd", vec!["ru_RU"]);
        }
        ("windows", "de_DE") => {
            // PowerShell CurrentCulture changes .NET formatting.
            m.insert("date_long", vec!["Samstag"]); // Saturday in German
            m.insert("month_long", vec!["Mai"]);
            m.insert("number_currency", vec!["€"]);
        }
        ("windows", "es_ES") => {
            m.insert("date_long", vec!["sábado"]);
            m.insert("month_long", vec!["mayo"]);
        }
        ("windows", "ru_RU") => {
            m.insert("date_long", vec!["мая"]); // "of May" in Russian
            m.insert("month_long", vec!["Май"]); // "May" capitalised
        }
        _ => {}
    }
    m
}

fn run_locale_dir_for_os(os: &str, locale: &str) {
    let mut dir = fixture_dir(os);
    dir.push(format!("locale_{locale}"));
    if !dir.exists() {
        return;
    }
    let entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
        .flatten()
        .filter(|e| e.path().extension().map(|x| x == "txt").unwrap_or(false))
        .collect();
    assert!(!entries.is_empty(), "{os} locale_{locale} dir is empty");

    let must = locale_must_contain(os, locale);
    let mut unrouted: Vec<String> = Vec::new();
    let mut info_violations: Vec<String> = Vec::new();
    let mut routed = 0usize;

    for ent in entries {
        let path = ent.path();
        let stem = path.file_stem().unwrap().to_string_lossy().to_string();
        let body = fs::read_to_string(&path).unwrap();
        if body.trim().is_empty() {
            continue;
        }
        let (mut prog, mut ok) = resolve_prog(&stem, &body);
        // Windows locale fixtures are PowerShell-expression captures
        // (`culture`, `month_long`, `number_currency`, …) — not real
        // program names. Fall back to the powershell compactor.
        if !ok && os == "windows" {
            if let Some(out) = dispatch_named("powershell", &body) {
                let _ = out;
                prog = "powershell".to_string();
                ok = true;
            }
        }
        if !ok {
            unrouted.push(format!("{stem}.txt — no prefix matched"));
            continue;
        }
        routed += 1;
        let compacted = dispatch_compact(&prog, &body).unwrap();
        if let Some(needles) = must.get(stem.as_str()) {
            for n in needles {
                if !compacted.contains(n) {
                    info_violations.push(format!(
                        "{os}/locale_{locale}/{stem}.txt (routed via {prog}): lost \"{n}\""
                    ));
                }
            }
        }
    }

    println!(
        "{os}/locale_{locale}: {routed} routed, {} unrouted, {} info violations",
        unrouted.len(),
        info_violations.len(),
    );
    for u in &unrouted {
        println!("  UNROUTED - {u}");
    }
    for v in &info_violations {
        println!("  VIOLATION - {v}");
    }
    assert!(
        unrouted.is_empty(),
        "{} unrouted {os}/locale_{locale}",
        unrouted.len()
    );
    assert!(
        info_violations.is_empty(),
        "{} info violations in {os}/locale_{locale}",
        info_violations.len()
    );
}

#[test]
fn locale_de_DE_fixtures_route_and_preserve_unicode() {
    run_locale_dir_for_os("macos", "de_DE");
    run_locale_dir_for_os("windows", "de_DE");
}

#[test]
fn locale_es_ES_fixtures_route_and_preserve_unicode() {
    run_locale_dir_for_os("macos", "es_ES");
    run_locale_dir_for_os("windows", "es_ES");
}

#[test]
fn locale_ru_RU_fixtures_route_and_preserve_unicode() {
    run_locale_dir_for_os("macos", "ru_RU");
    run_locale_dir_for_os("windows", "ru_RU");
}

/// `syslocale_*/` fixtures were captured AFTER the user installed
/// real language packs on the Windows VM (de-DE / es-MX / pt-PT) and
/// after the persistent `Set-Culture` call took effect. PowerShell
/// cmdlets and .NET-formatted output respect this; pure cmd.exe
/// builtins (systeminfo/dir/sc query) still need `Set-WinSystemLocale`
/// + reboot, so we only assert PS-output content here.
#[test]
fn syslocale_de_DE_windows_fixtures_route_and_preserve_unicode() {
    run_syslocale_dir("de_DE");
}

#[test]
fn syslocale_es_MX_windows_fixtures_route_and_preserve_unicode() {
    run_syslocale_dir("es_MX");
}

#[test]
fn syslocale_pt_PT_windows_fixtures_route_and_preserve_unicode() {
    run_syslocale_dir("pt_PT");
}

fn run_syslocale_dir(locale: &str) {
    let mut dir = fixture_dir("windows");
    dir.push(format!("syslocale_{locale}"));
    if !dir.exists() {
        return;
    }
    let entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
        .flatten()
        .filter(|e| e.path().extension().map(|x| x == "txt").unwrap_or(false))
        .collect();
    assert!(!entries.is_empty(), "syslocale_{locale} is empty");

    // Locale-neutral tokens that should always survive.
    let must: BTreeMap<&'static str, Vec<&'static str>> = {
        let mut m = BTreeMap::new();
        match locale {
            "de_DE" => {
                m.insert("get_date_long", vec!["Samstag"]);
                m.insert("month_long", vec!["Mai"]);
            }
            "es_MX" => {
                m.insert("get_date_long", vec!["mayo"]);
                m.insert("month_long", vec!["mayo"]);
            }
            "pt_PT" => {
                m.insert("get_date_long", vec!["maio"]);
                m.insert("month_long", vec!["maio"]);
            }
            _ => {}
        }
        m
    };

    let mut routed = 0usize;
    let mut unrouted: Vec<String> = Vec::new();
    let mut info_violations: Vec<String> = Vec::new();

    for ent in entries {
        let path = ent.path();
        let stem = path.file_stem().unwrap().to_string_lossy().to_string();
        let body = std::fs::read_to_string(&path).unwrap();
        if body.trim().is_empty() {
            continue;
        }
        let (mut prog, mut ok) = resolve_prog(&stem, &body);
        if !ok {
            // PS expressions: fall back to powershell compactor.
            if dispatch_named("powershell", &body).is_some() {
                prog = "powershell".to_string();
                ok = true;
            }
        }
        if !ok {
            unrouted.push(format!("{stem}.txt"));
            continue;
        }
        routed += 1;
        let compacted = dispatch_compact(&prog, &body).unwrap();
        if let Some(needles) = must.get(stem.as_str()) {
            for n in needles {
                if !compacted.contains(n) {
                    info_violations.push(format!(
                        "syslocale_{locale}/{stem}.txt (via {prog}): lost \"{n}\""
                    ));
                }
            }
        }
    }
    println!(
        "syslocale_{locale}: {routed} routed, {} unrouted, {} info violations",
        unrouted.len(),
        info_violations.len()
    );
    for u in &unrouted {
        println!("  UNROUTED - {u}");
    }
    for v in &info_violations {
        println!("  VIOLATION - {v}");
    }
    assert!(
        unrouted.is_empty(),
        "{} syslocale_{locale} unrouted",
        unrouted.len()
    );
    assert!(
        info_violations.is_empty(),
        "{} info violations syslocale_{locale}",
        info_violations.len()
    );
}

/// Compression-ratio assertion: for every fixture whose raw body is
/// large enough that a compactor *should* visibly shrink it (>=2 KB),
/// verify the compacted output is smaller. Catches regressions where
/// a compactor degrades to identity (returns text unchanged) or, worse,
/// grows the output via verbose annotations.
///
/// Below the size threshold we don't assert anything — short fixtures
/// (a `whoami`, a `pwd`) legitimately pass through unchanged.
#[test]
fn compactors_actually_compress_large_fixtures() {
    const THRESHOLD_BYTES: usize = 2048;
    let mut regressions = Vec::new();
    let mut large_count = 0usize;
    let mut wins = 0usize;
    // Aggregate, in TOKENS: bytes over-weight whitespace, which is exactly what
    // these compactors strip, so a byte ratio flatters them.
    let (mut tok_before, mut tok_after) = (0usize, 0usize);

    for os in ["macos", "windows"] {
        let dir = fixture_dir(os);
        for ent in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = ent.path();
            if path.extension().map(|x| x != "txt").unwrap_or(true) {
                continue;
            }
            let body = match std::fs::read_to_string(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            if body.len() < THRESHOLD_BYTES {
                continue;
            }
            large_count += 1;
            let stem = path.file_stem().unwrap().to_string_lossy().to_string();
            let (prog, ok) = resolve_prog(&stem, &body);
            if !ok {
                continue; // covered by routing test
            }
            let compacted = dispatch_compact(&prog, &body).unwrap();
            // Pass-through compactors don't shrink. We only require a
            // strict shrink when the compactor's archetype is cap/strip
            // (i.e. the output is shorter, or at minimum not larger).
            if compacted.len() > body.len() {
                regressions.push(format!(
                    "{os}/{stem}.txt: compactor GREW the output ({} → {} bytes)",
                    body.len(),
                    compacted.len()
                ));
            } else if compacted.len() < body.len() {
                wins += 1;
            }
            tok_before += tokenade_core::tokenizer::estimate(&body);
            tok_after += tokenade_core::tokenizer::estimate(&compacted);
            // Per-fixture minimum-reduction floor: catches a compactor that
            // degrades to (near-)identity on output we know is noisy.
            if let Some(floor) = min_saved_percent(os, &stem) {
                let pct = saved_pct(&body, &compacted);
                if pct < floor {
                    regressions.push(format!(
                        "{os}/{stem}.txt: saved {pct:.0}% < min_saved_percent {floor:.0}%"
                    ));
                }
            }
        }
    }
    let corpus_saved = if tok_before > 0 {
        100.0 * (tok_before - tok_after) as f64 / tok_before as f64
    } else {
        0.0
    };
    println!(
        "compression assertion: {large_count} fixtures ≥{THRESHOLD_BYTES} B; {wins} compressed; \
         corpus {corpus_saved:.1}% saved ({tok_before} → {tok_after} tok); {} flagged (grew or below floor)",
        regressions.len()
    );

    // Two AGGREGATE floors. Until now `wins` and the corpus ratio were printed
    // and never asserted: a change that quietly stopped compacting most command
    // output would have left this test green, because the only hard assertion
    // was "nothing GREW". Identity output never grows.
    //
    // Only three fixtures carry a per-fixture floor (all Windows), and a blanket
    // one would be wrong here — plenty of command output is legitimately dense
    // and compacts to itself. So the guard is on the corpus, not the item.
    //
    // Measured 2026-08-03: 39 large fixtures, 34 compressing. Floors sit below
    // that so a genuinely incompressible fixture can join without a false alarm.
    const MIN_COMPRESSING_FRACTION: f64 = 70.0;
    const MIN_CORPUS_SAVED_PCT: f64 = 25.0;
    if large_count > 0 {
        let frac = 100.0 * wins as f64 / large_count as f64;
        assert!(
            frac >= MIN_COMPRESSING_FRACTION,
            "only {wins}/{large_count} ({frac:.0}%) large fixtures still compress, \
             floor is {MIN_COMPRESSING_FRACTION:.0}% — the compactors are drifting \
             toward identity, which no other assertion here would catch"
        );
        assert!(
            corpus_saved >= MIN_CORPUS_SAVED_PCT,
            "corpus compaction fell to {corpus_saved:.1}%, floor is {MIN_CORPUS_SAVED_PCT}%"
        );
    }
    for r in &regressions {
        println!("  REGRESSION - {r}");
    }
    assert!(
        regressions.is_empty(),
        "{} compactor(s) failed the size/floor check:\n{}",
        regressions.len(),
        regressions.join("\n")
    );
}

/// The hook feeds a compactor the COMBINED stream (`stdout` ++ `stderr`), because
/// tools like cargo/biome write their substantive output to STDERR while stdout
/// is empty. This models that split-stream contract: a filter must still compact
/// when the payload is on stderr. It would FAIL if the harness (or the hook) fed
/// only stdout — the filter would see an empty string, save 0%, and miss the
/// floor. This is the classic "0% saved but the test is green" trap.
fn combined_output(stdout: &str, stderr: &str) -> String {
    // Mirrors tokenade-cli hookwrap::combined_output.
    format!("{stdout}{stderr}")
}

#[test]
fn filter_compacts_the_correct_stream() {
    // biome-style filter: drop the per-file "checked" progress spam.
    let toml = "name=\"biome\"\nmatch_command=\"^biome\"\nstrip_lines_matching=[\"checked\"]\n";
    let f = tokenade_core::user_filters::parse_and_validate(toml).expect("filter compiles");

    // biome writes progress to STDERR; stdout is empty. The summary line uses a
    // capital "Checked" so the lowercase strip pattern keeps it.
    let stdout = "";
    let mut stderr = String::new();
    for i in 0..80 {
        stderr.push_str(&format!("./src/f{i}.ts checked\n"));
    }
    stderr.push_str("Checked 80 files: 0 errors\n");
    let min_saved = 80.0_f64;

    // WRONG stream (stdout only): empty payload, nothing to compact.
    let wrong = f.apply(stdout);
    let wrong_saved = saved_pct(stdout, &wrong);
    assert!(
        wrong_saved < min_saved,
        "case is not stream-sensitive: reading stdout alone already saves {wrong_saved:.0}%"
    );

    // CORRECT: the hook's combined stream carries the stderr payload.
    let combined = combined_output(stdout, &stderr);
    let right = f.apply(&combined);
    let right_saved = saved_pct(&combined, &right);
    assert!(
        right.contains("Checked 80 files: 0 errors"),
        "summary must survive: {right:?}"
    );
    assert!(
        right_saved >= min_saved,
        "filter on the combined stream saved {right_saved:.0}% < floor {min_saved:.0}% \
         (wrong-stream would save {wrong_saved:.0}%)"
    );
}
