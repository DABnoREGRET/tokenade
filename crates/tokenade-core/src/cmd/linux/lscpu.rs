//! `lscpu` — CPU information.
//!
//! ```text
//! Architecture:                       x86_64
//! CPU op-mode(s):                     32-bit, 64-bit
//! Byte Order:                         Little Endian
//! Address sizes:                      48 bits physical, 48 bits virtual
//! CPU(s):                             8
//! On-line CPU(s) list:                0-7
//! Thread(s) per core:                 2
//! Core(s) per socket:                 4
//! Socket(s):                          1
//! ...
//! Vulnerability ...:                  ...
//! Vulnerability ...:                  ...
//! ```
//!
//! Compaction: drop `Vulnerability` rows (CVE listings are long
//! and noisy for typical workflows). Drop `Flags` row (huge).


/// The VALUE of a `label : value` line, or `None` if there is none.
fn value_of(line: &str) -> Option<&str> {
    line.split_once(':').map(|(_, v)| v.trim())
}

/// `lscpu` recognised WITHOUT a single word of language.
///
/// The detector required `architecture:` glued, then a label
/// tolerant of six languages. Both are words, and a word in the
/// DETECTOR is a gate: when it is missing, the compactor does not run at
/// all. MEASURED 2026-09-07 by scrambling the labels of the corpus's 383
/// fixtures — rot13 left of the colon, structure intact — **120
/// dedicated compactors still fire and 15 drop out**; this one was
/// among them.
///
/// What `lscpu` has that is invariant needs no reading:
/// - a key/value BLOCK: at least eight `label : value` lines whose
///   label is a bare label (no quote, no brace, no comma —
///   otherwise a `docker inspect` carrying `x86_64` would pass for `lscpu`);
/// - an architecture VALUE (`x86_64`, `aarch64`, …), which no locale
///   translates, carried by one of those lines.
///
/// A log line that mentions `x86_64` in passing is not a block;
/// a key/value block without an architecture is not `lscpu`.
pub fn detect(text: &str) -> bool {
    const ARCH_VALUE: [&str; 8] = [
        "x86_64", "aarch64", "armv7l", "armv6l", "ppc64le", "s390x", "riscv64", "i686",
    ];
    let mut rows = 0usize;
    let mut arch = false;
    for line in text.lines() {
        let Some((label, v)) = line.split_once(':') else {
            continue;
        };
        // A block line, not a JSON or log line: the label
        // is a bare label. Without that, a `docker inspect` — hundreds of
        // `"key": "value",` lines one of which carries `x86_64` — would pass for
        // `lscpu`.
        if label.contains(['"', '{', '}', ',', '[']) || label.trim().is_empty() {
            continue;
        }
        let v = v.trim();
        rows += 1;
        if ARCH_VALUE.iter().any(|a| v.split_whitespace().any(|w| w == *a)) {
            arch = true;
        }
    }
    arch && rows >= 8
}

crate::archetypes::detect_gated_try_compact!();

/// The processor flags line, recognised by its SHAPE.
///
/// Its label is translated (`Flags:`, `Drapeaux :`, `Markierungen:`), its
/// value is not: a list of short, lowercase tokens without spaces — the real
/// line of a modern machine carries 214 of them, and an old machine a dozen.
/// The threshold is TWELVE: no other `lscpu` line lines up that many
/// all-lowercase tokens (model names carry capitals, vulnerability
/// verdicts punctuation), and the compactor only runs
/// on output already recognised as `lscpu`.
fn is_flags_row(line: &str) -> bool {
    let Some(v) = value_of(line) else {
        return false;
    };
    let mut n = 0usize;
    for tok in v.split_whitespace() {
        if !tok
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
        {
            return false;
        }
        n += 1;
    }
    n >= 12
}

