//! Per-command compactors.
//!
//! Each shell command we recognise lives in its OWN file under one of
//! three OS-keyed submodules: [`linux`], [`macos`], [`windows`].
//!
//! ## Why one file per command?
//!
//! The corpus runs into the high hundreds today, the thousands soon
//! (bash_logs corpus alone has 3000+ unique program bases). One file
//! per command keeps detectors, fixtures, and locale variants
//! decoupled — a Windows-only test failure can't pollute a Linux
//! refactor.
//!
//! ## Mass module discovery
//!
//! `build.rs` walks each `cmd/<os>/` directory at compile time and
//! emits `pub mod <name>;` declarations into `cmd_<os>_mods.rs`,
//! pulled in via `include!`. **Adding a new compactor never requires
//! touching `mod.rs`** — just drop the file in place.
//!
//! ## Dispatch
//!
//! - [`dispatch(text)`] — content-shape detection ladder (used when
//!   the program name is unknown).
//! - [`dispatch_named(prog, text)`] — fast name-based routing. The
//!   table below covers the *complex* cases (multi-alias commands
//!   like `iptables`/`ip6tables`/`nft`, OS-priority fallthrough for
//!   names that exist on Linux + macOS + Windows). Trivial 1:1
//!   filename-to-module routes are handled automatically by the
//!   build-script-generated `dispatch_auto` fallback at the tail.
//!
//! ## Locale handling
//!
//! Many commands emit headers / status words / summaries in the
//! system locale. Parsers MUST tolerate at minimum English, French,
//! German, Spanish, Italian, Portuguese, and Russian. Locale-
//! invariant token shapes (`<num>%`, `\d+(\.\d+)?[KMGT]i?[Bb]?`) win
//! over hard-coded English keywords. See [`locale`] for the shared
//! regex / keyword sets.

pub mod families;
pub mod linux;
pub mod macos;
pub mod spec;
mod spec_fold_parity;
pub mod spec_table;
pub mod spec_table_folds;
pub mod windows;

// build.rs-generated trailing fallback.
include!(concat!(env!("OUT_DIR"), "/cmd_auto_dispatch.rs"));

/// Run a content-shape detection ladder. Returns `Some(compacted)`
/// when any per-command compactor recognises the output. Used when
/// the program name is unknown (raw output came back over a pipe).
pub fn dispatch(text: &str) -> Option<String> {
    if let Some(out) = linux::dispatch(text) {
        return Some(out);
    }
    if let Some(out) = macos::dispatch(text) {
        return Some(out);
    }
    if let Some(out) = windows::dispatch(text) {
        return Some(out);
    }
    None
}

/// Look up a compactor by program name. Multi-alias commands and
/// OS-fallthrough routes live here as explicit arms. The trailing
/// `_` arm hands off to the build-script-generated `dispatch_auto`
/// table, which covers the simple `filename → module` routes for
/// every per-command file on disk.
pub fn dispatch_named(prog: &str, text: &str) -> Option<String> {
    // Phase 1: case-exact match. Zero behaviour change for the Unix world,
    // where program names ARE case-sensitive (`Make` ≠ `make`) and every
    // generated dispatch key is already lowercase.
    if let Some(out) = dispatch_named_cased(prog, text) {
        return Some(out);
    }
    // Phase 2: case-insensitive retry. Windows command + PowerShell cmdlet
    // names are case-insensitive by design — an agent writes the canonical
    // `Get-ChildItem` / `Get-Process` / `Invoke-WebRequest`, but every key
    // in the dispatch table is lowercase, so the exact match above always
    // misses and the whole `cmd/windows/get_*|invoke_*|set_*` family is
    // unreachable. Retrying lowercased reaches it. Safe in both directions:
    // (a) it only fires when `prog` actually contains uppercase, so a Unix
    // lowercase name never triggers a second lookup; (b) every `try_compact`
    // is content-defensive (`if !detect(text) { return None }`), so routing
    // unrelated output to a wrongly-guessed compactor just yields `None`.
    let lower = prog.to_ascii_lowercase();
    if lower != prog {
        return dispatch_named_cased(&lower, text);
    }
    None
}

