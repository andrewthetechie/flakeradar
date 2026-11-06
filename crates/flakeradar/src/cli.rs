//! The binary's command line.
//!
//! One binary serves the API, and the same binary answers `healthcheck` for a
//! container probe and `--version` for a release check. [`Cli`] is the parsed
//! command line and [`healthcheck`] is the probe itself, kept out of `main` so a
//! test can run it against a listener on a loopback port.

// Rust guideline compliant 2026-07-21

use std::time::Duration;

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use reqwest::StatusCode;

use crate::settings::{DEFAULT_HOST, Settings};

/// How long a `healthcheck` waits for its answer.
///
/// The image's `HEALTHCHECK` allows three seconds per probe, so the client gives
/// up inside that window and the probe fails rather than hanging.
pub const HEALTHCHECK_TIMEOUT: Duration = Duration::from_secs(2);

/// The path a `healthcheck` asks for.
pub const HEALTH_PATH: &str = "/api/health";

/// The address an IPv6 wildcard host is reached by from the same machine.
const LOOPBACK_V6: &str = "::1";

/// What the command line asked the binary to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Subcommand)]
pub enum Command {
    /// Serve the API, the MCP endpoint and the embedded dashboard.
    Serve,
    /// Check that a running server answers `GET /api/health`.
    Healthcheck,
}

/// The parsed command line.
///
/// With no subcommand the binary serves, which is how `docker run` starts it and
/// how the image's entrypoint runs it.
#[derive(Debug, Parser)]
#[command(
    name = "flakeradar",
    version,
    about = "Self-hosted flaky-test and flaky-job detection"
)]
pub struct Cli {
    /// The subcommand to run; serving when none is named.
    #[command(subcommand)]
    pub command: Option<Command>,
}

impl Cli {
    /// The action the command line asks for, serving when it named none.
    #[must_use]
    pub fn action(&self) -> Command {
        self.command.unwrap_or(Command::Serve)
    }
}

/// The URL a healthcheck polls for `settings`' host and port.
///
/// A wildcard or empty host is not an address a client can connect to, so it is
/// reached through loopback: the probe runs inside the same container or on the
/// same machine, which is the only caller.
#[must_use]
pub fn healthcheck_url(settings: &Settings) -> String {
    let host = settings.host.trim();
    let host = match host {
        "" | "0.0.0.0" => DEFAULT_HOST,
        "::" | "[::]" => LOOPBACK_V6,
        other => other
            .strip_prefix('[')
            .and_then(|bare| bare.strip_suffix(']'))
            .unwrap_or(other),
    };
    let authority = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    format!("http://{authority}:{}{HEALTH_PATH}", settings.port)
}

/// Polls `GET /api/health` on the settings' port and refuses anything but a 200.
///
/// The probe reads no database and sends no token: `/api/health` needs neither,
/// so the answer says whether the process is serving rather than whether its
/// database is ready.
///
/// # Errors
///
/// Returns an error naming the URL when the request cannot be made, and one
/// naming the URL and the status when the answer is not a 200.
pub async fn healthcheck(settings: &Settings) -> anyhow::Result<()> {
    let url = healthcheck_url(settings);
    let response = reqwest::Client::new()
        .get(&url)
        .timeout(HEALTHCHECK_TIMEOUT)
        .send()
        .await
        .with_context(|| format!("requesting {url}"))?;
    let status = response.status();
    if status == StatusCode::OK {
        return Ok(());
    }
    anyhow::bail!("{url} answered {status}")
}
