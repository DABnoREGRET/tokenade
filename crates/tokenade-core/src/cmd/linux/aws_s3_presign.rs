//! `aws_s3_presign` — aws s3 presign (returns a single URL).

pub fn detect(text: &str) -> bool {
    let t = text.trim();
    !t.is_empty() && (t.starts_with("https://") || t.starts_with("http://"))
}

pub fn try_compact(_text: &str) -> Option<String> {
    None // URLs are already short — passthrough is correct
}

pub fn compact(text: &str) -> String {
    crate::archetypes::passthrough(text)
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn detector_rejects_empty() { assert!(!detect("")); }
    #[test] fn detector_rejects_non_url() { assert!(!detect("plain text\n")); }
    #[test] fn detector_accepts_https_url() {
        assert!(detect("https://bucket.s3.amazonaws.com/key?X-Amz-Signature=abc\n"));
    }
    #[test] fn compact_returns_url_unchanged() {
        let url = "https://bucket.s3.amazonaws.com/key?X-Amz-Signature=abc\n";
        assert_eq!(compact(url), crate::archetypes::passthrough(url));
    }
}
