//! The asset table a Vite build prints (`vite build`, `slidev build`, an
//! `npm run build` that runs Vite).
//!
//! ```text
//! dist/index.html                          1.24 kB │ gzip:  0.57 kB
//! dist/assets/index-BThaGNfT.css         114.32 kB │ gzip: 21.98 kB
//! dist/assets/slidev/IconButton-….js       0.58 kB │ gzip:  0.39 kB
//! …
//! ✓ built in 993ms
//! ```
//!
//! One line per emitted file, hashed names, sorted by size: the caller reads
//! the verdict, the warnings, the entry pages and the few files that weigh —
//! not sixty `IconButton-BPNU_pds.js 0.58 kB`. Measured 2026-10-05 on a real
//! 25-slide `slidev build` (5 581 bytes, 60 asset lines): delivered at 4 323
//! bytes, the table untouched. Across the fleet `slidev` was 470 k uncovered
//! tokens over 75 runs.
//!
//! Kept verbatim: every line that is not an asset row (banners, `(!)`
//! warnings, errors, `✓ built in`), every `.html` entry, and the five largest
//! files. The rest become one line per directory and extension, with counts
//! and summed sizes.

use std::collections::BTreeMap;

/// `dist/assets/x-Ab12.js   12.34 kB │ gzip: 3.21 kB` → (path, size in kB, gzip).
fn asset_row(line: &str) -> Option<(&str, f64, Option<f64>)> {
    let mut parts = line.split_whitespace();
    let path = parts.next()?;
    if !path.contains('/') || path.starts_with('/') {
        return None;
    }
    let size: f64 = parts.next()?.parse().ok()?;
    let unit = parts.next()?;
    let kb = match unit {
        "B" => size / 1000.0,
        "kB" => size,
        "MB" => size * 1000.0,
        _ => return None,
    };
    let gzip = line.split_once("gzip:").and_then(|(_, g)| {
        let mut p = g.split_whitespace();
        let v: f64 = p.next()?.parse().ok()?;
        Some(match p.next()? {
            "B" => v / 1000.0,
            "MB" => v * 1000.0,
            _ => v,
        })
    });
    Some((path, kb, gzip))
}

pub fn detect(text: &str) -> bool {
    text.lines().filter(|l| asset_row(l).is_some()).count() >= 8
}

crate::archetypes::detect_gated_try_compact!();

fn size(kb: f64) -> String {
    if kb >= 1000.0 {
        format!("{:.2} MB", kb / 1000.0)
    } else {
        format!("{kb:.2} kB")
    }
}

pub fn compact(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let rows: Vec<(usize, &str, f64, Option<f64>)> =
        lines.iter().enumerate().filter_map(|(i, l)| asset_row(l).map(|(p, k, g)| (i, p, k, g))).collect();
    let mut by_size: Vec<&(usize, &str, f64, Option<f64>)> = rows.iter().collect();
    by_size.sort_by(|a, b| b.2.total_cmp(&a.2));
    let keep: Vec<usize> = by_size
        .iter()
        .take(5)
        .map(|r| r.0)
        .chain(rows.iter().filter(|r| r.1.ends_with(".html")).map(|r| r.0))
        .collect();
    // (directory, extension) → (files, kB, gzip kB)
    let mut groups: BTreeMap<(String, String), (usize, f64, f64)> = BTreeMap::new();
    for (i, path, kb, gz) in &rows {
        if keep.contains(i) {
            continue;
        }
        let (dir, file) = path.rsplit_once('/').unwrap_or(("", path));
        let ext = file.rsplit_once('.').map_or("", |(_, e)| e);
        let g = groups.entry((dir.to_string(), ext.to_string())).or_default();
        g.0 += 1;
        g.1 += kb;
        g.2 += gz.unwrap_or(0.0);
    }
    let first = rows.first().map_or(0, |r| r.0);
    let mut out: Vec<String> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let is_row = rows.iter().any(|r| r.0 == i);
        if i == first {
            for ((dir, ext), (n, kb, gz)) in &groups {
                let gz = if *gz > 0.0 { format!(" │ gzip: {}", size(*gz)) } else { String::new() };
                out.push(format!("{dir}/*.{ext}  {n} file(s)  {}{gz}", size(*kb)));
            }
            if !groups.is_empty() {
                out.push(format!("… the {} largest and the HTML entries, as printed:", keep.len().min(5)));
            }
        }
        if !is_row || keep.contains(&i) {
            out.push(line.to_string());
        }
    }
    out.join("\n")
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUILD: &str = include_str!("../../../tests/fixtures/linux/slidev_build.txt");

    #[test]
    fn the_table_folds_to_groups_and_the_files_that_weigh() {
        assert!(detect(BUILD));
        let out = compact(BUILD);
        // Not an asset row: kept as printed.
        for kept in ["✓ 551 modules transformed.", "✓ built in 993ms", "@slidev/theme-default"] {
            assert!(out.contains(kept), "{kept}: {out}");
        }
        // The entry page and the largest files, verbatim.
        for path in ["dist/index.html", "dist/assets/modules/shiki-C4eO27Uf.js", "dist/assets/index-BThaGNfT.css"] {
            let line = BUILD.lines().find(|l| l.starts_with(path)).expect(path);
            assert!(out.lines().any(|o| o == line), "{line}: {out}");
        }
        // A small file is counted, not listed.
        assert!(!out.contains("IconButton-BPNU_pds.js"), "{out}");
        assert!(out.contains("dist/assets/slidev/*.js"), "{out}");
        assert!(out.len() * 2 < BUILD.len(), "{} vs {}", out.len(), BUILD.len());
    }

    #[test]
    fn a_few_files_are_not_a_table() {
        assert!(!detect("dist/index.html 1.24 kB │ gzip: 0.57 kB\ndist/assets/a.js 3.10 kB\n✓ built in 1s\n"));
        assert_eq!(asset_row("src/main.rs 12 lines"), None);
    }
}
