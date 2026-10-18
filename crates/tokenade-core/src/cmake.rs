//! CMake (`CMakeLists.txt`, `*.cmake`) compactor.
//!
//! Covers the `CMakeLists.txt` build-system files common to most
//! C/C++ codebases plus `*.cmake` toolchain / module includes. Real
//! projects often have 200+ line CMakeLists with extensive header
//! comments, option-explanation banners, and platform-detection
//! ladders — typically 25-40% drop after this pass.
//!
//! What it strips:
//!   * full-line `#` comments (license headers, section banners,
//!     individual explanation lines)
//!   * inline `# …` comment tails
//!   * blank-line runs → one blank max
//!   * trailing whitespace
//!
//! What it preserves verbatim:
//!   * every CMake command call (`add_executable`, `target_*`,
//!     `find_package`, `set`, `option`, `add_subdirectory`, etc.)
//!   * control flow (`if`, `elseif`, `else`, `endif`, `foreach`,
//!     `endforeach`, `function`, `endfunction`, `macro`, `endmacro`)
//!   * `cmake_minimum_required`, `project`, `include`,
//!     `add_definitions`, `enable_*` directives
//!   * generator expressions (`$<…>`) — untouched on substring level
//!   * block-comment markers `#[[` / `]]` — IGNORED here; we don't
//!     try to strip them (correct but rare)

/// CMake commands that prove the input is genuinely a CMake file.
/// Case-insensitive match against the first token.
const CMAKE_COMMANDS: &[&str] = &[
    "cmake_minimum_required",
    "project",
    "add_executable",
    "add_library",
    "add_subdirectory",
    "add_definitions",
    "add_compile_options",
    "add_custom_command",
    "add_custom_target",
    "add_test",
    "target_sources",
    "target_link_libraries",
    "target_include_directories",
    "target_compile_options",
    "target_compile_definitions",
    "target_compile_features",
    "find_package",
    "find_library",
    "find_path",
    "find_program",
    "include",
    "include_directories",
    "link_directories",
    "link_libraries",
    "install",
    "set",
    "set_target_properties",
    "set_property",
    "set_source_files_properties",
    "set_tests_properties",
    "option",
    "configure_file",
    "file",
    "list",
    "string",
    "math",
    "message",
    "enable_language",
    "enable_testing",
    "ctest",
    "if",
    "elseif",
    "else",
    "endif",
    "foreach",
    "endforeach",
    "while",
    "endwhile",
    "function",
    "endfunction",
    "macro",
    "endmacro",
    "return",
    "break",
    "continue",
    "fetch_package",
    "add_bare_module",
];