/// Case-exact name routing — the actual dispatch table. Wrapped by
/// [`dispatch_named`], which adds a case-insensitive retry for Windows /
/// PowerShell command names. See that function for why the retry is safe.
fn dispatch_named_cased(prog: &str, text: &str) -> Option<String> {
    match prog {
        // ─── multi-alias families ───
        "ping" | "ping6" => {
            linux::ping::try_compact(text).or_else(|| macos::ping_macos::try_compact(text))
        }
        "iptables" | "iptables-save" | "ip6tables" | "nft" => linux::iptables::try_compact(text),
        // `set` dumps the same `NAME=value` lines (POSIX shells, and cmd.exe):
        // it was a head/tail table row that cut the MIDDLE of the dump — `PATH`,
        // `PWD`, `SHELL`, `USER` among the 53 lines lost on a real `set` — and
        // redacted nothing.
        "env" | "printenv" | "set" => linux::env::try_compact(text),
        // PowerShell's grep prints grep's `path:line:text` (real capture,
        // 2026-10-05: 16 741 bytes kept at 73 % as generic text).
        "select-string" | "sls" => {
            Some(linux::grep::compact(text)).filter(|c| c.len() < text.len())
        }
        "who" | "w" => linux::who::try_compact(text),
        "last" | "lastlog" | "lastb" => linux::last::try_compact(text),
        "uname" | "hostnamectl" => linux::uname::try_compact(text),
        "pip" | "pip3" => linux::pip::try_compact(text),
        "apropos" | "whatis" => linux::apropos::try_compact(text),
        "at" | "atq" => linux::at::try_compact(text),
        "locate" | "mlocate" | "plocate" => linux::locate::try_compact(text),
        "fsck" | "e2fsck" | "fsck.ext4" | "fsck.ext3" | "fsck.ext2" | "fsck.xfs" => {
            linux::fsck::try_compact(text)
        }
        "traceroute" | "tracepath" | "traceroute6" => linux::traceroute::try_compact(text),
        "bzip2" | "bunzip2" => linux::bzip2::try_compact(text),
        "cal" | "ncal" => linux::cal::try_compact(text),
        "gzip" | "gunzip" => linux::gzip::try_compact(text),
        "nc" | "ncat" => linux::nc::try_compact(text),
        "sha256sum" | "sha1sum" | "sha512sum" | "md5sum" => linux::sha256sum::try_compact(text),
        "zip" | "unzip" => linux::zip::try_compact(text),

        // ─── OS-priority fallthrough for shared names ───
        // (Linux first, macOS, then Windows.)
        "ifconfig" => {
            linux::ifconfig::try_compact(text).or_else(|| macos::ifconfig::try_compact(text))
        }
        "id" => linux::id_cmd::try_compact(text).or_else(|| macos::id_macos::try_compact(text)),
        "ps" => linux::ps_cmd::try_compact(text).or_else(|| macos::ps::try_compact(text)),
        "stat" => {
            linux::stat_cmd::try_compact(text).or_else(|| macos::stat_macos::try_compact(text))
        }
        "tar" => linux::tar::try_compact(text).or_else(|| windows::tar_win::try_compact(text)),
        "ssh" => linux::ssh_cmd::try_compact(text).or_else(|| windows::ssh_win::try_compact(text)),
        "klist" => {
            linux::klist::try_compact(text).or_else(|| windows::klist_win::try_compact(text))
        }
        "nslookup" => {
            linux::nslookup::try_compact(text).or_else(|| windows::nslookup_win::try_compact(text))
        }
        // Linux `netstat`/`ss`/`lsof -i` first — its compactor collapses by
        // state and keeps every LISTEN entry verbatim. macOS' detector is loose
        // (matches any "Proto"/"LISTEN"), so it must come *after* Linux or it
        // greedily swallows Linux output and barely compacts it.
        "netstat" => crate::netstat::try_compact(text)
            .or_else(|| windows::netstat::try_compact(text))
            .or_else(|| macos::netstat::try_compact(text)),
        // `ss` is always socket output, so the same compactor applies. (`lsof`
        // is NOT here: a bare `lsof` lists open *files* too, and rebuilding a
        // connection summary would drop them — it is gated on `-i` in the
        // sysinfo dispatcher, where argv is visible.)
        "ss" => crate::netstat::try_compact(text),
        "type" => {
            linux::type_cmd::try_compact(text).or_else(|| windows::type_win::try_compact(text))
        }
        "fc" => macos::fc::try_compact(text).or_else(|| windows::fc_cmd::try_compact(text)),
        // **`net` exists on both systems, and NEITHER detector
        // knows which.** The macOS one answers « yes » to any text carrying a
        // `:` or a `.`, the Windows one to any non-empty text: the second
        // arm of an `or_else` was therefore DEAD, and the banner announced
        // `[tokenade:net-macos]` on a Windows `net start` output —
        // measured on the corpus capture `windows/net_start_full`.
        //
        // The two do different and complementary work: Windows
        // removes its conclusion line, macOS folds by shape. So we apply
        // BOTH, in that order, under a label that claims nothing.
        "net" => net_either_platform(text),
        "openssl" => linux::openssl_cli::try_compact(text)
            .or_else(|| macos::openssl_macos::try_compact(text)),
        "tcpdump" => macos::tcpdump::try_compact(text),
        "shutdown" => linux::shutdown_cmd::try_compact(text)
            .or_else(|| windows::shutdown_win::try_compact(text)),
        "whoami" => linux::whoami::try_compact(text)
            .or_else(|| macos::whoami_macos::try_compact(text))
            .or_else(|| windows::whoami_win::try_compact(text)),
        "hostname" => {
            linux::hostname::try_compact(text).or_else(|| windows::hostname_win::try_compact(text))
        }
        "curl" => linux::curl::try_compact(text).or_else(|| windows::curl_win::try_compact(text)),
        "date" => linux::date::try_compact(text).or_else(|| windows::date_cmd::try_compact(text)),
        "expand" => {
            linux::expand::try_compact(text).or_else(|| windows::expand_cmd::try_compact(text))
        }
        "popd" => linux::popd::try_compact(text).or_else(|| windows::popd_cmd::try_compact(text)),
        "pushd" => {
            linux::pushd::try_compact(text).or_else(|| windows::pushd_cmd::try_compact(text))
        }
        "install" => {
            linux::install::try_compact(text).or_else(|| macos::installer_macos::try_compact(text))
        }
        "logname" => {
            linux::logname::try_compact(text).or_else(|| macos::logname_macos::try_compact(text))
        }
        "timeout" => {
            linux::timeout::try_compact(text).or_else(|| windows::timeout_cmd::try_compact(text))
        }
        "whereis" => {
            linux::whereis::try_compact(text).or_else(|| macos::whereis_macos::try_compact(text))
        }
        "which" => {
            linux::which_cmd::try_compact(text).or_else(|| macos::which_macos::try_compact(text))
        }
        "write" => {
            linux::write_msg::try_compact(text).or_else(|| macos::wall_macos::try_compact(text))
        }
        "uptime" => {
            linux::uptime_cmd::try_compact(text).or_else(|| macos::uptime_macos::try_compact(text))
        }

        // ─── short-name → suffixed-filename routes ───
        "sc" => windows::sc_query::try_compact(text),
        "where" => windows::where_exe::try_compact(text),

        // ─── case-sensitive aliases (Sysinternals capitalisation, BSD CamelCase) ───
        "AccessChk" | "accesschk" => windows::accesschk::try_compact(text),
        "ADDUSERS" | "addusers" => windows::addusers::try_compact(text),
        "ScriptRunner" | "scriptrunner" => windows::scriptrunner::try_compact(text),
        "GetFileInfo" | "getfileinfo" => macos::getfileinfo::try_compact(text),
        "SetFile" | "setfile" => macos::setfile::try_compact(text),

        // ─── alias variants with dotted / underscore names ───
        "ntfs.util" | "ntfs_util" => macos::ntfs_util::try_compact(text),
        "manage-bde" | "manage_bde" => windows::manage_bde::try_compact(text),
        "ansible-playbook" => {
            linux::ansible_playbook::try_compact(text).or_else(|| linux::ansible::try_compact(text))
        }
        "notify-send" => linux::notify_send::try_compact(text),
        "youtube-dl" | "yt-dlp" => linux::youtube_dl::try_compact(text),
        // Worktrunk before the table's `wt`, which is Windows Terminal: the
        // table's head-and-tail cut the verdict of the hooks `wt merge` runs.
        // A recursive listing before the table spec, whose 50-row head hid
        // every directory but the first.
        "get-childitem" | "get_childitem" | "gci" => windows::get_childitem::try_compact(text)
            .or_else(|| spec::dispatch("get_childitem", text)),
        "wt" | "git-wt" => {
            linux::worktrunk::try_compact(text).or_else(|| spec::dispatch("wt", text))
        }

        // ─── trailing fallback: auto-generated 1:1 routes ───
        // Data-table built-ins (Phase A): byte-identical to the stub they
        // replaced (parity-enforced in `spec::tests`), checked before the
        // generated per-file `dispatch_auto` fallback. Explicit arms above
        // still win. Keys not in the table fall through to their stub module.
        _ => spec::dispatch(prog, text).or_else(|| dispatch_auto(prog, text)),
    }
}

