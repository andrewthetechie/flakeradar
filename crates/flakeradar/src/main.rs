//! FlakeRadar entry point.
//!
//! The binary reads its command line and its settings, refuses the shipped
//! default token, and either serves until it is interrupted or answers one
//! `healthcheck`. Everything it runs lives in the `flakeradar` library, so the
//! suite proves the path a deployment takes and only the process edge is left
//! here.

// Rust guideline compliant 2026-07-21

// cargo-llvm-cov sets `coverage_nightly` on nightly; see `lib.rs`.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

use std::process::ExitCode;

use anyhow::Context as _;
use clap::Parser as _;
use flakeradar::{
    cli::Cli, cli::Command, cli::healthcheck, init_tracing, server::Server,
    settings::ALLOW_INSECURE_VARIABLE, settings::Settings, settings::assert_secure_token,
};

/// The allocator musl builds run with.
///
/// musl's own allocator serializes badly under the multi-threaded runtime, and a
/// musl build is what the container image runs. Other targets keep the system
/// allocator.
#[cfg(all(target_os = "linux", target_env = "musl"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

// Process start-up, environment access and signal wiring are outside what a test
// process can run, so the coverage policy excludes them.
#[cfg_attr(coverage_nightly, coverage(off))]
fn main() -> ExitCode {
    init_tracing();
    let cli = Cli::parse();
    let outcome = match cli.action() {
        Command::Serve => serve(),
        Command::Healthcheck => health_check(),
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // The subscriber is installed first, so a failure reaches the log
            // rather than a bare exit status.
            tracing::error!(
                name: "server.exited",
                error = ?error,
                "flakeradar exited with a failure",
            );
            ExitCode::FAILURE
        }
    }
}

/// Reads the settings, gates the token and serves until interrupted.
///
/// The token gate runs before the database is touched, so an instance left on
/// the shipped default never opens a pool.
#[cfg_attr(coverage_nightly, coverage(off))]
fn serve() -> anyhow::Result<()> {
    let settings = Settings::from_env().context("reading the settings")?;
    let allow_insecure = std::env::var(ALLOW_INSECURE_VARIABLE).ok();
    assert_secure_token(&settings, allow_insecure.as_deref())?;
    runtime()?.block_on(async move {
        let server = Server::start(settings).await?;
        server.serve(shutdown_signal()).await
    })
}

/// Reads the settings and polls `/api/health` once.
///
/// It loads the settings the way the server does, so the probe follows the same
/// `FLAKERADAR_PORT`, but it neither checks the token nor touches the database:
/// a container must be able to tell whether its process serves without a working
/// database, and `/api/health` answers before the pool is ready.
#[cfg_attr(coverage_nightly, coverage(off))]
fn health_check() -> anyhow::Result<()> {
    let settings = Settings::from_env().context("reading the settings")?;
    runtime()?.block_on(healthcheck(&settings))
}

/// A multi-threaded runtime with a worker per core, as the server runs.
#[cfg_attr(coverage_nightly, coverage(off))]
fn runtime() -> anyhow::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("starting the async runtime")
}

/// Resolves on Ctrl-C, or on SIGTERM where the platform has it.
///
/// The serve loop stops accepting requests and then stops the worker, so a
/// request in flight finishes. Signal wiring is the one part of the process a
/// test cannot run, so the coverage policy excludes it.
#[cfg_attr(coverage_nightly, coverage(off))]
async fn shutdown_signal() {
    let ctrl_c = async {
        // A failure to listen must not shut the server down at once, so it is
        // logged and the other arm is left to stop the process, the way the
        // SIGTERM arm below handles one.
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(
                name: "server.signal_failed",
                error = %error,
                "listening for Ctrl-C failed",
            );
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                let _ = stream.recv().await;
            }
            Err(error) => {
                tracing::error!(
                    name: "server.signal_failed",
                    error = %error,
                    "listening for SIGTERM failed",
                );
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
