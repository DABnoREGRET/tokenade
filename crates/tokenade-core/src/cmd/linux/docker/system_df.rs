//! `docker system df -v` — four tables, one of which is usually the whole
//! output.
//!
//! On a machine that has run a few hundred `docker run`s without `--rm`, the
//! "Local Volumes" table holds thousands of anonymous volumes: 64-hex names,
//! no container linked, a handful of distinct sizes. MEASURED on a real
//! machine: 2 341 lines, 198 610 bytes, of which 2 250 rows were anonymous
//! volumes — delivered whole, because `system` went to the auto router and no
//! detector reads four stacked tables. What the command is run for is "what
//! takes space, and what can go": named volumes and every other table are
//! kept as printed; the anonymous rows become one census line (count, links,
//! most common sizes, total).

use std::collections::BTreeMap;
use std::fmt::Write;

fn is_anonymous_row(line: &str) -> Option<(&str, &str)> {
    let mut cols = line.split_whitespace();
    let name = cols.next()?;
    if name.len() != 64 || !name.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let links = cols.next()?;
    let size = cols.next()?;
    (cols.next().is_none() && links.bytes().all(|b| b.is_ascii_digit())).then_some((links, size))
}

/// `19.67MB` → bytes, with Docker's decimal units.
fn size_bytes(s: &str) -> Option<f64> {
    let at = s.find(|c: char| c.is_ascii_alphabetic())?;
    let n: f64 = s[..at].parse().ok()?;
    let mult = match &s[at..] {
        "B" => 1.0,
        "kB" | "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        _ => return None,
    };
    Some(n * mult)
}

fn human(bytes: f64) -> String {
    for (unit, mult) in [("TB", 1e12), ("GB", 1e9), ("MB", 1e6), ("kB", 1e3)] {
        if bytes >= mult {
            return format!("{:.2}{unit}", bytes / mult);
        }
    }
    format!("{bytes:.0}B")
}

pub fn compact(output: &str) -> String {
    let anonymous: Vec<(&str, &str)> = output.lines().filter_map(is_anonymous_row).collect();
    if anonymous.len() < 10 {
        return output.to_string();
    }
    let mut links: BTreeMap<&str, usize> = BTreeMap::new();
    let mut sizes: BTreeMap<&str, usize> = BTreeMap::new();
    let mut total = 0.0;
    let mut unparsed = 0usize;
    for (l, s) in &anonymous {
        *links.entry(l).or_default() += 1;
        *sizes.entry(s).or_default() += 1;
        match size_bytes(s) {
            Some(b) => total += b,
            None => unparsed += 1,
        }
    }
    let mut by_count: Vec<(&str, usize)> = sizes.into_iter().collect();
    by_count.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let shown: Vec<String> = by_count.iter().take(5).map(|(s, n)| format!("{s}×{n}")).collect();
    let rest = by_count.len().saturating_sub(5);
    let links: Vec<String> = links.iter().map(|(l, n)| format!("{l}×{n}")).collect();
    let mut census = format!(
        "[{} anonymous volume(s) — 64-hex names omitted · LINKS {} · SIZE {}{}",
        anonymous.len(),
        links.join(", "),
        shown.join(", "),
        if rest > 0 { format!(", +{rest} other size(s)") } else { String::new() },
    );
    let _ = write!(census, " · total {}", human(total));
    if unparsed > 0 {
        let _ = write!(census, " (+{unparsed} size(s) not summed)");
    }
    census.push(']');

    let mut out = String::with_capacity(output.len() / 8);
    let mut written = false;
    for line in output.lines() {
        if is_anonymous_row(line).is_some() {
            if !written {
                out.push_str(&census);
                out.push('\n');
                written = true;
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.truncate(out.trim_end().len());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(i: usize) -> String {
        format!("{:064x}", i * 2_654_435_761 + 17)
    }

    #[test]
    fn anonymous_volumes_become_a_census_and_everything_else_stays() {
        let mut t = String::from(
            "Images space usage:\n\nREPOSITORY   TAG   IMAGE ID       CREATED       SIZE\nredis        7.2   aca82ccee5d9   6 weeks ago   38.6MB\n\n\
             Local Volumes space usage:\n\nVOLUME NAME                                                        LINKS     SIZE\n",
        );
        let sizes = ["19.67MB", "19.67MB", "19.67MB", "488B", "50.23kB", "0B"];
        for i in 0..60 {
            let _ = writeln!(t, "{}   0         {}", hex(i), sizes[i % sizes.len()]);
        }
        t.push_str("orbit-dev_pg                                                       1         78.68MB\n");
        let _ = writeln!(t, "{}   1         2.1GB", hex(999));
        t.push_str("\nBuild cache usage: 4.035GB\n\nCACHE ID       CACHE TYPE     SIZE\n23alosdgaqod   regular        0B\n");
        let out = compact(&t);
        assert!(out.contains("redis        7.2   aca82ccee5d9   6 weeks ago   38.6MB"), "{out}");
        assert!(out.contains("orbit-dev_pg                                                       1         78.68MB"));
        assert!(out.contains("23alosdgaqod   regular        0B"));
        assert!(!out.contains(&hex(3)), "{out}");
        let census = out.lines().find(|l| l.starts_with("[61 anonymous")).expect(&out);
        assert!(census.contains("LINKS 0×60, 1×1"), "{census}");
        assert!(census.contains("SIZE 19.67MB×30, "), "{census}");
        // 30×19.67MB + 10×488B + 10×50.23kB + 2.1GB
        assert!(census.contains("total 2.69GB"), "{census}");
        assert!(out.find("[61 anonymous").unwrap() < out.find("orbit-dev_pg").unwrap());
    }

    /// The route the agent takes. `-v` reads as a request for raw bytes, and
    /// was honoured at ANY size: the 2 341-line real output then reached the
    /// stash, whose head and tail kept one container of ten and no named
    /// volume. Past the delivery threshold the request yields to this file.
    #[test]
    fn the_verbose_flag_does_not_keep_a_huge_listing_from_its_compactor() {
        let mut t = String::from(
            "Images space usage:\n\nREPOSITORY   TAG   IMAGE ID       CREATED       SIZE\nredis        7.2   aca82ccee5d9   6 weeks ago   38.6MB\n\n\
             Local Volumes space usage:\n\nVOLUME NAME                                                        LINKS     SIZE\n",
        );
        for i in 0..1_500 {
            let size = if i % 9 == 0 { "488B" } else { "19.67MB" };
            let _ = writeln!(t, "{}   0         {size}", hex(i));
        }
        t.push_str("myapp_hf-cache                                         0         62.94GB\n");
        assert!(crate::tokenizer::estimate(&t) > crate::disclose::threshold_tokens());
        let argv: Vec<String> = ["docker", "system", "df", "-v"].iter().map(|s| s.to_string()).collect();
        let r = crate::filter::filter_command(&argv, &t);
        assert!(r.output.contains("[1500 anonymous volume(s)"), "{}", &r.output[..r.output.len().min(400)]);
        assert!(r.output.contains("myapp_hf-cache"), "the named volume is the answer");
        assert!(r.output.contains("redis        7.2"));
    }

    #[test]
    fn a_handful_of_volumes_is_left_alone() {
        let t = format!("VOLUME NAME   LINKS   SIZE\n{}   0   1MB\n", hex(1));
        assert_eq!(compact(&t), t);
    }
}
