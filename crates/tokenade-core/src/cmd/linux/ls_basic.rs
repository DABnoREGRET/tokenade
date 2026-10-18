//! `ls_basic` — shared compactor for short `ls` listing variants.

const HEAD: usize = 100;

/// Shared engine used by `ls`, `ls -r`, `ls -t`, etc. Keeps `HEAD` entries
/// verbatim, then folds the tail into a per-extension count summary.
/// `label` is the name embedded in the banner (e.g. `"ls"`, `"ls -r"`).
pub fn fold_listing(text: &str, label: &str) -> String {
    let entries: Vec<&str> = text.lines()
        .map(|l| l.trim_end())
        .filter(|l| !l.is_empty())
        .collect();

    let total = entries.len();
    if total == 0 { return String::new(); }

    if total <= HEAD {
        return entries.join("\n");
    }

    let head = &entries[..HEAD];
    let tail = &entries[HEAD..];

    // Count tail entries by extension.
    let mut ext_counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for entry in tail {
        let ext = if let Some(pos) = entry.rfind('.') { &entry[pos..] } else { "(no ext)" };
        *ext_counts.entry(ext).or_insert(0) += 1;
    }
    let mut ext_list: Vec<(&str, usize)> = ext_counts.into_iter().collect();
    ext_list.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let summary: String = ext_list.iter()
        .map(|(ext, n)| format!("{ext} \u{d7}{n}"))
        .collect::<Vec<_>>()
        .join(", ");

    let fold_line = format!("\u{2026} (+{}) — {summary}", tail.len());
    format!("[tokenade:{label}] {total} entries\n{}\n{fold_line}", head.join("\n"))
}

pub fn detect(text: &str) -> bool {
    crate::archetypes::any_output(text)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) { return None; }
    let out = fold_listing(text, "ls");
    if out.len() < text.len() { Some(out) } else { None }
}

pub fn compact(text: &str) -> String {
    fold_listing(text, "ls")
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
        assert!(!detect("   \n"));
    }

    #[test]
    fn try_compact_some_or_none() {
        let s = "sample output line\nsecond line\n";
        let _ = try_compact(s);
        let _ = compact(s);
        let _ = compact_with_savings(s);
    }
}
