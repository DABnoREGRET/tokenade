//! `getent` — NSS database record dumps (`passwd`, `group`, `hosts`,
//! `services`, `protocols`, `aliases`, `networks`).
//!
//! `getent <db>` prints one record per line in a fixed, locale-INVARIANT
//! machine format: either colon-delimited (`name:x:uid:gid:gecos:home:shell`
//! for `passwd`, `name:x:gid:members` for `group`, `alias: targets` for
//! `aliases`) or whitespace-delimited (`addr canonical aliases…` for `hosts`,
//! `name port/proto aliases` for `services`, `name number aliases` for
//! `protocols`/`networks`). On an LDAP/NIS-backed host a single query can dump
//! thousands of records; that bulk is the cost.
//!
//! This compactor folds the MIDDLE of the record stream, keeping the first and
//! last `KEEP_EDGE` rows verbatim — so the key column (usernames, group names,
//! hostnames) and every GECOS byte, including non-ASCII, survive exactly — and
//! replaces the interior with an announced counter. Nothing is dropped
//! silently and the banner carries the real total record count.
//!
//! Detection is purely STRUCTURAL: a dominant run of same-delimiter records.
//! It never matches on an English label (getent output has none), so a
//! `LANG=de_DE`/`fr_FR` dump with non-ASCII names folds identically. It
//! declines (`None`) on the common small query (`getent passwd root`) so the
//! caller keeps that handful of lines verbatim.

const KEEP_EDGE: usize = 5;
const MIN_ROWS: usize = 20;