/// A vulnerability line, recognised by its VERDICT.
///
/// The label is translated (`Vulnerability Spectre v2`, `Vulnérabilité Spectre
/// v2`, `Sicherheitslücke …`); the verdict the kernel publishes comes from
/// `/sys/devices/system/cpu/vulnerabilities` and is translated in no
/// locale. Checked on the French capture: `Vulnérabilité Ghostwrite :
/// Not affected`.
fn is_vulnerability_row(line: &str) -> bool {
    let Some(v) = value_of(line) else {
        return false;
    };
    // The verdict is not always first: the kernel publishes
    // `KVM: Mitigation: VMX disabled` for `itlb_multihit`, where the measure is
    // preceded by the subsystem that carries it. So we look at each segment.
    v.split(": ").any(|seg| {
        ["Not affected", "Mitigation", "Vulnerable", "Unknown"]
            .iter()
            .any(|verdict| seg.starts_with(verdict))
    })
}

/// A vulnerability row the kernel calls SETTLED — `Not affected` or
/// `Mitigation…` in every segment. Those are the noise this compactor drops.
/// A `Vulnerable` or `Unknown` verdict is the rare line the whole section
/// exists for, and it used to go with the rest: twenty rows elided under one
/// count, the one that said the CPU was exposed among them.
fn is_settled_vulnerability_row(line: &str) -> bool {
    is_vulnerability_row(line)
        && value_of(line).is_some_and(|v| !v.contains("Vulnerable") && !v.contains("Unknown"))
}

