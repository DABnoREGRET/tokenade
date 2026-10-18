//! Build llama.cpp (latest master, or `LLAMA_CPP_DIR`) as static libs, link
//! them, and bindgen `llama.h`. Per project decision, when we build llama.cpp
//! ourselves we ALWAYS pull the latest master — MiniCPM5 (and other new models)
//! only run correctly on recent llama.cpp.
use std::path::{Path, PathBuf};
use std::process::Command;

const REPO: &str = "https://github.com/ggml-org/llama.cpp";

fn main() {
    let root = match std::env::var_os("LLAMA_CPP_DIR") {
        Some(d) => PathBuf::from(d), // pre-built tree (CI / offline / dev)
        None => build_latest(),      // clone + build latest master
    };

    let inc = root.join("include");
    let ggml_inc = root.join("ggml").join("include");
    let build = root.join(build_subdir());

    // Static libs (llama + the ggml backends). Search every dir they land in —
    // single-config generators (Make/Ninja) drop them in build/<sub>, while
    // multi-config ones (MSVC/Xcode) nest them under build/<sub>/Release.
    for sub in ["src", "ggml/src", "ggml/src/ggml-cpu", "ggml/src/ggml-cuda"] {
        let d = build.join(sub);
        println!("cargo:rustc-link-search=native={}", d.display());
        println!(
            "cargo:rustc-link-search=native={}",
            d.join("Release").display()
        );
    }
    // Order matters for static linking (user before definer): llama → ggml
    // frontend → backends → base.
    for lib in ["llama", "ggml", "ggml-cpu"] {
        println!("cargo:rustc-link-lib=static={lib}");
    }
    if cuda_enabled() {
        // Dev-only GPU build (TOKENADE_LLAMA_CUDA=1): link the CUDA backend and
        // the CUDA runtime. Never on by default — shipped builds stay CPU-only.
        println!("cargo:rustc-link-lib=static=ggml-cuda");
        let cuda_root = std::env::var("CUDA_PATH").unwrap_or_else(|_| "/opt/cuda".into());
        println!("cargo:rustc-link-search=native={cuda_root}/lib64");
        println!("cargo:rustc-link-search=native={cuda_root}/lib64/stubs");
        for lib in ["cudart", "cublas", "cublasLt", "cuda"] {
            println!("cargo:rustc-link-lib=dylib={lib}");
        }
    }
    println!("cargo:rustc-link-lib=static=ggml-base");
    // Per-target runtime deps. OpenMP is disabled in the cmake config above, so
    // there's no libgomp to link on any platform.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    match target_os.as_str() {
        "macos" | "ios" => {
            // Apple: libc++ + the Accelerate framework (ggml-cpu's BLAS backend).
            println!("cargo:rustc-link-lib=dylib=c++");
            println!("cargo:rustc-link-lib=framework=Accelerate");
        }
        "windows" => {
            // MSVC links its C/C++ runtime automatically; nothing extra needed.
        }
        _ => {
            // Linux/BSD: libstdc++ + libm. Static libstdc++ on musl (fully static
            // binaries) or when TOKENADE_LLAMA_STATIC_CXX=1 is set.
            let static_cxx =
                target_env == "musl" || std::env::var_os("TOKENADE_LLAMA_STATIC_CXX").is_some();
            let kind = if static_cxx { "static" } else { "dylib" };
            println!("cargo:rustc-link-lib={kind}=stdc++");
            println!("cargo:rustc-link-lib=dylib=m");
        }
    }
    println!("cargo:rerun-if-env-changed=TOKENADE_LLAMA_STATIC_CXX");

    let bindings = bindgen::Builder::default()
        .header(inc.join("llama.h").to_string_lossy())
        .clang_arg(format!("-I{}", inc.display()))
        .clang_arg(format!("-I{}", ggml_inc.display()))
        .allowlist_function("llama_.*")
        .allowlist_type("llama_.*")
        .allowlist_var("LLAMA_.*")
        .allowlist_var("GGML_.*")
        .prepend_enum_name(false)
        // Disable bindgen's generated size/align self-tests: on some structs
        // they emit const array tricks that overflow at const-eval. The
        // bindings themselves are correct (bindgen runs natively on-target).
        .layout_tests(false)
        .generate()
        .expect("bindgen failed on llama.h");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out.join("bindings.rs"))
        .expect("write bindings");

    println!("cargo:rerun-if-env-changed=LLAMA_CPP_DIR");
    println!("cargo:rerun-if-env-changed=LLAMA_CPP_REFRESH");
    println!("cargo:rerun-if-env-changed=TOKENADE_LLAMA_CUDA");
}

