//! Framework detection — Next.js, Django, Rails, NestJS, Spring, …
//!
//! Knowing the framework an agent is editing lets STS:
//!   1. Boost framework-canonical files in semantic search (a query about
//!      "routing" in a Next.js project should rank `pages/`, `app/` and
//!      `next.config.*` higher than other matches);
//!   2. Surface framework-specific entry points in `tokenade map`;
//!   3. Skip framework-managed directories that aren't already in the index
//!      skip list (e.g. `.next/`, `staticfiles/`, `tmp/cache/`).
//!
//! Detection is **manifest-driven** — we look for `package.json` deps,
//! `requirements.txt` / `pyproject.toml`, `Gemfile`, `pom.xml`, `mix.exs`,
//! `Cargo.toml`, `composer.json`, etc. — and **config-file marker** files
//! (`next.config.*`, `nuxt.config.*`, `vite.config.*`, `manage.py`,
//! `config/application.rb`, `pom.xml` with Spring deps, …). Multiple matches
//! return the most specific one (a Next.js app is a React app, but we'd
//! report Next.js).

use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Framework {
    // ---- JS/TS ----
    NextJs,
    Nuxt,
    Remix,
    Astro,
    SvelteKit,
    NestJs,
    Express,
    Fastify,
    React,
    Vue,
    Svelte,
    // ---- Python ----
    Django,
    Flask,
    FastApi,
    // ---- Ruby ----
    Rails,
    Sinatra,
    // ---- Java/Kotlin ----
    SpringBoot,
    // ---- PHP ----
    Laravel,
    Symfony,
    // ---- C#/.NET ----
    AspNet,
    // ---- Elixir ----
    Phoenix,
    // ---- Rust ----
    Axum,
    Actix,
    Rocket,
    // ---- nothing ----
    Unknown,
}

impl Framework {
    pub fn as_str(&self) -> &'static str {
        match self {
            Framework::NextJs => "next.js",
            Framework::Nuxt => "nuxt",
            Framework::Remix => "remix",
            Framework::Astro => "astro",
            Framework::SvelteKit => "sveltekit",
            Framework::NestJs => "nestjs",
            Framework::Express => "express",
            Framework::Fastify => "fastify",
            Framework::React => "react",
            Framework::Vue => "vue",
            Framework::Svelte => "svelte",
            Framework::Django => "django",
            Framework::Flask => "flask",
            Framework::FastApi => "fastapi",
            Framework::Rails => "rails",
            Framework::Sinatra => "sinatra",
            Framework::SpringBoot => "spring-boot",
            Framework::Laravel => "laravel",
            Framework::Symfony => "symfony",
            Framework::AspNet => "asp.net",
            Framework::Phoenix => "phoenix",
            Framework::Axum => "axum",
            Framework::Actix => "actix",
            Framework::Rocket => "rocket",
            Framework::Unknown => "unknown",
        }
    }

    /// Canonical entry files / directories where the framework's "interesting"
    /// code lives — used to boost semantic-search ranking. Path segments are
    /// matched as substrings against indexed file paths.
    pub fn canonical_paths(&self) -> &'static [&'static str] {
        match self {
            Framework::NextJs => &["pages/", "app/", "next.config", "middleware."],
            Framework::Nuxt => &["pages/", "components/", "nuxt.config", "middleware/"],
            Framework::Remix => &["app/routes/", "remix.config"],
            Framework::Astro => &["src/pages/", "astro.config"],
            Framework::SvelteKit => &["src/routes/", "svelte.config"],
            Framework::NestJs => &[
                "src/main.ts",
                "src/app.module",
                "src/**/*.controller",
                "src/**/*.service",
            ],
            Framework::Express => &["app.js", "server.js", "routes/", "index.js"],
            Framework::Fastify => &["server.js", "plugins/", "routes/"],
            Framework::React => &["src/App", "src/components/", "src/pages/"],
            Framework::Vue => &["src/App.vue", "src/components/", "src/views/"],
            Framework::Svelte => &["src/App.svelte", "src/lib/"],
            Framework::Django => &[
                "manage.py",
                "settings.py",
                "urls.py",
                "views.py",
                "models.py",
            ],
            Framework::Flask => &["app.py", "wsgi.py", "blueprints/"],
            Framework::FastApi => &["main.py", "app.py", "routers/", "schemas.py"],
            Framework::Rails => &[
                "app/controllers/",
                "app/models/",
                "app/views/",
                "config/routes.rb",
            ],
            Framework::Sinatra => &["app.rb", "config.ru"],
            Framework::SpringBoot => {
                &["src/main/java", "src/main/resources/application", "pom.xml"]
            }
            Framework::Laravel => &["app/Http/", "routes/web.php", "routes/api.php"],
            Framework::Symfony => &["src/Controller/", "config/routes."],
            Framework::AspNet => &["Program.cs", "Startup.cs", "Controllers/"],
            Framework::Phoenix => &["lib/", "lib/*_web/", "router.ex", "endpoint.ex"],
            Framework::Axum => &["src/main.rs", "src/routes/", "src/handlers/"],
            Framework::Actix => &["src/main.rs", "src/routes/", "src/handlers/"],
            Framework::Rocket => &["src/main.rs", "src/routes/"],
            Framework::Unknown => &[],
        }
    }
}

