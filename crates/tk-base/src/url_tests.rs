// The tests of `url`.
//
// Spliced in by `include!` from `url.rs`. They came from `net_tests.rs` with their
// subject: none of them opens a connection, they read strings.
//
// A `//!` is impossible here: a file spliced in by `include!` is not a
// module, and an INNER doc comment has nothing to qualify.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_section_is_kept_and_a_document_id_is_not() {
        assert_eq!(
            domain_and_section("https://stackoverflow.com/questions/1234567/how-do-i"),
            "stackoverflow.com/questions"
        );
        assert_eq!(
            domain_and_section("https://acme.atlassian.net/wiki/spaces/ENG"),
            "atlassian.net/wiki"
        );
        // No section to name.
        assert_eq!(domain_and_section("https://example.com"), "example.com");
        assert_eq!(domain_and_section("https://example.com/"), "example.com");
        // A first segment that is only an identifier names the document, not
        // the shape — and is the part most able to single someone out.
        assert_eq!(
            domain_and_section("https://example.com/91827364"),
            "example.com"
        );
        assert_eq!(
            domain_and_section("https://example.com/9f8e7d6c5b4a3210"),
            "example.com"
        );
        assert_eq!(domain_and_section("not a url"), "unknown");
    }
    #[test]
    fn a_missing_suffix_can_only_ever_under_report() {
        // The privacy invariant the suffix list rests on, asserted instead of
        // merely claimed in prose: whatever the list does or does not contain,
        // the result is a SUFFIX of the input and never longer than 3 labels.
        // An entry that is absent therefore costs a dashboard bucket, never a
        // workspace slug on the wire.
        //
        // `zz` suffixes stand in for "a real public suffix nobody added yet".
        for host in [
            "acme-internal.corp.co.zz",
            "workspace-slug.notion.site",
            "customer1.eu-west.saas.example.co.zz",
            "a.b.c.d.e.example.com",
            "www.bbc.co.uk",
            "shop.example.com.au",
        ] {
            let got = registrable_domain(host);
            let lower = host.trim_end_matches('.').to_ascii_lowercase();
            assert!(
                lower.ends_with(&got),
                "{host} → {got} is not a suffix of the input"
            );
            assert!(
                got.split('.').count() <= 3,
                "{host} → {got} kept more than the registrable domain"
            );
            // The decisive half: the label that identifies WHO is gone.
            let leading = lower.split('.').next().unwrap();
            assert!(
                got != lower || lower.split('.').count() <= 2,
                "{host} → {got} kept the identifying label {leading}"
            );
        }
    }
    #[test]
    fn registrable_domain_drops_the_identifying_subdomain() {
        // The whole point: a workspace slug names the user's employer.
        assert_eq!(
            registrable_domain("acme-internal.atlassian.net"),
            "atlassian.net"
        );
        assert_eq!(registrable_domain("myteam.notion.site"), "notion.site");
        assert_eq!(registrable_domain("app.slack.com"), "slack.com");
        assert_eq!(registrable_domain("a.b.c.example.com"), "example.com");
        // Multi-part public suffixes keep three labels, not two.
        assert_eq!(registrable_domain("www.bbc.co.uk"), "bbc.co.uk");
        assert_eq!(registrable_domain("shop.example.com.au"), "example.com.au");
        // Already minimal / degenerate inputs pass through untouched.
        assert_eq!(registrable_domain("example.com"), "example.com");
        assert_eq!(registrable_domain("localhost"), "localhost");
        assert_eq!(registrable_domain("Example.COM."), "example.com");
        assert_eq!(registrable_domain("192.168.1.50"), "192.168.1.50");
        // A bare ccSLD is left alone rather than reduced to the suffix.
        assert_eq!(registrable_domain("bbc.co.uk"), "bbc.co.uk");
    }
    #[test]
    fn internal_hosts_are_recognised_for_the_ssrf_guard() {
        for h in [
            "localhost",
            "foo.localhost",
            "svc.internal",
            "printer.local",
            "127.0.0.1",
            "127.1.2.3",
            "0.0.0.0",
            "10.0.0.5",
            "192.168.1.50",
            "172.16.5.4",
            "169.254.169.254",
            "100.64.0.1",
            "::1",
            "::ffff:127.0.0.1",
            "fe80::1",
            "fc00::1",
            "fd12:3456::1",
        ] {
            assert!(is_internal_host(h), "must be internal: {h}");
        }
        for h in [
            "example.com",
            "93.184.216.34",
            "8.8.8.8",
            "172.32.0.1",
            "100.128.0.1",
            "2606:2800:220:1:248:1893:25c8:1946",
            "notion.so",
        ] {
            assert!(!is_internal_host(h), "must be PUBLIC: {h}");
        }
        // Full-URL form, including IPv6 brackets + port, and fail-safe on junk.
        assert!(is_internal_url("http://169.254.169.254/latest/meta-data/"));
        assert!(is_internal_url("http://[::1]:8080/admin"));
        assert!(is_internal_url("http://localhost:3000/"));
        assert!(!is_internal_url("https://example.com/page"));
        assert!(is_internal_url("not a url"), "unparseable → fail safe");
    }
}
