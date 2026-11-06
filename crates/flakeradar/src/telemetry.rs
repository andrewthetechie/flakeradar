//! Log output for the binary.
//!
//! One process-wide `tracing` subscriber, installed by the binary at start-up.
//! It writes to stdout at `INFO` unless `RUST_LOG` says otherwise, so a
//! deployment that sets nothing still logs every event this server emits.

// Rust guideline compliant 2026-07-21

use tracing_subscriber::EnvFilter;

/// Installs the process-wide log subscriber.
///
/// `RUST_LOG` wins when it is set, so an operator can raise or lower the level
/// without a rebuild. A subscriber that is already installed is left in place,
/// which keeps a second call harmless.
// Process-wide state a test cannot install twice or observe from another thread;
// see `docs/plans/rust-rewrite/coverage-gaps.md`.
#[cfg_attr(coverage_nightly, coverage(off))]
pub fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}
