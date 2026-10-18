//! `pacman -Ql` / `-Qlq` / `-Fl` — the files a package owns.
//!
//! Read as an archive listing, a real `pacman -Ql bash` (269 entries, 9 KB)
//! came back as nine directory COUNTS and not one file name (2026-10-04):
//! the names are the answer. Here every file is named once, under its
//! directory; the directory entries themselves (`/usr/bin/`) go, being implied
//! by the files under them; and directories that differ by ONE segment and
//! hold the same names (`/usr/share/locale/<lang>/LC_MESSAGES/: bash.mo`, 62
//! times) are one line with that segment's values.

use std::fmt::Write;

pub fn try_compact(output: &str) -> Option<String> {
    // package → directory → file names, in source order.
    let mut pkgs: Vec<(String, Vec<(String, Vec<String>)>)> = Vec::new();
    let mut dirs_only = 0usize;
    for line in output.lines().filter(|l| !l.trim().is_empty()) {
        let (pkg, path) = match line.split_once(' ') {
            Some((p, rest)) if rest.starts_with('/') && !p.contains('/') => (p, rest),
            _ if line.starts_with('/') => ("", line),
            _ => return None,
        };
        if path.ends_with('/') {
            dirs_only += 1;
            continue;
        }
        let (dir, name) = path.rsplit_once('/')?;
        if pkgs.last().is_none_or(|(p, _)| p != pkg) {
            pkgs.push((pkg.to_string(), Vec::new()));
        }
        let groups = &mut pkgs.last_mut()?.1;
        let dir = format!("{dir}/");
        match groups.iter_mut().find(|(d, _)| *d == dir) {
            Some((_, names)) => names.push(name.to_string()),
            None => groups.push((dir, vec![name.to_string()])),
        }
    }
    let files: usize = pkgs.iter().flat_map(|(_, g)| g).map(|(_, n)| n.len()).sum();
    if files < 20 {
        return None;
    }
    let mut out = String::new();
    for (pkg, groups) in &pkgs {
        let n: usize = groups.iter().map(|(_, v)| v.len()).sum();
        if !pkg.is_empty() {
            let _ = writeln!(out, "{pkg}: {n} file(s)");
        }
        for line in merged(groups) {
            let _ = writeln!(out, "  {line}");
        }
    }
    if dirs_only > 0 {
        let _ = writeln!(out, "({dirs_only} directory entries not listed: each holds the files above)");
    }
    Some(out.trim_end().to_string())
}

/// Directories alike but for one segment, holding the same names, said once
/// with `*` and the segment's values.
fn merged(groups: &[(String, Vec<String>)]) -> Vec<String> {
    let mut taken = vec![false; groups.len()];
    let mut lines: Vec<(usize, String)> = Vec::new();
    for (i, (dir, names)) in groups.iter().enumerate() {
        if taken[i] {
            continue;
        }
        let segs: Vec<&str> = dir.split('/').collect();
        let mut best: Vec<(usize, String)> = Vec::new();
        for k in 0..segs.len() {
            let same: Vec<(usize, String)> = groups
                .iter()
                .enumerate()
                .filter(|(j, (d, n))| {
                    let s: Vec<&str> = d.split('/').collect();
                    !taken[*j]
                        && n == names
                        && s.len() == segs.len()
                        && s.iter().zip(&segs).enumerate().all(|(m, (a, b))| m == k || a == b)
                })
                .map(|(j, (d, _))| (j, d.split('/').nth(k).unwrap_or("").to_string()))
                .collect();
            if same.len() > best.len() {
                best = same;
            }
        }
        if best.len() >= 3 {
            let k = (0..segs.len())
                .find(|&k| best.iter().any(|(j, _)| groups[*j].0.split('/').nth(k) != Some(segs[k])))
                .unwrap_or(0);
            let mut pattern = segs.clone();
            pattern[k] = "*";
            let values: Vec<&str> = best.iter().map(|(_, v)| v.as_str()).collect();
            for (j, _) in &best {
                taken[*j] = true;
            }
            lines.push((i, format!("{} {} (× {}: * = {})", pattern.join("/"), names.join(" "), best.len(), values.join(" "))));
        } else {
            taken[i] = true;
            lines.push((i, format!("{dir} {}", names.join(" "))));
        }
    }
    lines.into_iter().map(|(_, l)| l).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real `pacman -Ql bash` shape: every file named, directory entries
    /// gone, the per-language catalogues one line.
    #[test]
    fn a_package_file_list_names_every_file() {
        let mut s = String::from("bash /etc/\nbash /etc/bash.bashrc\nbash /usr/\nbash /usr/bin/\nbash /usr/bin/bash\nbash /usr/bin/bashbug\nbash /usr/bin/sh\n");
        for lang in ["af", "bg", "ca", "cs", "da", "de", "el", "eo", "es", "fr"] {
            s.push_str(&format!("bash /usr/share/locale/{lang}/\nbash /usr/share/locale/{lang}/LC_MESSAGES/\nbash /usr/share/locale/{lang}/LC_MESSAGES/bash.mo\n"));
        }
        for i in 0..12 {
            s.push_str(&format!("bash /usr/include/bash/h{i}.h\n"));
        }
        let out = try_compact(&s).unwrap();
        assert!(out.starts_with("bash: 26 file(s)\n"), "{out}");
        assert!(out.contains("  /usr/bin/ bash bashbug sh\n"), "{out}");
        assert!(out.contains("  /usr/share/locale/*/LC_MESSAGES/ bash.mo (× 10: * = af bg ca cs da de el eo es fr)"), "{out}");
        assert!(out.contains("h11.h"), "{out}");
        assert!(out.contains("(23 directory entries not listed"), "{out}");
    }

    #[test]
    fn other_output_is_left_alone() {
        assert!(try_compact("error: package 'nope' was not found\n").is_none());
    }
}
