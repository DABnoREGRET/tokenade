//! **The console's own code page, for the console's own programs.**
//!
//! On Windows the native console tools — `ipconfig`, `tasklist`,
//! `systeminfo`, `nslookup`, `netstat`… — write in the OEM code page: 850 on a
//! French or German system, 437 on a US one, 866 on a Russian one. That is
//! not UTF-8, and `chardetng` — which knows no DOS code page — guesses
//! windows-1252: measured 2026-10-02 on a real Windows 10 (fr-FR), `nslookup`
//! came back as `R‚ponse ne faisant pas autorit‚` and `tasklist` as
//! `26ÿ236 Ko`. Characters SUBSTITUTED are characters fabricated.
//!
//! Only for those programs: a FILE `cat`ted on the same machine is in the ANSI
//! code page (an Excel CSV is windows-1252), and decoding it as OEM would turn
//! its `é` into `Ú`. The program says which of the two the bytes are.
//!
//! `encoding_rs` carries IBM866 but neither 437 nor 850; their upper halves are
//! below, generated from Python's `codecs` tables, not typed by hand.

/// Bytes 0x80..=0xFF of code page 437.
const CP437_HIGH: &str = "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒáíóúñÑªº¿⌐¬½¼¡«»░▒▓│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■\u{a0}";
/// Bytes 0x80..=0xFF of code page 850.
const CP850_HIGH: &str = "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜø£Ø×ƒáíóúñÑªº¿®¬½¼¡«»░▒▓│┤ÁÂÀ©╣║╗╝¢¥┐└┴┬├─┼ãÃ╚╔╩╦╠═╬¤ðÐÊËÈıÍÎÏ┘┌█▄¦Ì▀ÓßÔÒõÕµþÞÚÛÙýÝ¯´\u{ad}±‗¾¶§÷¸°¨·¹³²■\u{a0}";

/// `bytes` decoded in OEM code page `cp`, or `None` for a page not carried.
pub fn decode(bytes: &[u8], cp: u32) -> Option<String> {
    let high: Vec<char> = match cp {
        437 => CP437_HIGH.chars().collect(),
        850 => CP850_HIGH.chars().collect(),
        866 => {
            return Some(
                encoding_rs::IBM866
                    .decode_without_bom_handling(bytes)
                    .0
                    .into_owned(),
            )
        }
        _ => return None,
    };
    debug_assert_eq!(high.len(), 128);
    Some(
        bytes
            .iter()
            .map(|&b| {
                if b < 0x80 {
                    b as char
                } else {
                    high[(b - 0x80) as usize]
                }
            })
            .collect(),
    )
}

/// The system's OEM code page — what a console program writes in. `None` off
/// Windows, where no program writes in one.
pub fn console_codepage() -> Option<u32> {
    #[cfg(windows)]
    {
        // SAFETY: `GetOEMCP` takes no argument and reads a process-wide value.
        Some(unsafe { windows_sys::Win32::Globalization::GetOEMCP() })
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Native Windows console programs: they print in the OEM code page.
const CONSOLE_PROGRAMS: &[&str] = &[
    "arp",
    "attrib",
    "bcdedit",
    "certutil",
    "chkdsk",
    "cmd",
    "dir",
    "driverquery",
    "fc",
    "getmac",
    "gpresult",
    "icacls",
    "ipconfig",
    "nbtstat",
    "net",
    "netsh",
    "netstat",
    "nslookup",
    "pathping",
    "ping",
    "powercfg",
    "query",
    "quser",
    "reg",
    "robocopy",
    "route",
    "sc",
    "schtasks",
    "sfc",
    "systeminfo",
    "taskkill",
    "tasklist",
    "tracert",
    "ver",
    "vol",
    "whoami",
    "wmic",
    "xcopy",
];

/// Is `prog` (a path, any case, with or without `.exe`) one of them?
pub fn is_console_program(prog: &str) -> bool {
    let base = prog
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(prog)
        .to_ascii_lowercase();
    let base = base.strip_suffix(".exe").unwrap_or(&base);
    CONSOLE_PROGRAMS.contains(&base)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real bytes from a Windows 10 fr-FR: `Réponse`, `26 236 Ko` (cp850).
    #[test]
    fn a_french_console_output_reads_correctly() {
        assert_eq!(
            decode(b"R\x82ponse ne faisant pas autorit\x82", 850).unwrap(),
            "Réponse ne faisant pas autorité"
        );
        assert_eq!(decode(b"26\xff236 Ko", 850).unwrap(), "26\u{a0}236 Ko");
        assert_eq!(decode(b"\x9b", 850).unwrap(), "ø");
        assert_eq!(decode(b"\x9b", 437).unwrap(), "¢");
        assert_eq!(decode(b"\x8f\xe0\xa8\xa2\xa5\xe2", 866).unwrap(), "Привет");
        assert!(decode(b"x", 1252).is_none());
        assert_eq!(CP437_HIGH.chars().count(), 128);
        assert_eq!(CP850_HIGH.chars().count(), 128);
    }

    /// Every entry of the table is recognised, under all its spellings.
    #[test]
    fn every_console_program_is_recognised() {
        for p in CONSOLE_PROGRAMS {
            assert!(is_console_program(p), "{p}");
            assert!(
                is_console_program(&format!(
                    "C:\\Windows\\System32\\{}.EXE",
                    p.to_ascii_uppercase()
                )),
                "{p}"
            );
        }
    }

    #[test]
    fn only_console_programs_are_concerned() {
        assert!(
            is_console_program("ipconfig")
                && is_console_program("C:\\Windows\\System32\\TASKLIST.EXE")
        );
        assert!(
            !is_console_program("cat") && !is_console_program("git") && !is_console_program("find")
        );
    }
}