/// `net`, without claiming to know which system we run on.
///
/// The two `net` compactors do different work — Windows removes its
/// conclusion line, macOS folds by shape — and neither detector
/// establishes the platform. Chaining them with `or_else` made the second
/// unreachable and made `[tokenade:net-macos]` be announced on a Windows
/// output. Here both passes apply, and the label names nobody.
fn net_either_platform(text: &str) -> Option<String> {
    let stripped = windows::net_win::compact(text);
    let folded = macos::net_macos::try_compact(&stripped);
    match folded {
        Some(f) => Some(f),
        // The fold declined, but removing the conclusion may have
        // produced something: do not throw it away.
        None if stripped.len() < text.len() => Some(stripped),
        None => None,
    }
}

/// Every program name that appears in an explicit arm of [`dispatch_named`]:
/// multi-alias families (`ping6`, `ncal`, `yt-dlp`, …), OS-priority
/// fallthrough names (`ifconfig`, `ps`, `stat`, …), and short-name routes
/// (`sc`, `where`, …).
///
/// These programs have NO dedicated `.rs` module file, so they are invisible
/// to [`dispatch_auto_has_prog`] / `AUTO_DISPATCH_PROGS`. Kept as a slice
/// (rather than a `matches!`) so callers like the shell proxy can enumerate
/// the full known-program set; a unit test asserts every entry actually
/// routes through `dispatch_named`.
pub static NAMED_EXTRA_PROGS: &[&str] = &[
    // multi-alias families
    "select-string",
    "sls",
    "gci",
    "ping",
    "ping6",
    "iptables",
    "iptables-save",
    "ip6tables",
    "nft",
    "env",
    "printenv",
    "set",
    "who",
    "w",
    "last",
    "lastlog",
    "lastb",
    "uname",
    "hostnamectl",
    "pip",
    "pip3",
    "apropos",
    "whatis",
    "at",
    "atq",
    "locate",
    "mlocate",
    "plocate",
    "fsck",
    "e2fsck",
    "fsck.ext4",
    "fsck.ext3",
    "fsck.ext2",
    "fsck.xfs",
    "traceroute",
    "tracepath",
    "traceroute6",
    "bzip2",
    "bunzip2",
    "cal",
    "ncal",
    "gzip",
    "gunzip",
    "nc",
    "ncat",
    "sha256sum",
    "sha1sum",
    "sha512sum",
    "md5sum",
    "zip",
    "unzip",
    // OS-priority fallthrough
    "ifconfig",
    "id",
    "ps",
    "stat",
    "tar",
    "ssh",
    "klist",
    "nslookup",
    "netstat",
    "type",
    "fc",
    "net",
    "openssl",
    "tcpdump",
    "shutdown",
    "whoami",
    "hostname",
    "curl",
    "date",
    "expand",
    "popd",
    "pushd",
    "install",
    "logname",
    "timeout",
    "whereis",
    "which",
    "write",
    "uptime",
    // short-name → suffixed-filename routes
    "sc",
    "where",
    // case-sensitive aliases
    "AccessChk",
    "accesschk",
    "ADDUSERS",
    "addusers",
    "ScriptRunner",
    "scriptrunner",
    "GetFileInfo",
    "getfileinfo",
    "SetFile",
    "setfile",
    // dotted / underscore aliases
    "ntfs.util",
    "ntfs_util",
    "manage-bde",
    "manage_bde",
    "ansible-playbook",
    "notify-send",
    "youtube-dl",
    "yt-dlp",
];

