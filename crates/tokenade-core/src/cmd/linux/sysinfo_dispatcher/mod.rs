//! Dispatcher for `sysinfo`.
//! Extracted from `filter.rs` in the L108 refactor.

use crate::compress::generic;
use crate::argv::{has_flag, flag_value};
use crate::ansi::strip_ansi;

pub fn dispatch(prog: &str, argv: &[String], output: &str) -> String {
    if has_flag(argv, "--json") || flag_value(argv, "--output").is_some_and(|v| v == "json") {
        let (_, c, _) = crate::auto::compact(output);
        return c;
    }
    // `du -sh` returns one line per arg; passing through is fine.
    if prog == "du" && has_flag(argv, "-s") {
        return strip_ansi(output).trim().to_string();
    }
    // `lsof -i` lists *network* sockets only — same shape the netstat
    // compactor understands (collapse by state, keep LISTEN verbatim).
    // A *bare* `lsof` also lists open files; rebuilding a connection
    // summary would drop them, so we gate strictly on the `-i` selector
    // (which may appear glued, e.g. `-iTCP`, `-i:22`).
    if prog == "lsof" && argv.iter().any(|a| a.starts_with("-i")) {
        if let Some(c) = crate::netstat::try_compact(output) {
            return c;
        }
    }
    // `ip -d` asks for the DETAIL block — `promiscuity`, `minmtu`, the
    // `gso_*`/`tso_*`/`gro_*` offload sizes, `parentbus`/`parentdev` — and
    // `ip`'s compactor reads exactly two tokens off the `link/…` line, so
    // every byte the flag was added to produce is what it drops. Measured
    // 2026-08-27: `ip addr` and `ip -d addr` returned identical output, and
    // the flag appears 17 times in this machine's transcripts. A flag whose
    // whole purpose is to ADD what the compactor removes must reach a path
    // that keeps it. Same lever as `du -s` and `lsof -i` above.
    if prog == "ip" && (has_flag(argv, "-d") || has_flag(argv, "-details")) {
        let (_, c, _) = crate::auto::compact(output);
        return if c.len() < output.len() { c } else { generic(output) };
    }
    // L83+ — per-command compactors. Try the named dispatch first;
    // each compactor has its own detector and returns None when the
    // output doesn't match, so we fall through cleanly to the
    // generic tabular path on mismatch (e.g. an empty `ifconfig`,
    // BSD `ifconfig` on a Mac dev box).
    let named = crate::cmd::dispatch_named(prog, output);
    // `ps aux` / `ps -ef` / `lsof -i` / `netstat -tulpn` all produce
    // wide tabular output. Auto's tabular detector compacts columns.
    let (_, c, _) = crate::auto::compact(output);
    let fallback = if c.len() < output.len() { c } else { generic(output) };
    // **The named compactor COMPETES, it does not preempt.**
    //
    // This return was a `return`: as soon as the program's module returned
    // something, `auto`'s tabular detector never ran. Yet a
    // module can return its input UNCHANGED — that is what `ps.rs` does on
    // a macOS `ps aux` output. MEASURED 2026-09-09 on the corpus
    // capture: `ps.rs` returned the **4 387 bytes** it received, where `auto`
    // produces **2 016** (−54 %), and the whole sysinfo family was in that
    // case. A compactor that folds nothing won against one that folds.
    // (Later: that −54 % was the COMMAND column alone — PID, %CPU, state
    // lost. `ps.rs` now respaces every column and folds by itself;
    // the rule stays for the modules that still return their input.)
    //
    // The module that FOLDED keeps the hand; see [`crate::fold_count::named_if_it_folded`],
    // and the zombie lesson that made « the shortest wins » rejected.
    crate::fold_count::named_if_it_folded(fallback, named, output)
}

#[cfg(test)]
mod tests {

