//! macOS `diskutil list` — disk layout.
//!
//! ```text
//! /dev/disk0 (internal, physical):
//!    #:                       TYPE NAME                    SIZE       IDENTIFIER
//!    0:      GUID_partition_scheme                        *500.3 GB   disk0
//!    1:                        EFI EFI                     314.6 MB   disk0s1
//!    2:                 Apple_APFS Container disk1         500.0 GB   disk0s2
//! /dev/disk1 (synthesized):
//!    ...
//! ```
//!
//! Compaction: passthrough by default (output is already terse),
//! but routes the input via the dispatch so other parsers don't
//! misclassify the table.


pub fn detect(text: &str) -> bool {
    text.contains("/dev/disk") || text.contains("DiskIdentifier")
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    text.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_diskutil_list() {
        let s = "/dev/disk0 (internal, physical):\n   0:      GUID_partition_scheme                        *500.3 GB   disk0\n   2:                 Apple_APFS Container disk1         500.0 GB   disk0s2\n";
        assert!(detect(s));
    }
}
