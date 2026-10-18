//! macOS `system_profiler SPHardwareDataType` / `SPSoftwareDataType`.
//!
//! ```text
//! Hardware:
//!
//!     Hardware Overview:
//!
//!       Model Name: Mac mini
//!       Model Identifier: Macmini9,1
//!       Model Number: MGNR3FN/A
//!       Chip: Apple M1
//!       Total Number of Cores: 8 (4 performance and 4 efficiency)
//!       Memory: 16 GB
//!       System Firmware Version: 10151.121.1
//!       OS Loader Version: 10151.121.1
//!       Serial Number (system): ABC123XYZ
//!       Hardware UUID: 12345678-1234-1234-1234-123456789ABC
//!       Provisioning UDID: 00000000-0000-0000-0000-000000000000
//!       Activation Lock Status: Disabled
//! ```
//!
//! Compaction: drop noise keys (UUIDs / serial numbers for privacy
//! and noise), keep substance (model, chip, cores, memory, OS).


const DROP_KEYS: &[&str] = &[
    "Hardware UUID",
    "Provisioning UDID",
    "Serial Number",
    "System Firmware Version",
    "OS Loader Version",
    "Apple ROM Info",
];

pub fn detect(text: &str) -> bool {
    text.contains("Hardware Overview") || text.contains("Memory:") || text.contains(':')
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let t = line.trim_start();
        if let Some((key, _)) = t.split_once(':') {
            if DROP_KEYS.iter().any(|k| key.starts_with(k)) {
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_hardware_overview() {
        let s = "Hardware:\n\n    Hardware Overview:\n\n      Model Name: Mac mini\n";
        assert!(detect(s));
    }

    #[test]
    fn drops_serial_and_uuid() {
        let s = "Hardware Overview:\n  Model Name: Mac mini\n  Hardware UUID: 12345678-ABCD-1234-1234-ABCDEFABCDEF\n  Serial Number (system): ABC123XYZ\n  Chip: Apple M1\n";
        let out = compact(s);
        assert!(out.contains("Mac mini"));
        assert!(out.contains("Apple M1"));
        assert!(!out.contains("Hardware UUID"));
        assert!(!out.contains("Serial Number"));
    }
}
