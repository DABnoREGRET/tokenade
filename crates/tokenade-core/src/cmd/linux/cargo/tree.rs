//! `cargo tree` — ASCII dependency tree.

use crate::compress::whole_or_truncate_middle;
use crate::ansi::strip_ansi;

/// `cargo tree -d` asks one thing: which crates are built in more than one
/// version, and what pulls each in. Cut in the middle like any tree, a real
/// one (27 KB, 606 lines elided) named the first few duplicates and the last.
/// Read as that answer — every duplicated crate, each version with the crates
/// that depend on it directly — it is whole.
pub fn dispatch(argv: &[String], output: &str) -> String {
    if argv.iter().any(|a| a == "-d" || a == "--duplicates") {
        if let Some(d) = duplicates(&strip_ansi(output)) {
            return d;
        }
    }
    compact(output)
}

fn duplicates(s: &str) -> Option<String> {
    use std::collections::BTreeMap;
    let mut roots: Vec<String> = Vec::new();
    // The same version twice is one crate BUILT twice (other features, or
    // for the host and the target): said so, not hidden in a merge.
    let mut builds: BTreeMap<String, usize> = BTreeMap::new();
    let mut dependents: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut stack: Vec<String> = Vec::new();
    let mut kind: Vec<Option<String>> = Vec::new();
    for line in s.lines() {
        if line.trim().is_empty() {
            stack.clear();
            kind.clear();
            continue;
        }
        let prefix = line.chars().take_while(|c| matches!(c, '│' | '├' | '└' | '─' | ' ')).count();
        let depth = prefix / 4;
        let text: String = line.chars().skip(prefix).collect();
        if let Some(k) = text.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
            kind.truncate(depth);
            kind.resize(depth + 1, None);
            kind[depth] = Some(k.trim_end_matches("-dependencies").to_string());
            continue;
        }
        let mut toks = text.split_whitespace();
        let (Some(name), Some(ver)) = (toks.next(), toks.next()) else { return None };
        if !ver.starts_with('v') {
            return None;
        }
        let key = format!("{name} {ver}");
        if depth == 0 {
            *builds.entry(key.clone()).or_default() += 1;
            if !roots.contains(&key) {
                roots.push(key.clone());
            }
        } else {
            let parent = stack.get(depth - 1)?.clone();
            let tag = kind.get(depth - 1).cloned().flatten().map_or(String::new(), |k| format!(" ({k})"));
            let d = dependents.entry(parent).or_default();
            let entry = format!("{name}{tag}");
            if !d.contains(&entry) {
                d.push(entry);
            }
        }
        stack.truncate(depth);
        stack.push(key);
        kind.truncate(depth + 1);
    }
    let mut by_name: Vec<(&str, Vec<&String>)> = Vec::new();
    for r in &roots {
        let name = r.split(' ').next().unwrap_or(r);
        match by_name.iter_mut().find(|(n, _)| *n == name) {
            Some((_, v)) => v.push(r),
            None => by_name.push((name, vec![r])),
        }
    }
    if by_name.is_empty() || by_name.iter().all(|(_, v)| v.len() < 2 && v.iter().all(|k| builds[*k] < 2)) {
        return None;
    }
    let mut out = format!("{} crate(s) built in more than one version — each version ← what depends on it directly:\n", by_name.len());
    for (name, versions) in &by_name {
        let parts: Vec<String> = versions
            .iter()
            .map(|k| {
                let ver = match builds[*k] {
                    1 => k.split(' ').nth(1).unwrap_or("").to_string(),
                    n => format!("{} ({n} builds)", k.split(' ').nth(1).unwrap_or("")),
                };
                match dependents.get(*k) {
                    Some(d) => format!("{ver} ← {}", d.join(", ")),
                    None => ver.to_string(),
                }
            })
            .collect();
        out.push_str(&format!("{name}: {}\n", parts.join(" · ")));
    }
    Some(out.trim_end().to_string())
}

pub fn compact(output: &str) -> String {
    let s = strip_ansi(output);
    // A tree that fits is given whole: every line is a dependency (see
    // `whole_or_truncate_middle`).
    whole_or_truncate_middle(&s, 100, 15)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tree_that_fits_keeps_its_middle() {
        let mut s = String::from("tk-base v0.1.0\n");
        for i in 0..130 {
            s.push_str(&format!("├── dep{i} v1.{i}.0\n"));
        }
        let out = compact(&s);
        assert_eq!(out, s);
    }

    #[test]
    fn passes_short_tree() {
        let s = "foo v0.1.0\n├── bar v1.0\n└── baz v2.0\n";
        let out = compact(s);
        assert!(out.contains("foo"));
        assert!(out.contains("bar"));
    }

    /// `cargo tree -d`: every duplicated crate, each version with what pulls
    /// it in — dev/build edges marked — and a version built twice said so.
    #[test]
    fn duplicates_are_read_as_versions_and_their_dependents() {
        let mut s = String::new();
        for i in 0..40 {
            s.push_str(&format!("c{i} v0.1.0\n└── user{i}a v1.0.0\n    └── app v1.0.0\n\nc{i} v0.2.0\n├── user{i}b v1.0.0\n│   [dev-dependencies]\n│   └── app v1.0.0\n└── app v1.0.0 (*)\n\n"));
        }
        s.push_str("fastrand v2.4.1\n└── phf_generator v0.13.1\n\nfastrand v2.4.1\n└── tempfile v3.27.0\n");
        let argv: Vec<String> = ["cargo", "tree", "-d"].iter().map(|s| s.to_string()).collect();
        let out = dispatch(&argv, &s);
        assert!(out.starts_with("41 crate(s)"), "{out}");
        assert!(out.contains("\nc39: v0.1.0 ← user39a · v0.2.0 ← user39b, app\n"), "{out}");
        assert!(out.contains("fastrand: v2.4.1 (2 builds) ← phf_generator, tempfile"), "{out}");
        let plain = dispatch(&argv[..2], &s);
        assert!(!plain.contains("crate(s) built"), "only `-d` is read this way");
    }
}
