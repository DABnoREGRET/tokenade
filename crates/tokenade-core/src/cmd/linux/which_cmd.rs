//! `which` / `which -a` — resolve a command name to its location(s) on
//! PATH. The output is one absolute path per line; with `-a` it lists
//! *every* match across PATH, so resolving many builtins at once (or a
//! PATH with duplicated entries) yields a long path list. Otherwise it is
//! one or two lines.
//!
//! The entries ARE filesystem paths, so the command-specific shape here is
//! a path listing: we fold a long run into a count while keeping the
//! bounds, reusing the shared path-listing engine (the same one `find`
//! uses). Detection is structural — absolute-path lines — so it never
//! keys off translated text.

const HEAD: usize = 40;

pub fn detect(text: &str) -> bool {
    text.lines().any(|l| {
        let t = l.trim();
        !t.is_empty() && t.starts_with('/')
    })
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    super::git_listing::fold_paths(text, HEAD, "which", "path(s)")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detector_rejects_empty_and_non_path() {
        assert!(!detect(""));
        assert!(!detect("no foo in (/usr/bin /bin)\n"));
    }

    #[test]
    fn detector_accepts_path() {
        assert!(detect("/usr/bin/ls\n"));
    }

    #[test]
    fn short_resolution_passes_through() {
        // The common case: one or two paths — nothing to fold.
        assert!(try_compact("/usr/bin/ls\n").is_none());
    }

    #[test]
    fn long_path_list_folds() {
        let s: String = (0..HEAD + 20).map(|i| format!("/opt/dup{i}/bin/tool\n")).collect();
        let out = try_compact(&s).unwrap();
        assert!(out.contains("/opt/dup0/bin/tool"), "first bound kept");
        assert!(out.len() < s.len(), "must shrink");
    }
}
