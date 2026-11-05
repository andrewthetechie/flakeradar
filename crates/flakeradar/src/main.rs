//! FlakeRadar entry point.
//!
//! The binary reads its settings, refuses the shipped default token, starts the
//! server and serves until it is interrupted. Everything it runs lives in the
//! `flakeradar` library, so the suite proves the path a deployment takes and
//! only the process edge is left here.

// cargo-llvm-cov sets `coverage_nightly` on nightly; see `lib.rs`.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

// Rust guideline compliant 2026-07-21

use std::process::ExitCode;

use anyhow::Context as _;
use flakeradar::{ALLOW_INSECURE_VARIABLE, Server, Settings, assert_secure_token, init_tracing};

// Process start-up, environment access and signal wiring are outside what a test
// process can run, so the coverage policy excludes them; see
// `docs/plans/rust-rewrite/coverage-gaps.md`.
#[cfg_attr(coverage_nightly, coverage(off))]
fn main() -> ExitCode {
    init_tracing();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // The subscriber is installed first, so a start-up failure reaches the
            // log rather than a bare exit status.
            tracing::error!(error = ?error, "flakeradar could not serve");
            ExitCode::FAILURE
        }
    }
}

/// Reads the settings, gates the token and serves until interrupted.
///
/// The token gate runs before the database is touched, so an instance left on
/// the shipped default never opens a pool.
#[cfg_attr(coverage_nightly, coverage(off))]
fn run() -> anyhow::Result<()> {
    let settings = Settings::from_env().context("reading the settings")?;
    let allow_insecure = std::env::var(ALLOW_INSECURE_VARIABLE).ok();
    assert_secure_token(&settings, allow_insecure.as_deref())?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("starting the async runtime")?;
    runtime.block_on(async move {
        let server = Server::start(settings).await?;
        server.serve(shutdown_signal()).await
    })
}

/// Resolves on Ctrl-C, or on SIGTERM where the platform has it.
///
/// The serve loop stops accepting requests and then stops the worker, so a
/// request in flight finishes. Signal wiring is the one part of the process a
/// test cannot run, so the coverage policy excludes it; see
/// `docs/plans/rust-rewrite/coverage-gaps.md`.
#[cfg_attr(coverage_nightly, coverage(off))]
async fn shutdown_signal() {
    let ctrl_c = async {
        // A failure to listen leaves the other arm to stop the process.
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                let _ = stream.recv().await;
            }
            Err(error) => {
                tracing::error!(error = %error, "listening for SIGTERM failed");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
}
