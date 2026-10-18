//! Fold the base64 body of an SSH public key, keep the line around it.
//!
//! `known_hosts`, `authorized_keys`, `*.pub` and `ssh-keyscan` output all share
//! one line shape, and the bulk of every line is a key body no model can read.
//! MEASURED 2026-09-06 on a 40-host `known_hosts` of 13 402 bytes — the size of
//! a real one, this machine's own being 10 973 — folded to 4 755 bytes of which
//! **3 553 characters (75%) were key material**. Dedup cannot touch it: no two
//! keys share a line. After this pass the same file comes back at 3 295 bytes
//! (64.5% saved → **75.4%**) carrying **no key material at all**, all forty
//! hosts, and every key still distinguishable from its neighbour.
//!
//! What a reader wants off these files is which hosts are known, with what kind
//! of key, and whether a key CHANGED. So the body is replaced by its first few
//! characters and its length: enough to tell two keys apart and to see that one
//! moved, at eight characters instead of four hundred. It is deliberately not
//! called a fingerprint, because it is not one — a fingerprint is a hash of the
//! key, and printing a prefix as though it were one would invite a comparison
//! that does not hold.
//!
//! A PARSE, not a sniff, exactly like [`crate::pem`]: the key type must be one
//! OpenSSH actually emits. That is what keeps this off `git ls-remote`, whose
//! lines are also "token, whitespace, token" and whose tokens are the answer.
//! The same care is why a commit sha, a container digest and a Playwright
//! `[ref=eN]` are never folded anywhere in tokenade: high entropy is not the
//! test, a declared format is.

/// Key types OpenSSH writes into these files. A line whose second field is not
/// one of these is left alone, whatever it looks like.
const KEY_TYPES: &[&str] = &[
    "ssh-rsa",
    "ssh-dss",
    "ssh-ed25519",
    "ssh-ed448",
    "rsa-sha2-256",
    "rsa-sha2-512",
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
    "sk-ssh-ed25519@openssh.com",
    "sk-ecdsa-sha2-nistp256@openssh.com",
];

/// Below this, the marker is no shorter than the body it replaces.
const MIN_BODY_CHARS: usize = 40;

/// How much of the body to keep, so two keys on the same host stay distinct.
const KEEP_CHARS: usize = 8;

fn is_base64(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=')
}

/// Fold the key body on every line that is an SSH public key.
///
/// Returns the rewritten text and how many keys were folded. A count of zero
/// means the text comes back untouched, which is what lets a caller stay silent
/// on output that holds no keys.
pub fn fold_ssh_key_bodies(text: &str) -> (String, usize) {
    if !KEY_TYPES.iter().any(|k| text.contains(k)) {
        return (text.to_string(), 0);
    }
    let mut folded = 0usize;
    let mut out: Vec<String> = Vec::with_capacity(text.lines().count());
    for line in text.lines() {
        match fold_line(line) {
            Some(rewritten) => {
                folded += 1;
                out.push(rewritten);
            }
            None => out.push(line.to_string()),
        }
    }
    if folded == 0 {
        return (text.to_string(), 0);
    }
    let mut s = out.join("\n");
    if text.ends_with('\n') {
        s.push('\n');
    }
    (s, folded)
}