/// Cheap heuristic detector. Among the first 80 non-blank, non-`#`
/// lines, ≥3 must start with a known CMake command AND ≥1 of those
/// must be a "distinctive" CMake-specific token (contains `_`, e.g.
/// `add_executable`, `target_link_libraries`, `find_package`,
/// `cmake_minimum_required`).
///
/// #589 — the bare 3-hit threshold was too loose: generic tokens
/// in CMAKE_COMMANDS (`if`/`else`/`endif`/`function`/`return`/`set`/
/// `include`/`file`/`list`/`string`/`message`/`option`/`install`) all
/// appear in PowerShell, bash, Python, and many other languages.
/// Production showed `cat install.ps1` (PowerShell) matching at 33%
/// efficiency because the PS script had `if`/`else`/`function`/
/// `return`/`set` — none of those are CMake-specific. Requiring at
/// least one distinctive token (`add_*`/`target_*`/`find_*`/etc.)
/// kills the false-positive class with no real cost — every actual
/// CMakeLists.txt has at least one such call.
pub fn detect(text: &str) -> bool {
    let mut hits = 0usize;
    let mut distinctive = 0usize;
    let mut considered = 0usize;
    for line in text.lines().filter(|l| !l.trim().is_empty()).take(80) {
        considered += 1;
        let t = line.trim_start();
        // Hard reject.
        if t.starts_with("<!DOCTYPE") || t.starts_with("<?xml") {
            return false;
        }
        if t.starts_with('#') {
            continue;
        }
        // First word, lowercased, stripped of `(`.
        let first: String = t
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != '(')
            .map(|c| c.to_ascii_lowercase())
            .collect();
        // A CMake command is a CALL: `project(MyApp CXX)`, or `project (…)`
        // with the space CMake tolerates. It is never `project = x` and never
        // `project repo repository codebase source` — the second is a word
        // list inside a Python string, and it is what made
        // `route_binary_pilot/predicate.py` the distinctive hit that carried
        // the whole file. Measured 2026-08-26: 121 source files were claimed
        // here, 64 of them had their `#` comments stripped under a `cmake`
        // banner.
        let is_a_call = t[first.len()..].trim_start().starts_with('(');
        if is_a_call && CMAKE_COMMANDS.contains(&first.as_str()) {
            hits += 1;
            // Distinctive: tokens that NEVER collide with other
            // scripting languages. Two sources:
            //   1. Multi-word CMake API (`add_executable`,
            //      `target_link_libraries`, `find_package`, …) —
            //      identified by containing `_`.
            //   2. CMake-unique non-`_` tokens (`project`, and the
            //      `end*` block-enders). PowerShell/bash/Python use
            //      `}`/`fi`/indent to end blocks, never `endif` /
            //      `endforeach` / `endfunction` / `endmacro` /
            //      `endwhile`. `project()` is a CMake-only directive.
            //      `elseif` only appears in CMake among the cmake-
            //      keyword overlap class.
            let cmake_unique_no_underscore = matches!(
                first.as_str(),
                "project"
                    | "endif"
                    | "endforeach"
                    | "endfunction"
                    | "endmacro"
                    | "endwhile"
                    | "elseif"
            );
            if first.contains('_') || cmake_unique_no_underscore {
                distinctive += 1;
            }
        }
    }
    hits >= 3 && distinctive >= 1 && considered > 0
}

/// Strip `#` comments + blank-run collapse + trailing-ws trim.
pub fn compact(text: &str) -> String {
    crate::code_strip::declare_strip(text, strip_only(text), "cmake", None)
}

/// The strip WITHOUT its declaration.
///
/// **Not an output path.** Everything a user or agent sees goes through
/// `compact`, which declares what this removed and returns the file whole when
/// the note would cost more than the strip saved. This is public only so the
/// per-language mechanics can be asserted directly: the lesson is that a
/// guard aimed at a wrapper which short-circuits on short input passes against
/// broken code underneath, and these fixtures are one line each on purpose.
pub fn strip_only(text: &str) -> String {
    crate::code_strip::strip_line_language(text, |t| t.starts_with('#'), strip_inline_comment)
}

/// `Savings`-yielding variant for the auto-compact ledger.
pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

