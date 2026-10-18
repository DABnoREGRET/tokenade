//! Comment stripping, per language, on the lines where the languages disagree.
//!
//! Five modules each carried a private copy of this scan. Two of them differed
//! from the rest in ways nobody had written down, and the block-comment family
//! next door hid a real defect for the same reason — HCL's copy had no idea
//! what a string was, so a `/*` inside a value ate the rest of the file.
//!
//! The copies are now one shared scanner with a per-language configuration.
//! What that configuration must be is not obvious, and getting it wrong loses
//! content silently, so it is written down here as tests rather than left in
//! four heads: a marker inside a value is not a comment, a real comment still
//! goes, and each language's own rule about quotes and escapes holds.
//! These drive `strip_only`, not `compact`. The shipped `compact` declares what
//! it removed and, on a fixture this small, that sentence costs more than the
//! strip saved — so it returns the file whole and there is no stripping left to
//! assert. The fixtures are one line each on purpose (see the paragraph above),
//! and the declaration's own cost is asserted once for every language in
//! `code_strip::declaration_cost`.

/// A `#` inside a double-quoted value is part of the value, in every language
/// of this family.
#[test]
fn a_marker_inside_a_double_quoted_value_is_kept() {
    let cases = [
        (
            "bazel",
            "genrule(\n    name = \"fragment#1\",\n    cmd = \"echo hi\",\n)\n",
            "fragment#1",
        ),
        (
            "cmake",
            "set(URL \"https://e.co/p#anchor\")\nproject(demo)\n",
            "p#anchor",
        ),
        (
            "toml",
            "[package]\nname = \"demo\"\nurl = \"https://e.co/p#anchor\"\n",
            "p#anchor",
        ),
        (
            "ini",
            "[server]\nurl = \"https://e.co/p#anchor\"\nport = 8080\n",
            "p#anchor",
        ),
    ];
    for (lang, src, needle) in cases {
        let out = match lang {
            "bazel" => tokenade_core::bazel::strip_only(src),
            "cmake" => tokenade_core::cmake::strip_only(src),
            "toml" => tokenade_core::toml_compact::strip_only(src),
            _ => tokenade_core::ini::strip_only(src),
        };
        assert!(out.contains(needle), "{lang}: lost the value:\n{out}");
    }
}

/// And a real comment is still removed — a stripper that keeps everything is
/// not a stripper.
#[test]
fn a_real_comment_is_still_removed() {
    let cases = [
        ("bazel", "genrule(\n    name = \"a\",  # internal only\n)\n"),
        ("cmake", "set(A 1)  # internal only\nproject(demo)\n"),
        ("toml", "[package]\nname = \"demo\"  # internal only\n"),
        ("ini", "[server]\nport = 8080  # internal only\n"),
    ];
    for (lang, src) in cases {
        let out = match lang {
            "bazel" => tokenade_core::bazel::strip_only(src),
            "cmake" => tokenade_core::cmake::strip_only(src),
            "toml" => tokenade_core::toml_compact::strip_only(src),
            _ => tokenade_core::ini::strip_only(src),
        };
        assert!(
            !out.contains("internal only"),
            "{lang}: comment survived:\n{out}"
        );
    }
}

/// TOML literal strings do NOT process escapes: `'C:\'` ends at that quote, so
/// treating the backslash as an escape swallows the terminator and everything
/// after it. This is the rule TOML has and the other three do not.
#[test]
fn a_toml_literal_string_does_not_escape() {
    let src = "[paths]\nwin = 'C:\\'\nnix = \"/tmp\"\nport = 8080\n";
    let out = tokenade_core::toml_compact::strip_only(src);
    assert!(
        out.contains("8080"),
        "the rest of the file was eaten:\n{out}"
    );
}

/// A backslash-escaped quote inside a basic string must not end it early.
#[test]
fn an_escaped_quote_does_not_end_a_basic_string() {
    let src = "[package]\ndesc = \"a \\\" b # not a comment\"\nport = 8080\n";
    let out = tokenade_core::toml_compact::strip_only(src);
    assert!(out.contains("not a comment"), "cut inside a string:\n{out}");
}

/// `;` in an INI-family file is a comment only when text follows it. `value;`
/// at end of line is an nginx statement terminator; cutting there loses the
/// value, and that is the shape of a real config file, not a corner case.
#[test]
fn an_ini_semicolon_is_a_terminator_unless_a_comment_follows() {
    let terminator = "[http]\nproxy_pass http://up;\nport = 8080\n";
    let out = tokenade_core::ini::strip_only(terminator);
    assert!(
        out.contains("http://up;") || out.contains("http://up"),
        "lost the nginx directive:\n{out}"
    );
    assert!(out.contains("8080"), "lost the rest of the file:\n{out}");

    let comment = "[http]\nport = 8080 ; internal only\n";
    let out = tokenade_core::ini::strip_only(comment);
    assert!(!out.contains("internal only"), "comment survived:\n{out}");
    assert!(out.contains("8080"), "cut too early:\n{out}");
}

/// CMake has no single-quoted strings — `'` is an ordinary character in an
/// argument. Treating it as a quote would leave the scanner "inside a string"
/// for the rest of the line and hide a real comment; the languages that DO have
/// them must keep a `#` inside one.
#[test]
fn single_quotes_are_a_string_only_where_the_language_says_so() {
    let cmake = "set(MSG don't)  # internal only\nproject(demo)\n";
    let out = tokenade_core::cmake::strip_only(cmake);
    assert!(
        !out.contains("internal only"),
        "cmake: comment survived:\n{out}"
    );

    let bazel = "genrule(\n    cmd = 'echo #1 done',\n)\n";
    let out = tokenade_core::bazel::strip_only(bazel);
    assert!(
        out.contains("#1 done"),
        "bazel: cut inside a single-quoted string:\n{out}"
    );
}

/// CMake escapes in UNQUOTED arguments too, so `a\#b` carries a literal `#`
/// and the line has no comment. Bazel's copy did the same and TOML's and INI's
/// did not — a difference nobody had written down, and the kind that a shared
/// implementation quietly erases if it is not preserved on purpose.
#[test]
fn cmake_escapes_outside_quotes_as_well() {
    let src = "set(FRAGMENT a\\#b)\nproject(demo)\n";
    let out = tokenade_core::cmake::strip_only(src);
    assert!(
        out.contains("a\\#b"),
        "an escaped marker was cut as a comment:\n{out}"
    );
}
