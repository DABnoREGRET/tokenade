//! `lsusb` — USB bus devices.


pub fn detect(text: &str) -> bool {
    let mut hits = 0usize;
    for line in text.lines().take(30) {
        if line.starts_with("Bus ") && line.contains("Device") && line.contains("ID") {
            hits += 1;
        }
    }
    hits >= 2
}

crate::archetypes::detect_gated_try_compact!();

pub fn compact(text: &str) -> String {
    // lsusb output is typically short; passthrough.
    text.trim().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_lsusb() {
        let s = "Bus 001 Device 002: ID 8087:8000 Intel Corp. \nBus 001 Device 001: ID 1d6b:0002 Linux Foundation 2.0 root hub\n";
        assert!(detect(s));
    }
}
