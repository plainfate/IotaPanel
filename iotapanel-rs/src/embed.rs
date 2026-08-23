//! Embedded static assets: frontend (web) and bundled plugin packages.
//!
//! Mirrors the original Go `internal/embed` package (compile-time embed via
//! Go `//go:embed`). Here we use `include_dir!` to bake `assets/` into the
//! binary at compile time.
//!
//! Plugin packages live under `assets/plugins/<name>/bin/<name>.gz` (gzipped
//! binaries, matching what the original `build.sh` produced) and are
//! decompressed at install time.

use include_dir::{include_dir, Dir};

/// Bundled frontend: index/login/setup.html, css, js.
pub static WEB: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/assets/web");

/// Bundled official plugin packages.
pub static PLUGINS: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/assets/plugins");

/// Read a file from the embedded frontend as bytes. `rel` is relative to the
/// web root, e.g. `"web/css/app.css"` or `"web/index.html"`.
pub fn web_file(rel: &str) -> Option<&'static [u8]> {
    match WEB.get_file(rel) {
        Some(f) => Some(f.contents()),
        None => WEB.get_file(match rel.strip_prefix("web/") {
            Some(r) => r,
            None => rel,
        }).map(|f| f.contents()),
    }
}