/// Structural detector. `true` when the text is a dominant run of same-shaped
/// delimited records — never keyed on any label, so localized data matches
/// identically.
pub fn detect(text: &str) -> bool {
    let mut nonblank = 0usize;
    let mut colon = 0usize;
    let mut space = 0usize;
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        nonblank += 1;
        // Colon family (passwd/group/aliases): a non-empty key precedes a `:`.
        if let Some((head, _)) = t.split_once(':') {
            if !head.trim().is_empty() {
                colon += 1;
                continue;
            }
        }
        // Whitespace family (hosts/services/protocols/networks): >= 2 fields.
        if t.split_whitespace().nth(1).is_some() {
            space += 1;
        }
    }
    let record = colon + space;
    if record < MIN_ROWS {
        return false;
    }
    // >= 70% of non-blank lines are delimited records...
    if record * 10 < nonblank * 7 {
        return false;
    }
    // ...and ONE delimiter family dominates (getent emits a single db shape),
    // which keeps prose carrying incidental colons/spaces from matching.
    let dominant = colon.max(space);
    dominant * 10 >= record * 7
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    crate::archetypes::fold_middle(text, KEEP_EDGE, "getent", "record(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use std::fmt::Write;
    use super::*;

    /// 52 accounts, 3 KB: the whole fits, so none is folded into a count.
    #[test]
    fn a_passwd_dump_that_fits_is_not_folded() {
        let t: String = (0..52)
            .map(|i| format!("svc{i}:x:{}:{}:Service {i}:/var/lib/svc{i}:/usr/bin/nologin\n", 900 + i, 900 + i))
            .collect();
        assert!(detect(&t));
        assert_eq!(try_compact(&t), None);
    }

    fn passwd_dump(n: usize) -> String {
        let mut s = String::new();
        for i in 0..n {
            let _ = writeln!(
                s,
                "user{i}:x:{uid}:{gid}:User Number {i}:/home/user{i}:/bin/bash",
                uid = 1000 + i,
                gid = 1000 + i
            );
        }
        s
    }

    #[test]
    fn detector_structural_not_keyword() {
        // 30 colon records → detect; short prose with incidental colons → no.
        assert!(detect(&passwd_dump(30)));
        let prose = "Note: the following is a description.\n\
                     It explains: things happen here.\n\
                     The end: finally.\n";
        assert!(!detect(prose), "short prose must not look like a getent dump");
    }

    #[test]
    fn small_query_declines() {
        // `getent passwd root` — a couple of lines must pass through verbatim.
        assert!(try_compact("root:x:0:0:root:/root:/bin/bash\n").is_none());
        assert!(try_compact(&passwd_dump(3)).is_none());
    }

    #[test]
    fn gain_on_large_dump() {
        let body = passwd_dump(200);
        let (out, sav) = compact_with_savings(&body);
        assert!(out.contains("[tokenade:getent]"));
        assert!(
            sav.ratio() >= 0.50,
            "expected >=50% savings on a 200-record dump, got {:.2} ({}→{})",
            sav.ratio(),
            sav.tokens_before,
            sav.tokens_after
        );
    }

    #[test]
    fn quality_real_count_edges_verbatim_fold_announced() {
        let body = passwd_dump(200);
        let out = try_compact(&body).expect("200 records must fold");
        // Real total in the banner.
        assert!(
            out.contains("200 record(s)"),
            "banner must carry real total: {}",
            out.lines().next().unwrap()
        );
        // First/last KEEP_EDGE keys preserved verbatim (named entities).
        assert!(out.contains("user0:x:1000:1000:User Number 0:"), "first record dropped");
        assert!(
            out.contains("user199:x:1199:1199:User Number 199:"),
            "last record dropped"
        );
        // Fold announced with a counter; folded = 200 - 2*KEEP_EDGE.
        let folded = 200 - KEEP_EDGE * 2;
        assert!(out.contains(&format!("folded {folded}")), "fold not announced: {out}");
        // A middle record is folded away (not leaked verbatim).
        assert!(!out.contains("user100:"), "middle record leaked verbatim");
        // Accounting closes: kept edges + folded == total.
        assert_eq!(KEEP_EDGE * 2 + folded, 200);
    }

    #[test]
    fn localized_non_ascii_records_fold_identically() {
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        // A de_DE/fr_FR-style passwd+GECOS dump: no English anywhere, non-ASCII
        // names and umlauts in the GECOS field. Detection (structural) and the
        // fold must behave exactly as for the C-locale dump, and the non-ASCII
        // edge bytes must survive verbatim.
        let mut body = String::new();
        body.push_str("jmüller:x:1000:1000:Jürgen Müller,Büro München:/home/jmüller:/bin/bash\n");
        body.push_str("fgüneş:x:1001:1001:Fatma Güneş,Kanton Zürich:/home/fgüneş:/bin/zsh\n");
        for i in 2..40 {
            let _ = writeln!(
                body,
                "konto{i}:x:{u}:{u}:Benutzer Nummer {i},Außenstelle:/home/konto{i}:/bin/bash",
                u = 1000 + i
            );
        }
        body.push_str("zägrößte:x:1099:1099:Zoé Élodie,Forêt Noire:/home/zägrößte:/bin/bash\n");

        assert!(detect(&body), "structural detection must not need English");
        let out = try_compact(&body).expect("localized dump must fold");
        // Non-ASCII first/last records preserved byte-for-byte.
        assert!(
            out.contains("jmüller:x:1000:1000:Jürgen Müller,Büro München:"),
            "leading non-ASCII record mangled: {out}"
        );
        assert!(
            out.contains("zägrößte:x:1099:1099:Zoé Élodie,Forêt Noire:"),
            "trailing non-ASCII record mangled: {out}"
        );
        // Real count + announced fold present.
        let total = body.lines().count();
        assert!(
            out.contains(&format!("{total} record(s)")),
            "real total missing for localized input"
        );
        assert!(out.contains("folded "), "fold counter missing for localized input");
    }

    #[test]
    fn whitespace_family_hosts_fold() {
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        // `getent hosts` (IPv4 + canonical + aliases) is whitespace-delimited —
        // detection must cover the non-colon family too.
        let mut hosts = String::new();
        for i in 0..40 {
            let _ = writeln!(hosts, "192.168.1.{i} host{i}.example.com host{i}");
        }
        assert!(detect(&hosts), "whitespace hosts family must detect");
        let out = try_compact(&hosts).expect("40 hosts must fold");
        assert!(out.contains("192.168.1.0 host0.example.com"), "first host dropped");
        assert!(out.contains("192.168.1.39 host39.example.com"), "last host dropped");
        assert!(out.contains("[tokenade:getent]"));
    }

    #[test]
    fn preserves_trailing_newline_state() {
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        let with = passwd_dump(30);
        assert!(try_compact(&with).unwrap().ends_with('\n'));
        let without = with.trim_end_matches('\n').to_string();
        let out = try_compact(&without).expect("must still fold");
        assert!(!out.ends_with('\n'), "must mirror missing trailing newline");
    }

    #[test]
    fn returns_something_via_compact() {
        let _ = compact(&passwd_dump(30));
        let _ = compact("root:x:0:0:root:/root:/bin/bash\n");
    }
}
