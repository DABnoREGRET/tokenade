//! `sc_query` — sc query.
//!
//! Form-aware: keeps this command's specific detector, but folds its
//! output by detected shape — columnar table (surfaces problem rows),
//! key/value records (never cut mid-record), else head+tail (the final
//! outcome line is never decapitated). Replaces the blind head-slice.
use std::fmt::Write;

const HEAD: usize = 50;

pub fn detect(text: &str) -> bool {
    text.contains("SERVICE_NAME") || text.contains("STATE")
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    // **One line per service, while that fits.** A record is ten lines and
    // most of them are the same everywhere (`CHECKPOINT : 0x0`, `WAIT_HINT :
    // 0x0`, the type, the accepted controls): measured 2026-10-03 on a real
    // `sc queryex type= service state= all` (Windows 10, 232 services, 99 KB)
    // the head cut kept 59 lines — five services. Name, state, PID, display
    // name and any non-zero code carry what is asked of this command.
    if let Some(table) = services_table(text).filter(|t| tk_base::delivery::fits(t)) {
        return Some(table);
    }
    crate::archetypes::form_aware(text, HEAD, 5, "sc-query", "line(s)")
}

/// `KEY : value` — `sc` pads the key, and the French and English keys are the
/// same words.
fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let (k, v) = line.split_once(':')?;
    (k.trim() == key).then(|| v.trim())
}

fn services_table(text: &str) -> Option<String> {
    #[derive(Default)]
    struct Svc<'a> {
        name: &'a str,
        display: &'a str,
        state: &'a str,
        pid: &'a str,
        codes: Vec<String>,
    }
    let mut services: Vec<Svc> = Vec::new();
    for line in text.lines() {
        if let Some(name) = field(line, "SERVICE_NAME") {
            services.push(Svc { name, ..Svc::default() });
            continue;
        }
        let Some(svc) = services.last_mut() else { continue };
        if let Some(v) = field(line, "DISPLAY_NAME") {
            svc.display = v;
        } else if let Some(v) = field(line, "STATE") {
            // `4  RUNNING` — the word, not its number.
            svc.state = v.split_whitespace().nth(1).unwrap_or(v);
        } else if let Some(v) = field(line, "PID") {
            svc.pid = v;
        } else {
            for key in ["WIN32_EXIT_CODE", "SERVICE_EXIT_CODE", "CHECKPOINT", "WAIT_HINT"] {
                if let Some(v) = field(line, key) {
                    let n = v.split_whitespace().next().unwrap_or(v);
                    if n != "0" && n != "0x0" {
                        svc.codes.push(format!("{key}={n}"));
                    }
                }
            }
        }
    }
    if services.len() < 2 || services.iter().any(|s| s.state.is_empty()) {
        return None;
    }
    let mut states: Vec<&str> = Vec::new();
    for s in &services {
        if !states.contains(&s.state) {
            states.push(s.state);
        }
    }
    let mut out = format!(
        "[{} services — name · PID · display name; TYPE, the accepted controls and zero codes omitted]\n",
        services.len()
    );
    for state in states {
        let group: Vec<&Svc> = services.iter().filter(|s| s.state == state).collect();
        let _ = writeln!(out, "{state} ({})", group.len());
        for s in group {
            let pid = if s.pid.is_empty() || s.pid == "0" { String::new() } else { format!("  {}", s.pid) };
            let codes = if s.codes.is_empty() { String::new() } else { format!("  {}", s.codes.join(" ")) };
            let _ = writeln!(out, "  {}{pid}  {}{codes}", s.name, s.display);
        }
    }
    Some(out.trim_end().to_string())
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detector_rejects_empty() { assert!(!detect("")); }

    /// The real `sc queryex` record, a running and a stopped service.
    #[test]
    fn one_line_per_service_grouped_by_state() {
        let rec = |name: &str, display: &str, state: &str, exit: &str, pid: &str| {
            format!("\nSERVICE_NAME: {name}\nDISPLAY_NAME: {display}\n        TYPE               : 20  WIN32_SHARE_PROCESS\n        STATE              : {state}\n                                (STOPPABLE, NOT_PAUSABLE, ACCEPTS_SHUTDOWN)\n        WIN32_EXIT_CODE    : {exit}\n        SERVICE_EXIT_CODE  : 0  (0x0)\n        CHECKPOINT         : 0x0\n        WAIT_HINT          : 0x0\n        PID                : {pid}\n        FLAGS              :\n")
        };
        let mut s = rec("AJRouter", "Service de routeur AllJoyn", "1  STOPPED", "1077  (0x435)", "0");
        s += &rec("Dhcp", "Client DHCP", "4  RUNNING", "0  (0x0)", "1300");
        s += &rec("ALG", "Service de la passerelle", "1  STOPPED", "1077  (0x435)", "0");
        let out = try_compact(&s).expect("a services table");
        let lines: Vec<&str> = out.lines().skip(1).collect();
        assert_eq!(lines, [
            "STOPPED (2)",
            "  AJRouter  Service de routeur AllJoyn  WIN32_EXIT_CODE=1077",
            "  ALG  Service de la passerelle  WIN32_EXIT_CODE=1077",
            "RUNNING (1)",
            "  Dhcp  1300  Client DHCP",
        ]);
        assert!(out.starts_with("[3 services"));
    }

    #[test]
    fn detector_accepts_specific_format() { assert!(detect("SERVICE_NAME: wuauserv")); }

    #[test]
    fn short_output_passes_through() {
        let s: String = std::iter::repeat_n("SERVICE_NAME: wuauserv", 3).map(|l| format!("{l}\n")).collect();
        assert!(try_compact(&s).is_none());
    }

    #[test]
    fn long_output_folds_non_decapitating() {
        let s: String = std::iter::repeat_n("SERVICE_NAME: wuauserv", HEAD + 20).map(|l| format!("{l}\n")).collect();
        let out = try_compact(&s).unwrap();
        assert!(out.contains("[tokenade:sc-query"));
    }
}
