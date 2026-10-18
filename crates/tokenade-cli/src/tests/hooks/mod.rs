//! Tests for `hooks`, split out of the flat `tests.rs`.

use super::*;

// One file per hook, split out of a 2995-line `hooks.rs`.
mod bash_pretool;
mod dispatch;
mod grep_pretool;
mod misc;
mod read_pretool;
mod read_staging;
mod session_lifecycle;
mod webfetch_pretool;
mod websearch_post;

fn om_sym(kind: &str, name: &str, file: &str, line: usize) -> tokenade_core::index::Symbol {
    tokenade_core::index::Symbol {
        name: name.into(),
        kind: kind.into(),
        file: file.into(),
        line,
        decorators: vec![],
        bases: vec![],
        owner: String::new(),
        structural_hash: String::new(),
        self_hash: String::new(),
        end_line: 0,
    }
}

fn om_hit(file: &str, score: f64) -> tokenade_core::semantic::Hit {
    tokenade_core::semantic::Hit {
        file: file.into(),
        score,
        matched: vec!["alpha".into(), "beta".into()],
    }
}