/// Clone (or refresh) the latest llama.cpp master into a persistent cache and
/// build its static libs. Cached across builds so we don't recompile llama.cpp
/// every `cargo build`; set `LLAMA_CPP_REFRESH=1` to re-pull latest.
fn build_latest() -> PathBuf {
    let cache = cache_dir();
    let src = cache.join("llama.cpp");
    let refresh = std::env::var_os("LLAMA_CPP_REFRESH").is_some();

    if src.join(".git").exists() && refresh {
        run(Command::new("git")
            .arg("-C")
            .arg(&src)
            .args(["pull", "--depth", "1", "--ff-only"]));
    } else if !src.exists() {
        std::fs::create_dir_all(&cache).ok();
        run(Command::new("git")
            .args(["clone", "--depth", "1", REPO])
            .arg(&src));
    }

    let build = src.join(build_subdir());
    let mut cfg = Command::new(cmake());
    cfg.arg("-B").arg(&build).arg("-S").arg(&src).args([
        "-DBUILD_SHARED_LIBS=OFF",
        "-DLLAMA_CURL=OFF",
        "-DLLAMA_BUILD_TESTS=OFF",
        "-DLLAMA_BUILD_TOOLS=OFF",
        "-DLLAMA_BUILD_EXAMPLES=OFF",
        "-DLLAMA_BUILD_SERVER=OFF",
        "-DLLAMA_BUILD_APP=OFF",
        "-DCMAKE_BUILD_TYPE=Release",
        "-DCMAKE_POSITION_INDEPENDENT_CODE=ON",
        // Zero-install portability: no OpenMP (drops the libgomp runtime
        // dep — matters for musl/macOS/Windows static builds) and no Metal
        // (no .metallib to ship; CPU + Accelerate on macOS is enough for the
        // small sub-agent models). Override via LLAMA_CPP_DIR if you want a
        // custom build.
        "-DGGML_OPENMP=OFF",
        "-DGGML_METAL=OFF",
    ]);
    if cuda_enabled() {
        cfg.arg("-DGGML_CUDA=ON");
    }
    run(&mut cfg);
    run(Command::new(cmake())
        .args(["--build"])
        .arg(&build)
        .args(["--config", "Release", "-j"]));
    src
}

/// Dev-only opt-in GPU build. Shipped builds never set this.
fn cuda_enabled() -> bool {
    std::env::var("TOKENADE_LLAMA_CUDA").ok().as_deref() == Some("1")
}

/// Separate build dirs per backend so toggling CUDA doesn't thrash the cache.
fn build_subdir() -> &'static str {
    if cuda_enabled() {
        "build-cuda"
    } else {
        "build"
    }
}

fn cache_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("LLAMA_CPP_CACHE") {
        return PathBuf::from(d);
    }
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| std::env::temp_dir());
    base.join("tokenade-llama")
}

fn cmake() -> String {
    std::env::var("CMAKE").unwrap_or_else(|_| "cmake".into())
}

fn run(cmd: &mut Command) {
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("failed to spawn {cmd:?}: {e}"));
    assert!(status.success(), "command failed ({status}): {cmd:?}");
}

// `_unused` was removed on 2026-09-13: an empty function, with no caller,
// covered by an exemption that did not say why.
