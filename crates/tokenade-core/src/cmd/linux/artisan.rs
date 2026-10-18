//! `artisan` — `php artisan <command>` (Laravel), every command but `test`.
//!
//! Laravel aligns its two-column views with DOT LEADERS: `route:list`,
//! `about`, `migrate:status`, `schedule:list`, `event:list`, `db:show` all
//! print `  GET|HEAD  api/users ......... users.index › UsersController@index`.
//! The dots carry nothing — they only reach the terminal's right edge — and
//! they were delivered whole: measured 2026-10-02 on a 50-route
//! `route:list`, 7 772 bytes in, 7 772 out (the `php` arm hands everything to
//! content detection, which saw no shape). Found while reading a competitor
//! hook (`Claude-Code-Token-Compressor`) that names `route:list` as its main
//! target and answers it with `| head` — a cut, where this is lossless.

/// The rule is shared with document extraction (`tk_base::leaders`).
fn collapse_leaders(line: &str) -> String {
    crate::leaders::collapse_dot_leaders(line)
}

pub fn detect(text: &str) -> bool {
    text.lines().filter(|l| l.contains(" ....")).count() >= 3
}

pub fn try_compact(text: &str) -> Option<String> {
    if !detect(text) {
        return None;
    }
    let mut out: Vec<String> = text.lines().map(collapse_leaders).collect();
    // `Showing [50] routes` is right-aligned under 100 spaces.
    for l in &mut out {
        if l.len() - l.trim_start().len() > 8 && !l.contains(" … ") {
            *l = format!("  {}", l.trim_start());
        }
    }
    let joined = out.join("\n");
    (joined.len() < text.len()).then_some(joined)
}

crate::archetypes::cmd_entry_points!(crate::archetypes::passthrough);

#[cfg(test)]
mod tests {
    use super::*;

    fn route_list() -> String {
        let mut s = String::from("\n");
        for (m, uri, name) in [("GET|HEAD", "api/users", "users.index"), ("POST", "api/users", "users.store"), ("DELETE", "api/users/{id}", "users.destroy")] {
            let left = format!("  {m:<10}{uri} ");
            let right = format!(" {name} › Api\\UsersController@x");
            s.push_str(&format!("{left}{}{right}\n", ".".repeat(150 - left.len() - right.len())));
        }
        s.push_str(&format!("\n{}Showing [3] routes\n", " ".repeat(100)));
        s
    }

    #[test]
    fn the_leaders_go_and_every_field_stays() {
        let out = try_compact(&route_list()).expect("a route list");
        assert!(out.contains("  GET|HEAD  api/users … users.index › Api\\UsersController@x"), "{out}");
        assert!(out.contains("  DELETE    api/users/{id} … users.destroy"));
        assert!(out.contains("\n  Showing [3] routes"));
        assert!(!out.contains("...."));
        assert!(out.len() * 2 < route_list().len());
    }

    #[test]
    fn an_ellipsis_inside_text_is_not_a_leader() {
        assert_eq!(collapse_leaders("  wait... done ...."), "  wait... done …");
        assert_eq!(collapse_leaders("v1...v2 ......"), "v1...v2 …");
        assert_eq!(collapse_leaders("a ...b ...."), "a ...b …", "dots glued to a word are text");
        assert!(!detect("one line ....\nanother\n"));
    }

    /// The `php` arm handed everything to content detection: 7 772 bytes in,
    /// 7 772 out.
    #[test]
    fn php_artisan_reaches_it_through_the_filter() {
        let argv: Vec<String> = ["php", "artisan", "route:list"].iter().map(|s| s.to_string()).collect();
        let r = crate::filter::filter_command(&argv, &route_list());
        assert!(r.output.contains("api/users … users.index"), "filter `{}`:\n{}", r.filter, r.output);
    }
}
