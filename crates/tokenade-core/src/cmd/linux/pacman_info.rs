//! `pacman -Qi` / `-Si` — one `Label : value` block per package.
//!
//! What a block is read for is its version, size, dependencies and dates. A
//! real `pacman -Qi bash coreutils glibc systemd python` (fr_FR, 2026-10-04)
//! was 29 KB, 12 KB of it ONE line: the 1 000-odd packages `glibc` is
//! `Requis par`. Labels are localised, so nothing here reads them: a field
//! whose value is `--` is empty in every language, and a value of more than
//! [`LIST_CAP`] space-separated names is a reverse-dependency census.

use std::fmt::Write;

const LIST_CAP: usize = 40;

pub fn try_compact(output: &str) -> Option<String> {
    let whole = crate::archetypes::collapse_alignment_padding(output);
    let rows = output.lines().filter(|l| l.contains(" : ") || l.contains(": ")).count();
    if rows < 8 {
        return None;
    }
    if tk_base::delivery::fits(&whole) {
        return (whole.len() < output.len()).then_some(whole);
    }
    let mut out = String::new();
    let mut empty: Vec<String> = Vec::new();
    for line in whole.lines() {
        if line.trim().is_empty() {
            flush_empty(&mut out, &mut empty);
            out.push('\n');
            continue;
        }
        let Some((label, value)) = line.split_once(" : ").or_else(|| line.split_once(": ")) else {
            out.push_str(line);
            out.push('\n');
            continue;
        };
        if value.trim() == "--" {
            empty.push(label.trim().to_string());
            continue;
        }
        let names: Vec<&str> = value.split_whitespace().collect();
        if names.len() > LIST_CAP {
            let _ = writeln!(out, "{label} : {} … (+{} more, {} in all)", names[..LIST_CAP].join(" "), names.len() - LIST_CAP, names.len());
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    flush_empty(&mut out, &mut empty);
    Some(out.trim_end().to_string())
}

fn flush_empty(out: &mut String, empty: &mut Vec<String>) {
    if !empty.is_empty() {
        let _ = writeln!(out, "(empty: {})", empty.join(", "));
        empty.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(name: &str, required_by: usize) -> String {
        let req: Vec<String> = (0..required_by).map(|i| format!("pkg{i}")).collect();
        format!(
            "Nom                      : {name}\nVersion                  : 1.0-1\nDépend de                : glibc ncurses\nRequis par               : {}\nGroupes                  : --\nRemplace                 : --\nTaille installée         : 9,59 MiB\nInstallé le              : sam. 04 juil. 2026\n\n",
            req.join("  ")
        )
    }

    /// The census of reverse dependencies is cut with its count; the
    /// dependencies, version and empty fields stay readable.
    #[test]
    fn a_reverse_dependency_census_is_counted() {
        let _g = tk_base::delivery::threshold_on_this_thread(100);
        let t = format!("{}{}", block("glibc", 900), block("bash", 3));
        let out = try_compact(&t).unwrap();
        assert!(out.contains("Requis par : pkg0 pkg1 "), "{out}");
        assert!(out.contains("pkg39 … (+860 more, 900 in all)\n"), "{out}");
        assert!(!out.contains("pkg40 "), "{out}");
        assert!(out.contains("Requis par : pkg0 pkg1 pkg2\n"), "a short list stays whole:\n{out}");
        assert!(out.contains("Dépend de : glibc ncurses"), "{out}");
        assert_eq!(out.matches("(empty: Groupes, Remplace)").count(), 2, "{out}");
    }

    /// What fits is only respaced.
    #[test]
    fn a_block_that_fits_is_only_respaced() {
        let t = block("bash", 3);
        let out = try_compact(&t).unwrap();
        assert!(out.contains("Groupes : --"), "{out}");
    }
}