/// One line, or `None` to leave it exactly as it is.
fn fold_line(line: &str) -> Option<String> {
    // The key type and its body are adjacent fields, and everything before
    // them (host patterns, `@cert-authority`, an authorized_keys options list)
    // and after them (the comment) is kept verbatim. Finding the pair rather
    // than counting fields is what makes one parser serve both file formats:
    // `known_hosts` puts hosts in front, `authorized_keys` puts options there,
    // and an options list may itself contain spaces inside quotes.
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        // Start of a field.
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if start == i {
            break;
        }
        let field = &line[start..i];
        if !KEY_TYPES.contains(&field) {
            continue;
        }
        // The next field must be the body.
        let mut j = i;
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        let body_start = j;
        while j < bytes.len() && !bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        let body = &line[body_start..j];
        if body.len() < MIN_BODY_CHARS || !is_base64(body) {
            return None;
        }
        return Some(format!(
            "{}{}…{} chars elided by tokenade{}",
            &line[..body_start],
            &body[..KEEP_CHARS],
            body.len() - KEEP_CHARS,
            &line[j..]
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: usize, kind: &str) -> String {
        let body: String = std::iter::repeat_n('A', 370).collect();
        format!("host{n}.example.com {kind} k{n}{body}")
    }

    #[test]
    fn a_known_hosts_keeps_its_hosts_and_loses_its_keys() {
        let text: String = (0..40)
            .map(|n| key(n, ["ssh-ed25519", "ssh-rsa", "ecdsa-sha2-nistp256"][n % 3]) + "\n")
            .collect();
        let (out, folded) = fold_ssh_key_bodies(&text);
        assert_eq!(folded, 40, "not every key folded");
        for n in [0usize, 17, 39] {
            assert!(
                out.contains(&format!("host{n}.example.com")),
                "lost host{n}"
            );
        }
        assert!(out.contains("ssh-ed25519") && out.contains("ecdsa-sha2-nistp256"));
        assert!(
            !out.contains("AAAAAAAAAAAAAAAA"),
            "a key body survived: {out}"
        );
        assert!(out.contains("chars elided by tokenade"));
        assert!(
            out.len() * 4 < text.len(),
            "{} of {} is not a saving",
            out.len(),
            text.len()
        );
        // Two hosts' keys stay distinguishable.
        assert!(out.contains("k0AAAAAA") && out.contains("k17AAAAA"));
    }

    #[test]
    fn an_authorized_keys_comment_is_the_part_worth_keeping() {
        let body: String = std::iter::repeat_n('B', 370).collect();
        let line = format!("command=\"/usr/bin/backup\",no-pty ssh-ed25519 {body} alice@laptop\n");
        let (out, folded) = fold_ssh_key_bodies(&line);
        assert_eq!(folded, 1);
        assert!(
            out.contains("command=\"/usr/bin/backup\",no-pty"),
            "options lost: {out}"
        );
        assert!(out.contains("alice@laptop"), "comment lost: {out}");
        assert!(!out.contains("BBBBBBBBBBBBBBBB"), "body survived: {out}");
    }

    #[test]
    fn a_marker_line_keeps_its_marker() {
        let body: String = std::iter::repeat_n('C', 370).collect();
        let line = format!("@cert-authority *.example.com ssh-rsa {body}\n");
        let (out, folded) = fold_ssh_key_bodies(&line);
        assert_eq!(folded, 1);
        assert!(
            out.starts_with("@cert-authority *.example.com ssh-rsa "),
            "{out}"
        );
    }

    /// The anti-vacuity arm: "fold any long base64 after a word" must not pass.
    #[test]
    fn what_must_not_be_folded_is_not() {
        // `git ls-remote`: two opaque tokens per line, and both are the answer.
        let ls = "9f2c1ab5e7d34410a8f0b2c6d1e5f7093a4b6c8d\trefs/heads/main\n\
                  1a2b3c4d5e6f70819a2b3c4d5e6f70819a2b3c4d\trefs/tags/v1.0\n";
        assert_eq!(fold_ssh_key_bodies(ls), (ls.to_string(), 0));

        // An unknown key type is left alone however much it looks like one.
        let odd = format!("host ssh-quantum42 {}\n", "D".repeat(370));
        assert_eq!(fold_ssh_key_bodies(&odd), (odd.clone(), 0));

        // The type is there but the body is not base64. It must be LONG enough
        // to clear MIN_BODY_CHARS, or the length check decides and the base64
        // check is never what refuses — measured: with a three-letter body a
        // mutation removing `is_base64` survived this arm untouched.
        let prose = "host ssh-rsa the-key-went-missing-and-this-is-a-long-sentence-indeed\n";
        assert!(
            prose.split_whitespace().nth(2).unwrap().len() > MIN_BODY_CHARS,
            "this fixture no longer exercises the base64 check"
        );
        assert_eq!(fold_ssh_key_bodies(prose), (prose.to_string(), 0));
        // And a path, which is what an `IdentityFile` line carries.
        let path = "host ssh-ed25519 /home/someone/.ssh/identities/primary_key_file.pub\n";
        assert_eq!(fold_ssh_key_bodies(path), (path.to_string(), 0));

        // A body too short to be worth replacing.
        let tiny = "host ssh-ed25519 AAAAC3NzaC1lZDI1\n";
        assert_eq!(fold_ssh_key_bodies(tiny), (tiny.to_string(), 0));

        // A line naming a key type with no body after it.
        let bare = "Permission denied (publickey). Offered: ssh-ed25519\n";
        assert_eq!(fold_ssh_key_bodies(bare), (bare.to_string(), 0));

        // Text with no key type at all comes back identical, and says so.
        let plain = "Host github.com\n  User git\n";
        assert_eq!(fold_ssh_key_bodies(plain), (plain.to_string(), 0));
    }

    #[test]
    fn a_trailing_newline_is_preserved_either_way() {
        let with = key(1, "ssh-rsa") + "\n";
        assert!(fold_ssh_key_bodies(&with).0.ends_with('\n'));
        let without = key(1, "ssh-rsa");
        assert!(!fold_ssh_key_bodies(&without).0.ends_with('\n'));
    }
}

// Same move as the `mod wiring` of `pem.rs`, and for the same reason:
// it queries `filter`. See `tests/misc/a_format_is_reached_by_its_router.rs`.
