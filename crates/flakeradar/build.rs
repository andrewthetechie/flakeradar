//! Declares whether the built frontend is there to embed.
//!
//! `frontend/` is a separate Vite project, and cargo never runs npm: the order
//! is `npm run build` in `frontend/`, then `cargo build`. This script only
//! reports what it finds. `frontend/dist/index.html` present means the crate is
//! compiled with `cfg(has_frontend)` and `rust-embed` carries the directory into
//! the binary; absent means a warning and an API-only binary that serves no
//! page.

// Rust guideline compliant 2026-07-21

use std::io::Write as _;
use std::path::PathBuf;

/// The directory `npm run build` writes, relative to this crate.
const DIST: &str = "../../frontend/dist";

fn main() {
    // Cargo reads the script's instructions from standard output, so this is the
    // one place in the workspace that writes to it.
    let emit = |line: &str| {
        let _ = writeln!(std::io::stdout(), "{line}");
    };
    // A build script that names any dependency of its own stops cargo's default
    // "rerun on any change in the package", so the script names itself.
    emit("cargo:rerun-if-changed=build.rs");
    emit("cargo:rustc-check-cfg=cfg(has_frontend)");
    let dist = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(DIST);
    emit(&format!("cargo:rerun-if-changed={}", dist.display()));
    if dist.join("index.html").is_file() {
        emit("cargo:rustc-cfg=has_frontend");
    } else {
        emit(&format!(
            "cargo:warning={} has no index.html: building an API-only binary. \
             Run `npm run build` in frontend/ to embed the dashboard.",
            dist.display()
        ));
    }
}