/// Returns `true` for every name in [`NAMED_EXTRA_PROGS`]. See the slice doc
/// for what lives there. [`dispatch_compact`] checks both this and
/// [`dispatch_auto_has_prog`] so that recognised-but-short content for
/// alias-only programs is correctly identified as "known" rather than
/// "unknown".
pub fn dispatch_named_has_prog(prog: &str) -> bool {
    NAMED_EXTRA_PROGS.contains(&prog)
}

/// Like `dispatch_named`, but returns `Some` whenever the program is *known*
/// even if the content is too short to require folding (passthrough case).
/// Returns `None` only for truly unknown program names.
///
/// Used by tests to distinguish "routing failure" from "recognised but short":
/// `dispatch_named` conflates the two (both return `None`), whereas this
/// function returns `Some(passthrough)` for the latter.
pub fn dispatch_compact(prog: &str, text: &str) -> Option<String> {
    dispatch_named(prog, text).or_else(|| {
        if dispatch_auto_has_prog(prog) || dispatch_named_has_prog(prog) || spec::has_key(prog) {
            Some(crate::archetypes::passthrough(text))
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_returns_none() {
        assert!(dispatch_named("not-a-real-cmd-name-xyz", "anything").is_none());
    }

    #[test]
    fn whoami_routes_via_explicit_arm() {
        let out = dispatch_named("whoami", "root\n");
        assert_eq!(out.as_deref(), Some("root"));
    }

    #[test]
    fn auto_dispatch_handles_simple_routes() {
        // `brew` has no explicit arm; reaches `dispatch_auto`.
        let out = dispatch_named("brew", "==> Auto-updating Homebrew...\nupdated\n");
        assert!(out.is_some(), "expected brew to route via dispatch_auto");
    }

    #[test]
    fn auto_dispatch_handles_windows_route() {
        // chkdsk is a recognised command, so the prod entry point
        // `dispatch_compact` must always yield Some — it falls back to
        // passthrough when the per-command `try_compact` declines (short
        // output with nothing to fold). `dispatch_named` alone may now
        // return None for such input: form-aware compactors return None
        // when there is nothing to compact, instead of the old
        // stub-always-Some passthrough. The prod contract is in dispatch_compact.
        let out = dispatch_compact(
            "chkdsk",
            "File data verification completed.\nWindows has scanned the file system.\n",
        );
        assert!(
            out.is_some(),
            "expected chkdsk to route via dispatch_compact"
        );
    }

    #[test]
    fn dispatch_named_has_prog_entries_are_all_reachable() {
        // Every name listed in dispatch_named_has_prog must still have an arm
        // in dispatch_named. Guards against stale entries where an arm was
        // removed from dispatch_named but not from has_prog.
        // Note: dispatch_compact always returns Some(passthrough) for any
        // has_prog name even when dispatch_named returns None for short input,
        // so it cannot distinguish routing failures from "recognised but short".
        // We therefore only verify the has_prog membership here; the companion
        // test below verifies actual compaction with realistic input.
        for prog in [
            // multi-alias families
            "ping",
            "ping6",
            "iptables",
            "ip6tables",
            "nft",
            "env",
            "printenv",
            "set",
            "who",
            "w",
            "last",
            "lastlog",
            "lastb",
            "uname",
            "hostnamectl",
            "pip",
            "pip3",
            "traceroute",
            "tracepath",
            "cal",
            "ncal",
            "gzip",
            "gunzip",
            "nc",
            "ncat",
            "sha256sum",
            "sha1sum",
            "md5sum",
            "zip",
            "unzip",
            // OS-priority fallthrough
            "ifconfig",
            "id",
            "ps",
            "stat",
            "tar",
            "ssh",
            "nslookup",
            "netstat",
            "openssl",
            "whoami",
            "hostname",
            "curl",
            "date",
            "timeout",
            "which",
            "uptime",
            // short-name / alias routes
            "sc",
            "where",
            // dotted / hyphen aliases
            "ansible-playbook",
            "yt-dlp",
            "youtube-dl",
        ] {
            assert!(
                dispatch_named_has_prog(prog),
                "{prog}: listed as expected but dispatch_named_has_prog returned false",
            );
        }
    }

    #[test]
    fn dispatch_named_compacts_representative_programs_with_realistic_input() {
        // Verify that dispatch_named actually routes and compacts output for a
        // handful of programs using input that matches their real output shape.
        // These are all alias-only programs (no .rs module) that live exclusively
        // in dispatch_named explicit arms — if an arm is removed, this test fails.
        assert_eq!(
            dispatch_named("whoami", "root\n").as_deref(),
            Some("root"),
            "whoami: expected compacted to strip trailing newline",
        );
        // ping: realistic output with RTT lines that the compactor summarises.
        let ping_out = "PING 8.8.8.8 (8.8.8.8) 56(84) bytes of data.\n\
            64 bytes from 8.8.8.8: icmp_seq=1 ttl=117 time=12.3 ms\n\
            64 bytes from 8.8.8.8: icmp_seq=2 ttl=117 time=11.8 ms\n\
            64 bytes from 8.8.8.8: icmp_seq=3 ttl=117 time=12.1 ms\n\
            --- 8.8.8.8 ping statistics ---\n\
            3 packets transmitted, 3 received, 0% packet loss, time 2002ms\n\
            rtt min/avg/max/mdev = 11.800/12.066/12.300/0.208 ms\n";
        assert!(
            dispatch_named("ping", ping_out).is_some(),
            "ping: dispatch_named must return Some for RTT output",
        );
        assert!(
            dispatch_named("ping6", ping_out).is_some(),
            "ping6 alias: must route to same compactor as ping",
        );
    }

    #[test]
    fn powershell_cmdlets_route_in_canonical_camelcase() {
        // PowerShell cmdlet names are case-insensitive; an agent writes the
        // canonical CamelCase (`Get-Process`, never `get-process`). The
        // dispatch table keys are all lowercase, so without the case-
        // insensitive retry in dispatch_named these compactors are dead code.
        // This exercises the REAL casing — vm_fixtures::resolve_prog derives
        // the program from the lowercase fixture filename and so never does.
        let proc_table = {
            let hdr = "Handles  NPM(K)    PM(K)      WS(K)     CPU(s)     Id  SI ProcessName\n\
                       -------  ------    ----      -----     ------     --  -- -----------\n";
            let rows: String = (0..80)
                .map(|i| {
                    format!("    {i:3}      12    3456     7890       0.12   {i:4}   1 proc{i}\n")
                })
                .collect();
            format!("{hdr}{rows}")
        };
        assert!(
            dispatch_named("Get-Process", &proc_table).is_some(),
            "Get-Process (canonical CamelCase) must fold the process table",
        );
        // Lowercase must still work and produce the identical result.
        assert_eq!(
            dispatch_named("Get-Process", &proc_table),
            dispatch_named("get-process", &proc_table),
            "CamelCase and lowercase cmdlet must route to the same compactor",
        );

        let svc_table = {
            let hdr = "Status   Name               DisplayName\n\
                       ------   ----               -----------\n";
            let rows: String = (0..80)
                .map(|i| format!("Running  svc{i:<14} Display Name For Service {i}\n"))
                .collect();
            format!("{hdr}{rows}")
        };
        assert!(
            dispatch_named("Get-Service", &svc_table).is_some(),
            "Get-Service (canonical CamelCase) must fold the service table",
        );

        let listing: String = (0..60)
            .map(|i| format!("-a----  2026-06-13  10:0{}  {i:6}  file{i}.txt\n", i % 10))
            .collect();
        assert!(
            dispatch_named("Get-ChildItem", &listing).is_some(),
            "Get-ChildItem (canonical CamelCase) must fold the directory listing",
        );
    }
}

#[cfg(test)]
mod path_parity {
    /// The SAME bytes must fold about as hard whether tokenade RAN the command
    /// or was handed its output as content.
    ///
    /// This gap was measured, and it cost 13.7×: `archetypes::kv_records`
    /// folded `pacman -Qi` from 33 599 bytes to 4 039 when the command path
    /// reached it, and the content path — a file, a paste, `read` — had no
    /// kind for that shape and handed it back whole. It was not a missing
    /// compactor, it was a missing ROUTE to one that already existed, and the
    /// only way to see that is to compare the two paths on the same input.
    #[test]
    #[ignore = "measurement over a locally generated corpus; run explicitly"]
    fn command_path_and_content_path_agree() {
        // corpus file → the command that produced it.
        const SAMPLES: &[(&str, &str)] = &[
            ("gitlog.txt", "git"),
            ("gitstatus.txt", "git"),
            ("gitdiff.txt", "git"),
            ("ps.txt", "ps"),
            ("df.txt", "df"),
            ("lsof.txt", "lsof"),
            ("lsof_i.txt", "lsof"),
            ("netstat.txt", "netstat"),
            ("journal.txt", "journalctl"),
            ("systemctl.txt", "systemctl"),
            ("systemctl_show.txt", "systemctl"),
            ("pacman_en.txt", "pacman"),
            ("find.txt", "find"),
            ("cargotree.txt", "cargo"),
            ("cargo_dupes.txt", "cargo"),
            ("piplist.txt", "pip"),
            ("npmls.txt", "npm"),
            ("docker.txt", "docker"),
        ];
        let dir = "/tmp/claude-1000/-home-username--devworkspace-projects-tokenade/67caa6a3-1a47-4a19-b18f-81af922cce79/scratchpad/corpus";
        for (f, prog) in SAMPLES {
            let Ok(t) = std::fs::read_to_string(format!("{dir}/{f}")) else {
                continue;
            };
            if t.len() < 2000 {
                continue;
            }
            let cmd = super::dispatch_auto(prog, &t)
                .map(|o| o.len())
                .unwrap_or(t.len());
            let (kind, content, _s) = crate::auto::compact(&t);
            let (rc, rk) = (
                cmd as f64 / t.len() as f64,
                content.len() as f64 / t.len() as f64,
            );
            let gap = rk / rc.max(0.0001);
            let flag = if gap > 2.0 { "GAP " } else { "ok  " };
            println!("PATHS {flag} {f:<20} cmd={rc:.3} content={rk:.3} ({kind:?}) gap={gap:.1}x");
        }
    }
}
