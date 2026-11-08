//! The binary's command line and the `healthcheck` probe.
//!
//! Parsing is checked against the actions the plan names, and the probe against
//! a real socket on a loopback port: a 200, another status, a closed port and a
//! wildcard host that must still be reached through loopback.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when the command line or the probe answers something else"
)]

use std::collections::BTreeMap;

use axum::Router;
use axum::http::StatusCode;
use axum::routing::get;
use clap::Parser as _;
use flakeradar::{
    cli::Cli, cli::Command, cli::healthcheck, cli::healthcheck_url, settings::Settings,
};
use tokio::net::TcpListener;

/// Settings built from `pairs`, which are `FLAKERADAR_*` variables.
fn settings(pairs: &[(&str, &str)]) -> Settings {
    let values: BTreeMap<String, String> = pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    Settings::from_map(&values).expect("the test's settings are valid")
}

/// Serves `/api/health` with `status` on a loopback port the kernel picks.
///
/// Returns the port and the task, which the caller aborts once the probe has
/// answered.
async fn serving(status: StatusCode) -> (u16, tokio::task::JoinHandle<()>) {
    let app = Router::new().route("/api/health", get(move || async move { status }));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the test can bind a loopback port");
    let port = listener
        .local_addr()
        .expect("a bound listener has an address")
        .port();
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (port, task)
}

#[test]
fn an_empty_command_line_serves() {
    let cli = Cli::try_parse_from(["flakeradar"]).expect("no arguments is a valid command line");
    assert_eq!(cli.action(), Command::Serve);
}

#[test]
fn serve_names_the_same_action() {
    let cli = Cli::try_parse_from(["flakeradar", "serve"]).expect("serve is a subcommand");
    assert_eq!(cli.action(), Command::Serve);
}

#[test]
fn the_version_is_the_workspace_version() {
    let error = Cli::try_parse_from(["flakeradar", "--version"])
        .expect_err("--version prints the version instead of building a command");
    assert_eq!(error.kind(), clap::error::ErrorKind::DisplayVersion);
    assert!(
        error.to_string().contains(env!("CARGO_PKG_VERSION")),
        "the printed version names the workspace version: {error}"
    );
}

#[test]
fn help_names_both_subcommands() {
    let error = Cli::try_parse_from(["flakeradar", "--help"])
        .expect_err("--help prints the usage instead of building a command");
    assert_eq!(error.kind(), clap::error::ErrorKind::DisplayHelp);
    let usage = error.to_string();
    for expected in ["serve", "healthcheck"] {
        assert!(
            usage.contains(expected),
            "the usage names {expected}: {usage}"
        );
    }
}

#[test]
fn a_misspelled_subcommand_is_refused() {
    let error = Cli::try_parse_from(["flakeradar", "healthchek"])
        .expect_err("an unknown subcommand is not a command");
    assert_eq!(error.kind(), clap::error::ErrorKind::InvalidSubcommand);
}

#[test]
fn a_wildcard_host_is_probed_through_loopback() {
    let wildcard = settings(&[("FLAKERADAR_HOST", "0.0.0.0"), ("FLAKERADAR_PORT", "9000")]);
    assert_eq!(
        healthcheck_url(&wildcard),
        "http://127.0.0.1:9000/api/health"
    );
    let unset = settings(&[("FLAKERADAR_PORT", "9000")]);
    assert_eq!(healthcheck_url(&unset), "http://127.0.0.1:9000/api/health");
}

#[test]
fn an_ipv6_host_is_bracketed() {
    for host in ["::", "[::]", "::1", "[::1]"] {
        let settings = settings(&[("FLAKERADAR_HOST", host), ("FLAKERADAR_PORT", "9000")]);
        assert_eq!(
            healthcheck_url(&settings),
            "http://[::1]:9000/api/health",
            "the probe reaches {host} through loopback or at its own address"
        );
    }
}

#[test]
fn a_named_host_is_kept() {
    let settings = settings(&[("FLAKERADAR_HOST", "flakeradar.internal")]);
    assert_eq!(
        healthcheck_url(&settings),
        "http://flakeradar.internal:8000/api/health"
    );
}

#[tokio::test]
async fn a_two_hundred_is_success() {
    let (port, task) = serving(StatusCode::OK).await;
    let settings = settings(&[("FLAKERADAR_PORT", &port.to_string())]);
    healthcheck(&settings)
        .await
        .expect("a 200 answers the probe");
    task.abort();
}

#[tokio::test]
async fn another_status_fails_naming_the_url_and_the_status() {
    let (port, task) = serving(StatusCode::SERVICE_UNAVAILABLE).await;
    let settings = settings(&[("FLAKERADAR_PORT", &port.to_string())]);
    let error = healthcheck(&settings)
        .await
        .expect_err("only a 200 answers the probe");
    let message = format!("{error:#}");
    assert!(
        message.contains(&format!("127.0.0.1:{port}/api/health")),
        "the failure names the URL it polled: {message}"
    );
    assert!(
        message.contains("503"),
        "the failure names the status it read: {message}"
    );
    task.abort();
}

#[tokio::test]
async fn a_wildcard_host_reaches_the_serving_process() {
    let (port, task) = serving(StatusCode::OK).await;
    let settings = settings(&[
        ("FLAKERADAR_HOST", "0.0.0.0"),
        ("FLAKERADAR_PORT", &port.to_string()),
    ]);
    healthcheck(&settings)
        .await
        .expect("a probe with a wildcard host connects over loopback");
    task.abort();
}

#[tokio::test]
async fn a_closed_port_fails_naming_the_url() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the test can bind a loopback port");
    let port = listener
        .local_addr()
        .expect("a bound listener has an address")
        .port();
    // Dropping the listener closes the port, so the probe is refused rather than
    // answered.
    drop(listener);
    let settings = settings(&[("FLAKERADAR_PORT", &port.to_string())]);
    let error = healthcheck(&settings)
        .await
        .expect_err("a closed port answers nothing");
    let message = format!("{error:#}");
    assert!(
        message.contains(&format!("127.0.0.1:{port}/api/health")),
        "the failure names the URL it polled: {message}"
    );
}

#[test]
fn the_binarys_error_path_never_names_the_password() {
    // The process output is what an operator and a CI log see, so the password a
    // database URL holds must not reach it when the URL cannot be parsed. A
    // password holding an unescaped `/` is both the case that makes the URL
    // unparseable and the one the redaction used to miss.
    let directory = std::env::temp_dir().join(format!("flakeradar-cli-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("a scratch directory for the run");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_flakeradar"))
        // A `.env` beside the process would configure it as well, so the run
        // starts from a directory that has none.
        .current_dir(&directory)
        .env_clear()
        .env("FLAKERADAR_API_TOKEN", "cli-token")
        .env("FLAKERADAR_PORT", "0")
        .env(
            "FLAKERADAR_DATABASE_URL",
            "postgresql://u:s3cretpw/x@db:5432/flakeradar",
        )
        .output()
        .expect("the binary runs");
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success(), "the run fails: {printed}");
    assert!(
        !printed.contains("s3cretpw"),
        "the password reached the output: {printed}"
    );
    assert!(
        printed.contains("postgresql://u:***@db:5432/flakeradar"),
        "the output names the URL with the password removed: {printed}"
    );
}
