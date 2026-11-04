//! Finds a PostgreSQL server for the test suite.
//!
//! Nextest runs every test in its own process, so a per-process container would
//! start hundreds of containers. Instead one container is reused under a name
//! every process agrees on, guarded by a file lock so two processes do not try
//! to create it at the same time. `FLAKERADAR_TEST_DATABASE_URL` skips Docker
//! entirely, which is how a build with only a plain PostgreSQL server runs.

// Rust guideline compliant 2026-07-21

use std::fs::File;
use std::process::Command;
use std::time::{Duration, Instant};

use crate::error::TestError;

/// Name of the container every test process reuses.
const CONTAINER: &str = "flakeradar-test-postgres";

/// Container image, matching the PostgreSQL the server supports.
const IMAGE: &str = "postgres:17-alpine";

/// Role, password and database the reused container is created with.
const USER: &str = "flakeradar";
/// Password for [`USER`]; the container listens on loopback only.
const PASSWORD: &str = "flakeradar";
/// Database [`USER`] owns; tests create their own databases beside it.
const DATABASE: &str = "flakeradar";

/// How long a newly started container may take to accept connections.
///
/// PostgreSQL initializes its data directory on first start, which can take
/// several seconds on a cold cache; the CI service container gets 50 seconds and
/// is normally ready well inside it.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(120);

/// How long to wait between readiness probes.
const READY_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The PostgreSQL server the test suite uses, as a connection URL.
///
/// `FLAKERADAR_TEST_DATABASE_URL` wins when it is set and not blank, so no
/// Docker is needed. Otherwise this returns the URL of one reused container,
/// starting it on first use.
///
/// # Errors
///
/// Returns [`TestError::Docker`] when Docker is missing or the container cannot
/// be started, and when it is still not accepting connections after
/// [`STARTUP_TIMEOUT`].
pub fn ensure_postgres() -> Result<String, TestError> {
    if let Ok(url) = std::env::var("FLAKERADAR_TEST_DATABASE_URL")
        && !url.trim().is_empty()
    {
        return Ok(url);
    }
    let _lock = lock()?;
    let port = ensure_container()?;
    Ok(format!(
        "postgresql://{USER}:{PASSWORD}@127.0.0.1:{port}/{DATABASE}"
    ))
}

/// Rewrites `url` so it names the `database` database on the same server.
///
/// Query parameters are kept, because a hosted server may need them.
///
/// # Errors
///
/// Returns [`TestError::Fixture`] when `url` has no scheme.
pub(crate) fn with_database(url: &str, database: &str) -> Result<String, TestError> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| TestError::Fixture(format!("{url} is not a URL")))?;
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    let query = tail
        .split_once('?')
        .map_or_else(String::new, |(_, query)| format!("?{query}"));
    Ok(format!("{scheme}://{authority}/{database}{query}"))
}

/// Takes the cross-process lock that guards container creation.
fn lock() -> Result<File, TestError> {
    let path = std::env::temp_dir().join("flakeradar-test-postgres.lock");
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|error| TestError::Docker(format!("cannot open {}: {error}", path.display())))?;
    file.lock()
        .map_err(|error| TestError::Docker(format!("cannot lock {}: {error}", path.display())))?;
    Ok(file)
}

/// Ensures the reused container runs, and returns its host port.
fn ensure_container() -> Result<u16, TestError> {
    match container_status() {
        Some(status) if status == "running" => {}
        Some(_) => {
            run_docker(&["start", CONTAINER])?;
        }
        None => {
            run_docker(&[
                "run",
                "--detach",
                "--name",
                CONTAINER,
                "--env",
                &format!("POSTGRES_USER={USER}"),
                "--env",
                &format!("POSTGRES_PASSWORD={PASSWORD}"),
                "--env",
                &format!("POSTGRES_DB={DATABASE}"),
                "--publish",
                "127.0.0.1::5432",
                IMAGE,
            ])?;
        }
    }
    let port = host_port()?;
    wait_until_ready()?;
    Ok(port)
}

/// The container's state, or `None` when Docker has no such container.
fn container_status() -> Option<String> {
    let output = Command::new("docker")
        .args(["inspect", "--format", "{{.State.Status}}", CONTAINER])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// The host port Docker published for the container's 5432.
fn host_port() -> Result<u16, TestError> {
    let output = run_docker(&["port", CONTAINER, "5432/tcp"])?;
    let port = output
        .lines()
        .next()
        .and_then(|line| line.rsplit(':').next())
        .and_then(|port| port.trim().parse().ok())
        .ok_or_else(|| TestError::Docker(format!("no published port in {output:?}")))?;
    Ok(port)
}

/// Probes the container until it accepts connections.
fn wait_until_ready() -> Result<(), TestError> {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    while Instant::now() < deadline {
        let ready = Command::new("docker")
            .args(["exec", CONTAINER, "pg_isready", "--username", USER])
            .output()
            .is_ok_and(|output| output.status.success());
        if ready {
            return Ok(());
        }
        std::thread::sleep(READY_POLL_INTERVAL);
    }
    Err(TestError::Docker(format!(
        "{CONTAINER} did not accept connections within {STARTUP_TIMEOUT:?}"
    )))
}

/// Runs `docker` with `args`, returning its trimmed standard output.
fn run_docker(args: &[&str]) -> Result<String, TestError> {
    let output = Command::new("docker")
        .args(args)
        .output()
        .map_err(|error| {
            TestError::Docker(format!("cannot run docker {}: {error}", args.join(" ")))
        })?;
    if !output.status.success() {
        return Err(TestError::Docker(format!(
            "docker {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(test)]
mod tests {
    //! URL rewriting needs no server, so it is checked here.

    use super::with_database;
    use crate::error::TestError;

    #[test]
    fn replaces_the_database_and_keeps_the_query() {
        assert_eq!(
            with_database(
                "postgresql://flakeradar:flakeradar@localhost:5432/flakeradar?sslmode=disable",
                "postgres"
            )
            .ok(),
            Some(
                "postgresql://flakeradar:flakeradar@localhost:5432/postgres?sslmode=disable"
                    .to_owned()
            )
        );
    }

    #[test]
    fn refuses_a_url_without_a_scheme() {
        let outcome = with_database("localhost:5432/flakeradar", "postgres");
        assert!(
            matches!(&outcome, Err(TestError::Fixture(message)) if message.contains("is not a URL")),
            "{outcome:?}"
        );
    }
}