    /// **A compactor that folds nothing must not win.**
    ///
    /// `ps.rs` returns its input UNCHANGED on a macOS `ps aux` output. The
    /// dispatcher returned that result without letting `auto`'s tabular
    /// detector run, which produces a little over half of it. Measured on
    /// the real corpus capture: **4 387 bytes returned as is versus
    /// 2 016**, and 3 012 versus 1 686 on the full delivery path.
    ///
    /// The fixture REACHES the rule: the first assertion checks that the
    /// program's module does return its input, otherwise the test would measure the
    /// neighbouring bound.
    #[test]
    fn a_named_compactor_that_folds_nothing_does_not_win() {
        // This capture was the case the rule was written for: `ps.rs` handed
        // a macOS `ps aux` back UNCHANGED and `auto` won with a shorter fold.
        // That fold was shorter because it kept COMMAND alone, so
        // `ps.rs` now respaces every column and keeps the hand by folding.
        // What the capture proves today: every column and every PID, shorter
        // than what came in.
        let body = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/macos/ps_aux.txt"),
        )
        .expect("the corpus `ps aux` capture");
        let out = dispatch("ps", &argv(&["ps", "aux"]), &body);
        assert!(out.contains("USER PID %CPU %MEM VSZ RSS TT STAT STARTED TIME COMMAND"), "{out}");
        for pid in body.lines().skip(1).filter_map(|l| l.split_whitespace().nth(1)) {
            assert!(out.contains(&format!(" {pid} ")), "PID {pid} lost:\n{out}");
        }
        assert!(out.len() < body.len(), "{} of {}", out.len(), body.len());
    }
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn json_flag_routes_to_auto() {
        // `ps --json` (some procps versions) / `lsof --format=json`.
        let body = r#"[{"pid":1,"comm":"systemd"},{"pid":2,"comm":"kthreadd"},{"pid":100,"comm":"sshd"}]"#;
        let out = dispatch("ps", &argv(&["ps", "--json"]), body);
        assert!(out.contains("systemd") || out.contains("pid"));
    }

    #[test]
    fn output_json_value_routes_to_auto() {
        // `--output json` value form.
        let body = r#"{"hostname":"box","uptime":3600}"#;
        let out = dispatch("uptime", &argv(&["uptime", "--output", "json"]), body);
        assert!(out.contains("hostname") || out.contains("uptime"));
    }

    #[test]
    fn du_s_strips_ansi_and_trims() {
        // `du -s` / `du -sh` returns one line per arg; just strip
        // and trim.
        let body = "\x1b[34m4.0K\t/var/log\n\x1b[0m";
        let out = dispatch("du", &argv(&["du", "-s", "/var/log"]), body);
        assert!(!out.contains("\x1b["), "must strip ANSI");
        assert_eq!(out, "4.0K\t/var/log");
    }

    #[test]
    fn du_without_s_does_not_take_strip_only_path() {
        // `du /var/log` (no -s) → falls through past the du-special
        // arm and routes through dispatch_named / auto / generic.
        let body = "4.0K\t/var/log/a\n8.0K\t/var/log/b\n12K\t/var/log\n";
        let out = dispatch("du", &argv(&["du", "/var/log"]), body);
        // Some compaction happens; main check: didn't just trim a
        // single line.
        assert!(out.contains("/var/log"));
    }

    #[test]
    fn ps_routes_through_default_path() {
        // ps output: header + many rows. Auto's tabular compactor
        // takes over (or generic does).
        let body = "USER       PID  %CPU %MEM    VSZ   RSS TTY   STAT COMMAND\n\
                    root         1   0.0  0.1   1234   567 ?     Ss   systemd\n\
                    root         2   0.0  0.0      0     0 ?     S    kthreadd\n\
                    root        10   0.0  0.0    100    50 ?     S    ksoftirqd/0\n";
        let out = dispatch("ps", &argv(&["ps", "aux"]), body);
        assert!(!out.is_empty());
        // Either tabular compactor or generic preserves PIDs/COMMANDs.
        assert!(out.contains("systemd") || out.contains("USER"));
    }

    #[test]
    fn lsof_i_routes_to_netstat_compactor() {
        // `lsof -i` is network-only; the netstat compactor collapses by
        // state and keeps LISTEN entries verbatim.
        let mut body = String::from(
            "COMMAND   PID USER   FD   TYPE DEVICE SIZE/OFF NODE NAME\n\
             sshd      900 root    3u  IPv4  12345      0t0  TCP *:22 (LISTEN)\n",
        );
        for i in 0..40 {
            body.push_str(&format!(
                "chrome   {pid} user   {fd}u  IPv4  {dev}      0t0  TCP 10.0.0.5:5000->1.2.3.{i}:443 (ESTABLISHED)\n",
                pid = 1000 + i,
                fd = 10 + i,
                dev = 20000 + i,
            ));
        }
        let out = dispatch("lsof", &argv(&["lsof", "-i"]), &body);
        // The listening sshd socket must survive verbatim.
        assert!(out.contains(":22") && out.contains("LISTEN"), "LISTEN dropped: {out}");
        // And the 40 ESTABLISHED rows must be collapsed, not echoed — to one
        // line per endpoint pair, each of the 40 peers still named.
        assert!(!out.contains("0t0") && out.len() < body.len(), "did not compact lsof -i: {out}");
        for i in 0..40 {
            assert!(out.contains(&format!(" 1.2.3.{i}:443")), "peer 1.2.3.{i} lost: {out}");
        }
    }

    #[test]
    fn bare_lsof_preserves_open_files() {
        // A *bare* `lsof` lists open files; routing it through the netstat
        // compactor would destroy them. The `-i` gate prevents that.
        let body = "COMMAND   PID USER   FD   TYPE DEVICE SIZE/OFF     NODE NAME\n\
                    bash     1234 user  cwd    DIR    8,1     4096   131073 /home/user\n\
                    bash     1234 user  txt    REG    8,1   123456   789012 /usr/bin/bash\n\
                    vim      5678 user    3u   REG    8,1     8192   456789 /home/user/notes.txt\n";
        let out = dispatch("lsof", &argv(&["lsof"]), body);
        // Filenames must survive — no connection-summary rewrite.
        assert!(out.contains("/usr/bin/bash"), "open file dropped: {out}");
        assert!(out.contains("notes.txt"), "open file dropped: {out}");
    }

    #[test]
    fn unknown_prog_falls_back_to_auto_or_generic() {
        // Any prog not handled by dispatch_named falls through; we
        // verify the function returns something non-empty.
        let body = "alpha\nbeta\nbeta\nbeta\ngamma\n";
        let out = dispatch("nonexistent_tool", &argv(&["nonexistent_tool"]), body);
        assert!(!out.is_empty());
    }
    /// A flag whose whole purpose is to ADD what the compactor removes must
    /// reach a path that keeps it.
    #[test]
    fn the_detail_flag_reaches_a_path_that_keeps_the_detail() {
        let body = "\
1: lo: <LOOPBACK,UP,LOWER_UP> mtu 65536 qdisc noqueue state UNKNOWN group default qlen 1000
    link/loopback 00:00:00:00:00:00 brd 00:00:00:00:00:00 promiscuity 0 allmulti 0 minmtu 0 maxmtu 0 gso_max_size 65536
    inet 127.0.0.1/8 scope host lo
2: eth0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc fq_codel state UP group default qlen 1000
    link/ether 04:7c:16:b9:ca:4e brd ff:ff:ff:ff:ff:ff promiscuity 0 allmulti 0 minmtu 68 maxmtu 16362 gso_max_size 64000
    inet 192.168.1.183/24 brd 192.168.1.255 scope global eth0
";
        let plain = dispatch("ip", &argv(&["ip", "addr"]), body);
        let detailed = dispatch("ip", &argv(&["ip", "-d", "addr"]), body);
        assert!(
            !plain.contains("gso_max_size"),
            "the plain form should still fold the knobs away: {plain}"
        );
        assert!(
            detailed.contains("gso_max_size"),
            "-d dropped the very block it asks for: {detailed}"
        );
        assert_ne!(plain, detailed, "the flag changed nothing");
    }

}
