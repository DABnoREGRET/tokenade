//! `df -i` — inode usage.
//!
//! Delegates to the shared [`super::df_table`] engine. The critical column
//! here is `IUse%` (inode exhaustion — a real and easily-overlooked cause
//! of "no space left on device" with bytes free). It too is a `NN%` token,
//! so the same content-based detection and full-first surfacing apply, in
//! any locale.
const HEAD: usize = 30;

pub fn detect(text: &str) -> bool {
    super::df_table::detect(text)
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    super::df_table::fold(text, HEAD, "df-i")
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    fn make_table(n: usize) -> String {
        let mut t = String::from("Filesystem Inodes IUsed IFree IUse% Mounted on\n");
        for i in 0..n {
            let _ = writeln!(t, "/dev/loop{i} 6500000 12 6499988 1% /snap/x{i}");
        }
        t
    }

    #[test]
    fn detector_rejects_empty() {
        assert!(!detect(""));
    }

    #[test]
    fn detector_accepts_table() {
        assert!(detect(&make_table(3)));
    }

    #[test]
    fn surfaces_inode_exhaustion_first() {
        let mut t = make_table(35);
        t.push_str("/dev/sdb1 1000000 980000 20000 98% /var\n");
        let out = try_compact(&t).unwrap();
        let full = out.find("/var").unwrap();
        let fold = out.find("more) ...").unwrap();
        assert!(full < fold, "inode-full FS must precede fold:\n{out}");
    }

    #[test]
    fn short_table_passes_through() {
        assert!(try_compact(&make_table(3)).is_none());
    }
}
