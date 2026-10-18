//! `docker inspect <container>` — JSON dump.
//!
//! Routes through auto::compact (JSON detection + schema_fold for
//! homogeneous arrays).

pub fn compact(output: &str) -> String {
    let (_, c, _) = crate::auto::compact(output);
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_json_through_compact() {
        let s = "[{\"Id\":\"abc\",\"State\":{\"Status\":\"running\"}}]";
        let out = compact(s);
        assert!(!out.is_empty());
    }
}