/// Detect the framework rooted at `root`. Returns `Unknown` if no manifest
/// matches a known framework.
pub fn detect(root: &Path) -> Framework {
    // ---- JS / TS via package.json ----
    if let Ok(pkg) = fs::read_to_string(root.join("package.json")) {
        // Order matters: more specific frameworks first.
        let in_deps = |name: &str| pkg.contains(&format!("\"{name}\""));
        if in_deps("next")
            || root.join("next.config.js").exists()
            || root.join("next.config.ts").exists()
            || root.join("next.config.mjs").exists()
        {
            return Framework::NextJs;
        }
        if in_deps("nuxt") || root.join("nuxt.config.ts").exists() {
            return Framework::Nuxt;
        }
        if in_deps("@remix-run/react") || in_deps("@remix-run/node") {
            return Framework::Remix;
        }
        if in_deps("astro") {
            return Framework::Astro;
        }
        if in_deps("@sveltejs/kit") {
            return Framework::SvelteKit;
        }
        if in_deps("@nestjs/core") || in_deps("@nestjs/common") {
            return Framework::NestJs;
        }
        if in_deps("fastify") {
            return Framework::Fastify;
        }
        if in_deps("express") {
            return Framework::Express;
        }
        if in_deps("react") {
            return Framework::React;
        }
        if in_deps("vue") {
            return Framework::Vue;
        }
        if in_deps("svelte") {
            return Framework::Svelte;
        }
    }

    // ---- Python ----
    let req = read_or_empty(&[
        root.join("requirements.txt"),
        root.join("pyproject.toml"),
        root.join("Pipfile"),
        root.join("setup.py"),
    ]);
    if !req.is_empty() {
        // Django: also signalled by `manage.py` at root.
        if req.to_lowercase().contains("django") || root.join("manage.py").exists() {
            return Framework::Django;
        }
        if req.to_lowercase().contains("fastapi") {
            return Framework::FastApi;
        }
        if req.to_lowercase().contains("flask") {
            return Framework::Flask;
        }
    }

    // ---- Ruby ----
    if let Ok(gem) = fs::read_to_string(root.join("Gemfile")) {
        if gem.contains("rails") || root.join("config/application.rb").exists() {
            return Framework::Rails;
        }
        if gem.contains("sinatra") {
            return Framework::Sinatra;
        }
    }

    // ---- Java / Kotlin ----
    let jvm = read_or_empty(&[
        root.join("pom.xml"),
        root.join("build.gradle"),
        root.join("build.gradle.kts"),
    ]);
    if jvm.contains("spring-boot") || jvm.contains("org.springframework.boot") {
        return Framework::SpringBoot;
    }

    // ---- PHP ----
    if let Ok(composer) = fs::read_to_string(root.join("composer.json")) {
        if composer.contains("laravel/framework") {
            return Framework::Laravel;
        }
        if composer.contains("symfony/symfony") || composer.contains("symfony/framework-bundle") {
            return Framework::Symfony;
        }
    }

    // ---- C# / .NET ----
    if root.join("Program.cs").exists() || root.join("Startup.cs").exists() {
        // Check any .csproj file for AspNet markers.
        if let Ok(entries) = fs::read_dir(root) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if name.ends_with(".csproj") {
                    if let Ok(csproj) = fs::read_to_string(e.path()) {
                        if csproj.contains("Microsoft.AspNetCore")
                            || csproj.contains("Microsoft.NET.Sdk.Web")
                        {
                            return Framework::AspNet;
                        }
                    }
                }
            }
        }
    }

    // ---- Elixir ----
    if let Ok(mix) = fs::read_to_string(root.join("mix.exs")) {
        if mix.contains(":phoenix") || mix.contains("phoenix,") {
            return Framework::Phoenix;
        }
    }

    // ---- Rust ----
    if let Ok(cargo) = fs::read_to_string(root.join("Cargo.toml")) {
        if cargo.contains("axum =") || cargo.contains("axum=") || cargo.contains("axum\"") {
            return Framework::Axum;
        }
        if cargo.contains("actix-web =") || cargo.contains("actix-web=") {
            return Framework::Actix;
        }
        if cargo.contains("rocket =") || cargo.contains("rocket=") {
            return Framework::Rocket;
        }
    }

    Framework::Unknown
}