/// CMake: `#` comments and only double-quoted strings — `'` is an ordinary
/// character in a CMake argument, so treating it as a quote would swallow the
/// rest of the line.
fn strip_inline_comment(line: &str) -> String {
    crate::code_strip::strip_inline_comment_line(line, b"#", b"\"", b"\"", true, |_, _| true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL_CMAKE: &str = "# Top-level CMakeLists.txt for foo\n# Copyright 2026 Example Corp.\n\ncmake_minimum_required(VERSION 3.20)\nproject(foo CXX)\n\n# Default to Release if not specified\nif(NOT CMAKE_BUILD_TYPE)\n  set(CMAKE_BUILD_TYPE Release)\nendif()\n\nfind_package(Threads REQUIRED)\nfind_package(OpenSSL REQUIRED)\n\nadd_executable(foo\n  src/main.cpp   # entry point\n  src/util.cpp\n)\n\ntarget_link_libraries(foo PRIVATE\n  Threads::Threads\n  OpenSSL::SSL\n)\n";

    #[test]
    fn detect_real_cmake() {
        assert!(detect(REAL_CMAKE));
    }

    #[test]
    fn detect_rejects_unrelated() {
        assert!(!detect("hello\nworld\n"));
        assert!(!detect("<?xml version='1.0'?>\n"));
        assert!(!detect("# only comments\n# more comments\n"));
    }

    #[test]
    fn detect_rejects_powershell_script() {
        // #589 — production showed `cat install.ps1` (PowerShell) being
        // mis-detected as cmake at 33% efficiency. The PS script
        // matched on if/else/function/return/set — all CMAKE_COMMANDS
        // entries but ALL also PowerShell keywords. With the
        // distinctive-token requirement, no `add_*`/`target_*`/etc.
        // appears → reject.
        let ps = "param(\n    [string]$Target = \"linux\"\n)\n\n\
                  if ($Target -eq \"windows\") {\n  \
                      Write-Host \"Installing for Windows\"\n  \
                      $cache = \"$env:LOCALAPPDATA\\tokenade\"\n\
                  } else {\n  \
                      Write-Host \"Installing for Linux\"\n  \
                      $cache = \"$env:HOME/.tokenade\"\n\
                  }\n\n\
                  function Install-Binary {\n  \
                      param([string]$Url)\n  \
                      Invoke-WebRequest -Uri $Url -OutFile $cache\n  \
                      return $true\n\
                  }\n\n\
                  Install-Binary -Url \"https://example.com/tokenade.tar.gz\"\n";
        assert!(
            !detect(ps),
            "PowerShell script with only generic CMake keywords must NOT be detected as cmake"
        );
    }

    #[test]
    fn detect_rejects_bash_with_generic_keywords() {
        // Sibling false-positive: bash scripts also have `if`/`else`/
        // `function`/`return`/`set`. Without distinctive tokens, must
        // reject.
        let sh = "#!/bin/bash\nset -euo pipefail\n\n\
                  function deploy {\n  \
                      if [ -f /etc/foo ]; then\n    \
                          echo \"found\"\n  \
                      else\n    \
                          echo \"missing\"\n  \
                      fi\n  \
                      return 0\n\
                  }\n\
                  deploy\n";
        assert!(
            !detect(sh),
            "Bash script with only generic CMake keywords must NOT be detected as cmake"
        );
    }

    #[test]
    fn detect_accepts_cmake_without_underscore_tokens_via_endif_project() {
        // #597 — a minimal CMakeLists.txt may not contain any `_`
        // token (no `cmake_minimum_required`, no `add_executable`,
        // no `target_*`). It still has `project` + `endif` + an
        // `if`/`set`/etc., all CMake-only signals. The distinctive
        // set must include these unique-non-`_` tokens or this
        // legit CMake file gets rejected.
        let legit = "project(foo)\n\
                     if(WIN32)\n  \
                         set(VAR \"windows\")\n\
                     elseif(UNIX)\n  \
                         set(VAR \"unix\")\n\
                     endif()\n\
                     message(STATUS \"VAR=${VAR}\")\n";
        assert!(
            detect(legit),
            "CMakeLists with `project`+`endif`+`elseif` (no _-tokens) must be accepted"
        );
    }

    #[test]
    fn detect_still_rejects_powershell_after_endif_extension() {
        // #597 paranoia: PowerShell has `if`/`else`/`function`/
        // `return`/`set`/`include`/`file`/`list`/`string` etc. but
        // NOT `endif`/`endforeach`/`endfunction`/`endmacro`/
        // `endwhile`/`elseif`/`project` (the new distinctive
        // additions). Verify the #589 PowerShell fixture is STILL
        // rejected after the extension.
        let ps = "param(\n    [string]$Target = \"linux\"\n)\n\n\
                  if ($Target -eq \"windows\") {\n  \
                      Write-Host \"Installing\"\n  \
                      $cache = \"$env:LOCALAPPDATA\\tokenade\"\n\
                  } else {\n  \
                      Write-Host \"Installing for Linux\"\n  \
                      $cache = \"$env:HOME/.tokenade\"\n\
                  }\n\n\
                  function Install-Binary {\n  \
                      param([string]$Url)\n  \
                      return $true\n\
                  }\n";
        assert!(
            !detect(ps),
            "PowerShell must still be rejected after #597 distinctive extension"
        );
    }

    #[test]
    fn detect_rejects_real_tokenade_install_ps1() {
        // #661 — production gain.jsonl on 2026-06-01 02:00 showed
        // `cat install/install.ps1` detected as auto:cmake at 33 %
        // saved despite #589/#597. The event predated the deploy of
        // those fixes (binary was rebuilt 12:13 the same day) — but
        // capture the REAL content of install.ps1 as a regression
        // test so a future refactor of CMAKE_COMMANDS / the
        // distinctive set can't quietly re-introduce the bug.
        //
        // First-word frequencies in install.ps1's first 80 effective
        // lines: `if` ×many, `function` ×2, `else` ×2, plus `try`/
        // `finally` and many `$varname` assignments. None are
        // distinctive (no `_`, none in the cmake-unique-no-underscore
        // set). So `distinctive` stays at 0 and detect() returns
        // false even though `hits >= 3`.
        let real_install_ps1 = r#"# Tokenade Windows installer (PowerShell)
$ErrorActionPreference = "Stop"
$Repo = "tokenade/tokenade"
$Version = if ($env:TOKENADE_VERSION) { $env:TOKENADE_VERSION } else { "latest" }
$BinDir = if ($env:TOKENADE_BIN_DIR) { $env:TOKENADE_BIN_DIR } else { Join-Path $env:USERPROFILE ".local\bin" }
function Say($msg)  { Write-Host "[tokenade] $msg" -ForegroundColor Cyan }
function Err($msg)  { Write-Host "[tokenade] error: $msg" -ForegroundColor Red; exit 1 }
if ($env:TOKENADE_TARGET) {
    $target = $env:TOKENADE_TARGET
    Say "platform overridden by TOKENADE_TARGET: $target"
} else {
    $arch = (Get-WmiObject -Class Win32_Processor).Architecture
    switch ($arch) {
        9  { $archTag = "x86_64" }
        12 { $archTag = "aarch64" }
        default { Err "unsupported architecture: $arch" }
    }
    $target = "${archTag}-pc-windows-msvc"
    Say "detected platform: $target"
}
if ($Version -eq "latest") {
    $url = "https://github.com/${Repo}/releases/latest/download/tokenade-${target}.zip"
} else {
    $url = "https://github.com/${Repo}/releases/download/${Version}/tokenade-${target}.zip"
}
$tmp = New-Item -ItemType Directory -Path ([System.IO.Path]::GetTempPath() + [System.Guid]::NewGuid())
try {
    $zip = Join-Path $tmp "tokenade.zip"
    if ($env:TOKENADE_LOCAL_TARBALL) {
        Copy-Item -Path $env:TOKENADE_LOCAL_TARBALL -Destination $zip -Force
    } else {
        Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
    }
}
"#;
        assert!(
            !detect(real_install_ps1),
            "real tokenade install.ps1 content must not pass CMake detection"
        );
    }

    #[test]
    fn detect_accepts_minimal_real_cmake_with_one_distinctive_token() {
        // Sanity: a small CMakeLists.txt with the minimum surface
        // (3 hits, one distinctive) is still accepted.
        let minimal = "cmake_minimum_required(VERSION 3.20)\n\
                       project(foo)\n\
                       add_executable(foo src/main.cpp)\n";
        assert!(detect(minimal));
    }

    #[test]
    fn compact_strips_full_line_comments() {
        let out = compact(REAL_CMAKE);
        assert!(!out.contains("Top-level CMakeLists"));
        assert!(!out.contains("Copyright"));
        assert!(!out.contains("Default to Release"));
    }

    #[test]
    fn compact_strips_inline_comments() {
        let out = compact(REAL_CMAKE);
        assert!(!out.contains("entry point"));
        assert!(out.contains("src/main.cpp"));
    }

    #[test]
    fn compact_preserves_commands_and_control_flow() {
        let out = compact(REAL_CMAKE);
        assert!(out.contains("cmake_minimum_required(VERSION 3.20)"));
        assert!(out.contains("project(foo CXX)"));
        assert!(out.contains("if(NOT CMAKE_BUILD_TYPE)"));
        assert!(out.contains("set(CMAKE_BUILD_TYPE Release)"));
        assert!(out.contains("endif()"));
        assert!(out.contains("find_package(Threads REQUIRED)"));
        assert!(out.contains("add_executable(foo"));
        assert!(out.contains("target_link_libraries(foo PRIVATE"));
    }

    #[test]
    fn compact_preserves_hash_inside_quoted_string() {
        let input = "set(URL \"https://example.com/p#frag\")\n";
        let out = compact(input);
        assert!(out.contains("p#frag"));
    }

    #[test]
    fn compact_collapses_blank_runs() {
        let input = "project(x)\n\n\n\nset(Y 1)\n";
        let out = strip_only(input);
        assert_eq!(out, "project(x)\n\nset(Y 1)\n");
    }

    #[test]
    fn real_world_ratio_meets_expected() {
        // Measured on the STRIP, not on the declared output. The two are
        // different claims and were one number: this fixture is a few hundred
        // bytes, so the 81-byte declaration costs ~9 points here, while on the
        // real files these fixtures stand in for it costs 1.9-3.7 (measured
        // 2026-08-26 on four Dockerfiles of 2.2-4.4 KB, savings 10-50%).
        // Folding the note into the ratio would have read as the compactor
        // getting worse. What declaring costs is asserted once for every
        // language in `declaration_costs_at_most_one_line`, size-independently.

        let (_, sav) = crate::archetypes::savings_of(REAL_CMAKE, strip_only);
        assert!(
            sav.percent() >= 15.0,
            "expected ≥15% on commented CMake, got {:.1}%",
            sav.percent()
        );
    }
}

#[cfg(test)]
mod a_cmake_command_is_a_call {
    //! `project` made a Python file distinctive-for-CMake, and the line was
    //! `project repo repository codebase source` — a word list inside a
    //! string. CMake has no command form without parentheses, so requiring
    //! the call costs nothing the language allows.

    #[test]
    fn a_word_list_is_not_a_project_directive() {
        let py = "KEYWORDS = \"\"\"\nproject repo repository codebase source\nbuild compile link target\n\"\"\"\n\n\ndef pick(x):\n    if x:\n        return 1\n    if not x:\n        return 0\n    return None\n\n\ndef other(y):\n    if y:\n        return 2\n    return None\n";
        assert!(!super::detect(py), "a word list is not a CMake call");
    }

    #[test]
    fn an_assignment_named_project_is_not_a_directive() {
        let py = "project = Path(__file__).parent\n\nif project.exists():\n    return 1\n\nif project.is_dir():\n    return 2\n\nreturn None\n";
        assert!(!super::detect(py));
    }

    #[test]
    fn a_real_cmakelists_still_detects() {
        let cm = "cmake_minimum_required(VERSION 3.16)\nproject(Demo CXX)\n\nadd_executable(demo src/main.cpp)\ntarget_link_libraries(demo PRIVATE fmt)\n\nif(WIN32)\n  add_definitions(-DWIN32)\nendif()\n";
        assert!(super::detect(cm));
    }

    #[test]
    fn the_space_before_the_paren_cmake_tolerates_still_counts() {
        let cm = "cmake_minimum_required (VERSION 3.16)\nproject (Demo CXX)\n\nadd_executable (demo src/main.cpp)\ntarget_link_libraries (demo PRIVATE fmt)\n";
        assert!(super::detect(cm), "CMake allows a space before the paren");
    }
}
