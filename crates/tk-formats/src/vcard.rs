//! **Contacts (`.vcf`, vCard 2.1–4.0), one card per person.**
//!
//! A phone or mail-app export embeds each contact's PHOTO as base64 — measured
//! on a two-card iPhone export: 9 270 bytes, three quarters of them two
//! pictures, served as text (the output was even LONGER, past a redaction
//! banner). The rest is `VERSION`/`PRODID`/`REV`/`UID` and Apple `X-AB…` lines.
//!
//! Each card becomes a short block of what identifies and reaches the person;
//! a photo is one line with its type and size; the plumbing is counted.

use crate::icalendar::{props, unescape, Prop};
use std::fmt::Write;

pub fn detect(text: &str) -> bool {
    text.trim_start_matches('\u{feff}')
        .trim_start()
        .to_ascii_uppercase()
        .starts_with("BEGIN:VCARD")
}

/// The property name without its group (`item1.ADR` → `ADR`).
fn bare(p: &Prop) -> &str {
    p.name.rsplit('.').next().unwrap_or(&p.name)
}

/// Every `TYPE` a property carries, however it is spelled (`type=CELL;type=pref`,
/// `TYPE=work,voice`, or vCard 2.1's bare `;CELL`).
fn types(p: &Prop) -> String {
    let mut out: Vec<String> = Vec::new();
    for (k, v) in &p.params {
        if k == "TYPE" {
            out.extend(v.split(',').map(|t| t.to_ascii_lowercase()));
        }
    }
    if out.is_empty() {
        String::new()
    } else {
        format!(" ({})", out.join(", "))
    }
}

/// A structured value (`;`-separated, `\;` escaped) as its non-empty parts.
fn parts(v: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut it = v.chars();
    while let Some(c) = it.next() {
        match c {
            '\\' => {
                if let (Some(n), Some(last)) = (it.next(), out.last_mut()) {
                    last.push(if n == 'n' || n == 'N' { '\n' } else { n });
                }
            }
            ';' => out.push(String::new()),
            c => {
                if let Some(last) = out.last_mut() {
                    last.push(c);
                }
            }
        }
    }
    out.into_iter()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

const PLUMBING: [&str; 4] = ["VERSION", "PRODID", "REV", "UID"];

pub fn compact(text: &str) -> String {
    let all = props(text);
    let mut cards: Vec<Vec<&Prop>> = Vec::new();
    let mut hidden: Vec<String> = Vec::new();
    for p in &all {
        match (bare(p), cards.last_mut()) {
            ("BEGIN", _) if p.value.eq_ignore_ascii_case("VCARD") => cards.push(Vec::new()),
            ("BEGIN" | "END", _) => {}
            (n, Some(_)) if PLUMBING.contains(&n) => hidden.push(n.to_string()),
            (n, Some(_)) if n.starts_with("X-") => hidden.push(n.to_string()),
            (_, Some(card)) => card.push(p),
            _ => {}
        }
    }
    let mut out = format!("[contacts: {}]\n", cards.len());
    for card in &cards {
        out.push('\n');
        render(card, &mut out, &mut hidden);
    }
    if !hidden.is_empty() {
        let mut names: Vec<(String, usize)> = Vec::new();
        for h in &hidden {
            match names.iter_mut().find(|(n, _)| n == h) {
                Some((_, c)) => *c += 1,
                None => names.push((h.clone(), 1)),
            }
        }
        let list: Vec<String> = names
            .iter()
            .map(|(n, c)| {
                if *c > 1 {
                    format!("{n} ×{c}")
                } else {
                    n.clone()
                }
            })
            .collect();
        let _ = writeln!(out, "\n[not shown: {}]", list.join(", "));
    }
    out
}

fn render(card: &[&Prop], out: &mut String, hidden: &mut Vec<String>) {
    let name = card
        .iter()
        .find(|p| bare(p) == "FN")
        .map(|p| unescape(&p.value))
        .or_else(|| {
            card.iter().find(|p| bare(p) == "N").map(|p| {
                parts(&p.value)
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
        })
        .unwrap_or_else(|| "(no name)".into());
    let _ = writeln!(out, "## {name}");
    for p in card {
        let label = match bare(p) {
            "FN" | "N" => continue,
            "ORG" => {
                let _ = writeln!(out, "Organization: {}", parts(&p.value).join(" — "));
                continue;
            }
            "ADR" => {
                let _ = writeln!(out, "Address: {}{}", parts(&p.value).join(", "), types(p));
                continue;
            }
            "PHOTO" | "LOGO" | "SOUND" | "KEY" => {
                // An embedded file is one line, never its bytes; a URL stays a URL.
                let inline = p.param("ENCODING").is_some() || p.value.starts_with("data:");
                if inline {
                    let b64 = p.value.rsplit(',').next().unwrap_or(&p.value);
                    let kind = p
                        .param("TYPE")
                        .map(|t| format!("{} ", t.to_ascii_uppercase()))
                        .unwrap_or_default();
                    let _ = writeln!(
                        out,
                        "[{}: {kind}~{} KB, not shown]",
                        bare(p).to_ascii_lowercase(),
                        (b64.len() * 3 / 4).div_ceil(1024)
                    );
                } else {
                    let _ = writeln!(out, "{}: {}", bare(p).to_ascii_lowercase(), p.value);
                }
                continue;
            }
            "TEL" => "Phone",
            "EMAIL" => "Email",
            "TITLE" => "Title",
            "ROLE" => "Role",
            "NOTE" => "Note",
            "URL" => "URL",
            "BDAY" => "Birthday",
            "NICKNAME" => "Nickname",
            "IMPP" => "Chat",
            "CATEGORIES" => "Groups",
            other => {
                hidden.push(other.to_string());
                continue;
            }
        };
        let _ = writeln!(
            out,
            "{label}: {}{}",
            unescape(&p.value).replace('\n', " / "),
            types(p)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_contact_export_reads_as_cards_with_photos_as_one_line() {
        let photo = "QUJD".repeat(700);
        let vcf = format!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nN:Martin;Alice;;;\r\nFN:Alice Martin\r\nORG:ACME\\, Inc.;Ventes\r\n\
             TEL;type=CELL;type=pref:+33 6 12 34 56 70\r\nEMAIL;TYPE=work,internet:alice@acme.example\r\n\
             item1.ADR;type=WORK:;;12 rue de la Paix;Paris;;75002;France\r\nitem1.X-ABADR:fr\r\n\
             NOTE:Salon\\nRappeler\r\nPHOTO;ENCODING=b;TYPE=JPEG:{}\r\n {}\r\nEND:VCARD\r\n\
             BEGIN:VCARD\r\nVERSION:4.0\r\nN:Durand;Bob;;;\r\nPHOTO:https://x.example/bob.jpg\r\nEND:VCARD\r\n",
            &photo[..1400],
            &photo[1400..]
        );
        assert!(detect(&vcf));
        assert_eq!(
            compact(&vcf),
            "[contacts: 2]\n\n## Alice Martin\nOrganization: ACME, Inc. — Ventes\n\
             Phone: +33 6 12 34 56 70 (cell, pref)\nEmail: alice@acme.example (work, internet)\n\
             Address: 12 rue de la Paix, Paris, 75002, France (work)\nNote: Salon / Rappeler\n\
             [photo: JPEG ~3 KB, not shown]\n\n## Bob Durand\nphoto: https://x.example/bob.jpg\n\n\
             [not shown: VERSION ×2, X-ABADR]\n"
        );
    }
}
