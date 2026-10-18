//! `pacman -Sl [repo]` — `repo name version [installed]`, one per package.
//!
//! A real `pacman -Sl core` (299 packages, fr_FR, 2026-10-04) came back
//! SAMPLED — one line in three, `[2 lines elided]` between them — so most
//! packages of the listing asked for were missing. Here the repository is
//! said once; every package keeps its version while that fits, and past the
//! delivery limit every NAME is still given (installed ones starred), the
//! versions left to the stash.

use std::fmt::Write;

pub fn try_compact(output: &str) -> Option<String> {
    // repo → [(name, version, installed marker)]
    let mut repos: Vec<(&str, Vec<(&str, &str, Option<&str>)>)> = Vec::new();
    for line in output.lines().filter(|l| !l.trim().is_empty()) {
        let mut t = line.split_whitespace();
        let (Some(repo), Some(name), Some(ver)) = (t.next(), t.next(), t.next()) else {
            return None;
        };
        let rest: Vec<&str> = t.collect();
        let marker = match rest.as_slice() {
            [] => None,
            [m] if m.starts_with('[') && m.ends_with(']') => Some(*m),
            _ => return None,
        };
        if !ver.chars().any(|c| c.is_ascii_digit()) {
            return None;
        }
        if repos.last().is_none_or(|(r, _)| *r != repo) {
            repos.push((repo, Vec::new()));
        }
        repos.last_mut()?.1.push((name, ver, marker));
    }
    let total: usize = repos.iter().map(|(_, p)| p.len()).sum();
    if total < 20 {
        return None;
    }
    let mut whole = String::new();
    for (repo, pkgs) in &repos {
        let _ = writeln!(whole, "[{repo}] {} package(s)", pkgs.len());
        for (n, v, m) in pkgs {
            let _ = writeln!(whole, "{n} {v}{}", m.map_or(String::new(), |m| format!(" {m}")));
        }
    }
    if tk_base::delivery::fits(&whole) {
        return Some(whole.trim_end().to_string());
    }
    let mut out = String::new();
    for (repo, pkgs) in &repos {
        let marker = pkgs.iter().find_map(|(_, _, m)| *m);
        let star = marker.map_or(String::new(), |m| format!(", * = {m}"));
        let names: Vec<String> = pkgs
            .iter()
            .map(|(n, _, m)| if m.is_some() { format!("{n}*") } else { n.to_string() })
            .collect();
        let _ = writeln!(out, "[{repo}] {} package(s){star} — versions in the full output:", pkgs.len());
        let _ = writeln!(out, "{}", names.join(" "));
    }
    Some(out.trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(n: usize) -> String {
        (0..n)
            .map(|i| format!("core pkg{i} 1.{i}-1{}\n", if i % 2 == 0 { " [installé]" } else { "" }))
            .collect()
    }

    #[test]
    fn a_repository_listing_that_fits_keeps_every_version() {
        let _g = tk_base::delivery::threshold_on_this_thread(100_000);
        let out = try_compact(&listing(30)).unwrap();
        assert!(out.starts_with("[core] 30 package(s)\npkg0 1.0-1 [installé]\npkg1 1.1-1\n"), "{out}");
        assert!(!out.contains("core pkg"), "{out}");
    }

    /// Past the limit: every name, never a sample.
    #[test]
    fn past_the_limit_every_name_is_given() {
        let _g = tk_base::delivery::threshold_on_this_thread(50);
        let out = try_compact(&listing(300)).unwrap();
        assert!(out.contains("[core] 300 package(s), * = [installé]"), "{out}");
        assert!(out.contains("pkg0* pkg1 pkg2*"), "{out}");
        assert!(out.contains(" pkg299"), "{out}");
    }

    #[test]
    fn other_output_is_left_alone() {
        assert!(try_compact("error: database 'nope' not found\n").is_none());
    }
}