pub fn compact(text: &str) -> String {
    let kept = crate::archetypes::elide_where(
        text,
        |line| is_flags_row(line) || is_settled_vulnerability_row(line),
        "Flags / settled Vulnerability line(s)",
    );
    // `Architecture:                    x86_64` — the label is padded to the
    // longest one, two thirds of what was left (real `lscpu`, fr_FR,
    // 2026-10-04). The shared rule keeps one space per indentation level.
    crate::archetypes::collapse_alignment_padding(&kept)
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real `lscpu` block is dozens of lines; these excerpts are
    /// eight, the minimum the detector requires — a three-line output
    /// does not exist, and a threshold that accepted it would also accept the
    /// first line that happens to carry `x86_64`.
    const FR: &str = "Architecture :                            x86_64\n\
                      Mode(s) opératoire(s) des processeurs :   32-bit, 64-bit\n\
                      Tailles=des adresses:                     48 bits physical\n\
                      Boutisme :                                Little Endian\n\
                      Processeur(s) :                           24\n\
                      Liste=de processeur(s) en ligne :         0-23\n\
                      Identifiant constructeur :                AuthenticAMD\n\
                      Nom de modèle :                           AMD Ryzen 9\n\
                      Thread(s) par cœur :                      2\n";
    const EN: &str = "Architecture:                       x86_64\n\
                      CPU op-mode(s):                     32-bit, 64-bit\n\
                      Address sizes:                      48 bits physical\n\
                      Byte Order:                         Little Endian\n\
                      CPU(s):                             24\n\
                      On-line CPU(s) list:                0-23\n\
                      Vendor ID:                          AuthenticAMD\n\
                      Model name:                         AMD Ryzen 9\n\
                      Thread(s) per core:                 2\n";

    /// The output of a machine in `fr_FR.UTF-8`: translated labels, space
    /// before the colon. The full fixture is in
    /// `tests/fixtures/linux/lscpu_full.txt`.
    #[test]
    fn a_vulnerable_verdict_is_kept_and_settled_ones_are_not() {
        let t = "Architecture : x86_64\nProcesseur(s) : 24\nNom de modèle : AMD Ryzen 9 7900X\nThread(s) par cœur : 2\nCœur(s) par socket : 12\nSocket(s) : 1\nBogoMIPS : 9400,65\nVirtualisation : AMD-V\n\
                 Drapeaux : fpu vme de pse tsc msr pae mce cx8 apic sep mtrr pge mca cmov pat pse36 clflush mmx fxsr sse sse2\n\
                 Vulnérabilité Ghostwrite : Not affected\n\
                 Vulnérabilité Spec store bypass : Mitigation; Speculative Store Bypass disabled via prctl\n\
                 Vulnérabilité Retbleed : Vulnerable: untrained return thunk; SMT enabled with STIBP protection\n\
                 Vulnérabilité Itlb multihit : KVM: Mitigation: VMX disabled\n";
        assert!(detect(t));
        let out = compact(t);
        assert!(out.contains("Retbleed : Vulnerable: untrained return thunk"), "{out}");
        assert!(!out.contains("Ghostwrite") && !out.contains("Spec store bypass") && !out.contains("Itlb"), "{out}");
        assert!(!out.contains("fpu vme"), "{out}");
    }

    #[test]
    fn detects_lscpu_in_french() {
        assert!(detect(FR), "the French output is not recognised");
        let with_noise = format!(
            "{FR}Vulnérabilité Spectre v2 :                Mitigation; Retpolines\n\
             Drapeaux :                                fpu vme de pse tsc msr pae mce cx8 apic sep mtrr pge mca cmov\n"
        );
        let out = compact(&with_noise);
        assert!(out.contains("Architecture"), "{out}");
        assert!(!out.contains("Retpolines"), "{out}");
        assert!(!out.contains("cmov"), "{out}");
    }

    /// The detector reads NO word: scrambling all the labels must
    /// change nothing. It is the property the corpus measures on 383 fixtures,
    /// pinned here on this one.
    #[test]
    fn detects_lscpu_in_a_language_nobody_speaks() {
        let mangled: String = EN
            .lines()
            .map(|l| match l.split_once(':') {
                Some((k, v)) => format!("{}:{v}\n", k.chars().rev().collect::<String>()),
                None => format!("{l}\n"),
            })
            .collect();
        assert!(detect(&mangled), "the detector still reads words:\n{mangled}");
    }

    /// Anti-vacuity: neither an isolated line, nor a JSON block that talks
    /// about architecture.
    #[test]
    fn a_line_or_a_json_naming_an_architecture_is_not_lscpu() {
        assert!(!detect("Compiling serde v1.0 for x86_64-unknown-linux-gnu\n"));
        let json = "{\n  \"Os\": \"linux\",\n  \"Arch\": \"x86_64\",\n  \"a\": \"1\",\n  \"b\": \"2\",\n  \"c\": \"3\",\n  \"d\": \"4\",\n  \"e\": \"5\",\n  \"f\": \"6\",\n  \"g\": \"7\"\n}\n";
        assert!(!detect(json), "a JSON carrying x86_64 passes for lscpu");
    }

    #[test]
    fn detects_lscpu() {
        assert!(detect(EN));
    }

    #[test]
    fn drops_vulnerability_rows() {
        let s = "Architecture: x86_64\nCPU(s): 8\nFlags: fpu vme de pse tsc msr pae mce cx8 apic sep mtrr pge mca cmov pat pse36 clflush\nVulnerability Itlb multihit:        KVM: Mitigation: VMX disabled\nVulnerability L1tf:                 Mitigation; PTE Inversion; VMX conditional cache flushes, SMT vulnerable\n";
        let out = compact(s);
        assert!(out.contains("Architecture"));
        // Data rows gone (elision summary still mentions the word).
        assert!(!out.contains("Itlb multihit"));
        assert!(!out.contains("PTE Inversion"));
        assert!(!out.contains("Flags: fpu"));
        // Footer counts the elision.
        assert!(out.contains("elided:"));
    }

    /// The label padding goes, in any locale; the values stay.
    #[test]
    fn the_label_padding_goes() {
        let mut t = String::from("Architecture :                            x86_64\nNom de modèle :                           AMD Ryzen 9 7900X 12-Core Processor\n");
        for i in 0..8 {
            t.push_str(&format!("Champ {i} :                                valeur {i}\n"));
        }
        let out = compact(&t);
        assert!(out.contains("Architecture : x86_64\nNom de modèle : AMD Ryzen 9 7900X 12-Core Processor\n"), "{out}");
        assert!(!out.contains("  "), "{out}");
    }
}