fn read_or_empty(paths: &[std::path::PathBuf]) -> String {
    let mut combined = String::new();
    for p in paths {
        if let Ok(s) = fs::read_to_string(p) {
            combined.push_str(&s);
            combined.push('\n');
        }
    }
    combined
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "tokenade-fw-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(p: &Path, content: &str) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut f = fs::File::create(p).unwrap();
        f.write_all(content.as_bytes()).unwrap();
    }

    #[test]
    fn detects_nextjs_from_package_json() {
        let d = tmp();
        write(
            &d.join("package.json"),
            r#"{"name":"x","dependencies":{"next":"14","react":"18"}}"#,
        );
        assert_eq!(detect(&d), Framework::NextJs);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn detects_django_via_manage_py() {
        let d = tmp();
        write(&d.join("manage.py"), "#!/usr/bin/env python\nimport django");
        write(&d.join("requirements.txt"), "Django==5.0\npsycopg2");
        assert_eq!(detect(&d), Framework::Django);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn detects_rails_via_gemfile_and_app_layout() {
        let d = tmp();
        write(
            &d.join("Gemfile"),
            "source 'https://rubygems.org'\ngem 'rails', '~> 7.1'",
        );
        assert_eq!(detect(&d), Framework::Rails);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn detects_spring_boot_from_pom() {
        let d = tmp();
        write(
            &d.join("pom.xml"),
            r#"<project><dependencies>
              <dependency><groupId>org.springframework.boot</groupId><artifactId>spring-boot-starter</artifactId></dependency>
            </dependencies></project>"#,
        );
        assert_eq!(detect(&d), Framework::SpringBoot);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn detects_phoenix_via_mix_exs() {
        let d = tmp();
        write(
            &d.join("mix.exs"),
            "defmodule App.MixProject do\n  defp deps do\n    [{:phoenix, \"~> 1.7\"}]\n  end\nend",
        );
        assert_eq!(detect(&d), Framework::Phoenix);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn detects_axum_via_cargo() {
        let d = tmp();
        write(
            &d.join("Cargo.toml"),
            "[package]\nname=\"x\"\n[dependencies]\naxum = \"0.7\"\ntokio = \"1\"",
        );
        assert_eq!(detect(&d), Framework::Axum);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn unknown_when_nothing_matches() {
        let d = tmp();
        write(&d.join("README.md"), "# A repo with no framework markers");
        assert_eq!(detect(&d), Framework::Unknown);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn nextjs_wins_over_react_when_both_present() {
        // Adversarial: Next.js IS React, but we should report the more
        // specific framework, not the generic library.
        let d = tmp();
        write(
            &d.join("package.json"),
            r#"{"dependencies":{"next":"14","react":"18","react-dom":"18"}}"#,
        );
        assert_eq!(detect(&d), Framework::NextJs);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn canonical_paths_help_search_boost() {
        // Each known framework must expose at least one canonical path so
        // semantic search has something to boost against.
        for fw in [
            Framework::NextJs,
            Framework::Django,
            Framework::Rails,
            Framework::SpringBoot,
            Framework::Phoenix,
        ] {
            assert!(
                !fw.canonical_paths().is_empty(),
                "no canonical paths for {:?}",
                fw
            );
        }
    }
}
