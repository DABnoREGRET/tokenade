//! **Two names in the same directory do not differ only by case.**
//!
//! macOS and Windows have case-insensitive file systems: on extraction on
//! the build machines, the second overwrites the first, and which one
//! survives depends on the ORDER of the archive. `cmd/linux/dpkg_l.rs` and
//! `dpkg_L.rs` lived that way: syncing from the live tree kept the right one
//! by chance, syncing through `git archive` (sorted) kept the other, and the
//! 1.1.22 release stopped on three VMs with one dispatch key missing
//! (2026-10-03). The macOS/Windows binary, for its part, had always been one
//! compactor short, without any test saying so.

use tk_base::source_guard::crates_dir;

#[test]
fn no_two_names_in_one_directory_differ_only_by_case() {
    fn walk(dir: &std::path::Path, clashes: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut seen: std::collections::BTreeMap<String, String> = Default::default();
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name == "target" || name.starts_with('.') {
                continue;
            }
            if let Some(other) = seen.insert(name.to_lowercase(), name.clone()) {
                clashes.push(format!("{} : {other} / {name}", dir.display()));
            }
            if e.path().is_dir() {
                walk(&e.path(), clashes);
            }
        }
    }
    let mut clashes = Vec::new();
    walk(&crates_dir(), &mut clashes);
    assert!(
        clashes.is_empty(),
        "names that differ only by case:\n{}",
        clashes.join("\n")
    );
}
