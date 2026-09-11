//! Builds the Vue SPA into `web/dist` so `rust-embed` can bake it into the binary.
//!
//! `#[derive(RustEmbed)] #[folder = "web/dist"]` is a *compile-time* failure when the
//! folder is absent, so `ensure_dist_placeholder` runs first and unconditionally —
//! that is what makes `cargo build` work on a clean clone with no Node installed.
//!
//! Set `SKIP_UI_BUILD=1` to skip the npm step entirely (used by `make check` and the
//! test targets, where the bundle is irrelevant and npm would only cost time).

use std::path::Path;
use std::process::Command;
use std::time::SystemTime;

const DIST: &str = "web/dist";
const PLACEHOLDER: &str = concat!(
    "<!doctype html><meta charset=\"utf-8\"><title>api2mcp</title>",
    "<body style=\"font:16px system-ui;padding:3rem;max-width:40rem\">",
    "<h1>UI not built</h1><p>Run <code>make ui</code> (or unset <code>SKIP_UI_BUILD</code>) ",
    "and reload. The API and the MCP endpoint are unaffected.</p>"
);

fn main() {
    // Watch the *sources*, never web/dist — build.rs writes there, and watching it loops.
    for p in [
        "web/src",
        "web/package.json",
        "web/index.html",
        "web/vite.config.ts",
    ] {
        println!("cargo:rerun-if-changed={p}");
    }
    println!("cargo:rerun-if-env-changed=SKIP_UI_BUILD");
    emit_version_metadata();

    ensure_dist_placeholder();

    if std::env::var_os("SKIP_UI_BUILD").is_some_and(|v| v != "0") {
        return;
    }
    if !sources_newer_than_dist() {
        return;
    }
    if !Path::new("web/node_modules").exists() && npm(&["ci"]).is_err() {
        fail("npm ci failed");
        return;
    }
    if npm(&["run", "build"]).is_err() {
        fail("npm run build failed");
    }
}

/// A failed UI build is fatal in release (shipping a placeholder in a release binary is
/// worse than a red build) and a warning in debug (so an unrelated backend change is not
/// blocked by a broken frontend).
fn fail(what: &str) {
    let release = std::env::var("PROFILE").as_deref() == Ok("release");
    let msg = format!(
        "{what}; the embedded UI is a placeholder. Fix with: cd web && npm ci && npm run build"
    );
    if release {
        panic!("{msg}");
    }
    println!("cargo:warning={msg}");
}

fn ensure_dist_placeholder() {
    let dist = Path::new(DIST);
    if std::fs::create_dir_all(dist).is_err() {
        return;
    }
    let index = dist.join("index.html");
    if !index.exists() {
        let _ = std::fs::write(index, PLACEHOLDER);
    }
}

fn sources_newer_than_dist() -> bool {
    let Some(built) = mtime(Path::new(DIST).join("index.html")) else {
        return true;
    };
    [
        "web/src",
        "web/package.json",
        "web/index.html",
        "web/vite.config.ts",
    ]
    .iter()
    .filter_map(|p| newest(Path::new(p)))
    .any(|src| src > built)
}

fn newest(path: &Path) -> Option<SystemTime> {
    if path.is_file() {
        return mtime(path);
    }
    std::fs::read_dir(path)
        .ok()?
        .filter_map(|e| newest(&e.ok()?.path()))
        .max()
}

fn mtime(path: impl AsRef<Path>) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

fn npm(args: &[&str]) -> Result<(), ()> {
    let status = Command::new("npm").args(args).current_dir("web").status();
    match status {
        Ok(s) if s.success() => Ok(()),
        _ => Err(()),
    }
}

/// `DESIGN`-style version metadata without pulling in a date crate.
fn emit_version_metadata() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    let commit = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=A2M_COMMIT={commit}");
    let pkg = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    println!("cargo:rustc-env=A2M_LONG_VERSION={pkg} ({commit})");
}
