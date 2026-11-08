//! Builds the PostgreSQL connection pool.
//!
//! The URL is accepted in the three spellings FlakeRadar has ever documented,
//! and the pool sets a statement timeout on every connection and pings a
//! connection before handing it out.

// Rust guideline compliant 2026-07-21

use std::time::Duration;

use sea_orm::{ConnectOptions, Database, DatabaseConnection};

use crate::error::DbError;

/// How long a request waits for a pooled connection before answering 500.
///
/// `SQLx` retries a failed connection attempt inside this window, and its default
/// is 30 seconds: measured against a stopped PostgreSQL, a read waited the whole
/// 30 seconds before its 500. Five seconds keeps an outage an error rather than a
/// hang without failing a request that is only waiting its turn behind the pool's
/// own traffic.
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);

/// Seconds a statement may run before PostgreSQL cancels it.
///
/// The settings and this pool take the value from one place, so they cannot
/// disagree. A long
/// statement would otherwise hold a pooled connection until the client gives up,
/// so the server cancels it instead.
const DEFAULT_COMMAND_TIMEOUT_SECONDS: f64 = flakeradar_core::defaults::DB_COMMAND_TIMEOUT_SECONDS;

/// Settings that shape the connection pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DbSettings {
    command_timeout: Duration,
}

impl DbSettings {
    /// Builds settings from the configured command timeout in seconds.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::InvalidTimeout`] when no `Duration` holds the value or
    /// it is zero: `1e30` overflows one, `1e-12` rounds it to nothing, and a
    /// timeout PostgreSQL is never given is not a setting to keep. This is the
    /// same rule [`Duration::try_from_secs_f64`] applies, so the check and the
    /// conversion cannot disagree.
    pub fn new(command_timeout_seconds: f64) -> Result<Self, DbError> {
        let command_timeout = Duration::try_from_secs_f64(command_timeout_seconds)
            .ok()
            .filter(|timeout| !timeout.is_zero())
            .ok_or_else(|| DbError::InvalidTimeout {
                variable: "FLAKERADAR_DB_COMMAND_TIMEOUT_SECONDS",
                value: command_timeout_seconds.to_string(),
            })?;
        Ok(Self { command_timeout })
    }

    /// How long a statement may run before PostgreSQL cancels it.
    #[must_use]
    pub fn command_timeout(self) -> Duration {
        self.command_timeout
    }
}

impl Default for DbSettings {
    fn default() -> Self {
        Self {
            command_timeout: Duration::from_secs_f64(DEFAULT_COMMAND_TIMEOUT_SECONDS),
        }
    }
}

/// Opens a PostgreSQL pool with `url`, using `settings`.
///
/// A `postgresql+asyncpg://` URL is accepted and rewritten for `SQLx`, so a
/// configuration written for FlakeRadar 1 keeps working. Other schemes are
/// refused with the message FlakeRadar 1 gave.
///
/// # Errors
///
/// Returns [`DbError::NotPostgres`] for any URL that is not PostgreSQL, and
/// [`DbError::UnusableUrl`] for one the driver cannot parse, and
/// [`DbError::Database`] when the server cannot be reached.
pub async fn connect(url: &str, settings: DbSettings) -> Result<DatabaseConnection, DbError> {
    let mut options = shared_options(url, settings)?;
    // A connection that died while idle is replaced instead of failing the
    // request that borrowed it, which is what a pooled server needs after a
    // database restart.
    options.test_before_acquire(true);
    Ok(Database::connect(options).await?)
}

/// The pool options both pools share: the URL, the statement timeout and the
/// bound on waiting for a connection.
///
/// # Errors
///
/// Returns [`DbError::NotPostgres`] for a URL that is not PostgreSQL and
/// [`DbError::UnusableUrl`] for one the driver cannot parse. The second check is
/// made here rather than left to the driver because `Database::connect` quotes
/// the whole connection string when it cannot parse it, and that string holds the
/// database password.
fn shared_options(url: &str, settings: DbSettings) -> Result<ConnectOptions, DbError> {
    let url = postgres_url(url)?;
    if url::Url::parse(&url).is_err() {
        return Err(DbError::UnusableUrl { url: redact(&url) });
    }
    Ok(apply_shared(ConnectOptions::new(url), settings))
}

/// The URL with its password replaced, so it can name itself in an error.
///
/// The scheme, the user and as much of the host, port and database as cannot be
/// a password stay readable, because one of them is the typo an operator has to
/// fix. Nothing of the query or the fragment is printed, because a URL that
/// carries its password there carries it nowhere else.
///
/// The userinfo runs to the last `@` in the whole string, so a password holding
/// an unescaped `/` — which is what makes a URL unparseable in the first place —
/// is found like any other, and the host and database after it are printed.
/// Where the text cannot say which part is the password, the part that might be
/// is hidden:
///
/// - With no `@`, `user:password` reads like `host:port`, so everything after
///   the first `:` is hidden.
/// - A `?` or `#` before the last `@` is either inside an unescaped password or
///   the start of a query that holds the `@`. What follows that `@` may then be
///   the tail of a password, so only the part before the first `:`, `/`, `?` or
///   `#` is printed.
fn redact(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return "an unparseable URL".to_owned();
    };
    let Some(at) = rest.rfind('@') else {
        let authority = before_any(rest, &['?', '#']);
        return match authority.split_once(':') {
            Some((host, _port_or_password)) => format!("{scheme}://{host}:***"),
            None => format!("{scheme}://{authority}"),
        };
    };
    let (userinfo, host_and_query) = rest.split_at(at);
    let user = before_any(userinfo, &[':', '/', '?', '#']);
    if userinfo.contains(['?', '#']) {
        return format!("{scheme}://{user}:***@***");
    }
    let host = before_any(host_and_query, &['?', '#']);
    format!("{scheme}://{user}:***{host}")
}

/// The part of `text` before the first of `delimiters`, or all of it.
fn before_any<'a>(text: &'a str, delimiters: &[char]) -> &'a str {
    text.split(delimiters).next().unwrap_or_default()
}

/// Applies the settings both pools share to a new option set.
///
/// `SQLx` logs one line per statement, and sea-orm's default is `INFO`: measured on
/// the seed run of a demo install, that was 1,546 of the 1,605 lines the default
/// `RUST_LOG` printed, which buries every line this server writes. The statements
/// are still logged, at `DEBUG`, so `RUST_LOG=debug` (or `RUST_LOG=sqlx=debug`)
/// brings them back without a rebuild.
fn apply_shared(mut options: ConnectOptions, settings: DbSettings) -> ConnectOptions {
    options.statement_timeout(settings.command_timeout());
    options.acquire_timeout(ACQUIRE_TIMEOUT);
    options.sqlx_logging_level(log::LevelFilter::Debug);
    options
}

/// Opens a pool of exactly one connection, for a session-scoped advisory lock.
///
/// A session lock lives and dies with the connection that took it, so the
/// connection must stay the same one for as long as the lock is held. A pool of
/// one guarantees that, and both pool reapers are turned off: an idle or
/// over-age connection closed behind the lock's back would leave this process
/// believing it still leads while the next process takes the lock. The pool
/// still replaces a connection the server closed when it next hands it out, and
/// that replacement holds no lock, so the holder records which backend took the
/// lock and checks every liveness answer against it; see
/// `flakeradar::worker::AdvisoryLock`.
///
/// # Errors
///
/// Returns [`DbError::NotPostgres`] for any URL that is not PostgreSQL,
/// [`DbError::UnusableUrl`] for one the driver cannot parse, and
/// [`DbError::Database`] when the server cannot be reached.
pub async fn connect_session(
    url: &str,
    settings: DbSettings,
) -> Result<DatabaseConnection, DbError> {
    let mut options = shared_options(url, settings)?;
    options.max_connections(1);
    options.min_connections(1);
    options.test_before_acquire(false);
    options.idle_timeout(None::<Duration>);
    options.max_lifetime(None::<Duration>);
    Ok(Database::connect(options).await?)
}

/// Rewrites the accepted PostgreSQL spellings to the one `SQLx` parses.
fn postgres_url(url: &str) -> Result<String, DbError> {
    if let Some(rest) = url.strip_prefix("postgresql+asyncpg://") {
        return Ok(format!("postgresql://{rest}"));
    }
    if url.starts_with("postgresql://") || url.starts_with("postgres://") {
        return Ok(url.to_owned());
    }
    Err(DbError::NotPostgres)
}

#[cfg(test)]
mod tests {
    //! URL and setting parsing need no database, so they are checked here.

    use sea_orm::ConnectOptions;

    use super::{
        ACQUIRE_TIMEOUT, DbError, DbSettings, apply_shared, postgres_url, redact, shared_options,
    };

    #[test]
    fn a_pool_waits_five_seconds_for_a_connection_and_no_longer() {
        let settings = DbSettings::default();
        let options = apply_shared(
            ConnectOptions::new("postgresql://u:p@h:5432/d".to_owned()),
            settings,
        );
        assert_eq!(options.get_acquire_timeout(), Some(ACQUIRE_TIMEOUT));
        assert_eq!(ACQUIRE_TIMEOUT.as_secs(), 5);
        assert_eq!(
            options.get_statement_timeout(),
            Some(settings.command_timeout())
        );
        // A statement is a `DEBUG` line, not an `INFO` one: turning the logging
        // off would hide it from `RUST_LOG=debug` as well.
        assert!(options.get_sqlx_logging());
        assert_eq!(
            options.get_sqlx_logging_level(),
            log::LevelFilter::Debug,
            "statement logging sits at debug so the default log stays readable"
        );
    }

    #[test]
    fn accepts_every_documented_postgres_url() {
        assert_eq!(
            postgres_url("postgresql://u:p@h:5432/d").ok(),
            Some("postgresql://u:p@h:5432/d".to_owned())
        );
        assert_eq!(
            postgres_url("postgres://u:p@h:5432/d").ok(),
            Some("postgres://u:p@h:5432/d".to_owned())
        );
        assert_eq!(
            postgres_url("postgresql+asyncpg://u:p@h:5432/d").ok(),
            Some("postgresql://u:p@h:5432/d".to_owned())
        );
    }

    #[test]
    fn refuses_other_schemes() {
        assert!(matches!(
            postgres_url("sqlite:///./x.db"),
            Err(DbError::NotPostgres)
        ));
        assert!(matches!(postgres_url(""), Err(DbError::NotPostgres)));
    }

    #[test]
    fn a_database_url_that_cannot_be_parsed_never_names_its_password() {
        // Every shape of a password, and the URL an operator sees instead: the
        // userinfo runs to the last `@` in the whole string, so a password with
        // an unescaped `/` or a Base64 tail is hidden like any other, and no part
        // of the query is printed, because that is where a password sits when the
        // URL holds none.
        for (url, printed) in [
            (
                "postgresql://flakeradar:s3cret-password@db:notaport/flakeradar",
                "postgresql://flakeradar:***@db:notaport/flakeradar",
            ),
            (
                "postgresql://u:s3cretpw/x@db:5432/d",
                "postgresql://u:***@db:5432/d",
            ),
            (
                "postgresql://u:ab+c/s3cretpw==@db:5432/d",
                "postgresql://u:***@db:5432/d",
            ),
            (
                "postgresql://db:notaport/d?password=s3cretpw",
                "postgresql://db:***",
            ),
            (
                "postgresql://u@db:notaport/d?password=s3cretpw",
                "postgresql://u:***@db:notaport/d",
            ),
        ] {
            let error =
                shared_options(url, DbSettings::default()).expect_err("this URL cannot be parsed");
            let text = error.to_string();
            assert!(!text.contains("s3cret"), "{url}: {text}");
            assert!(text.ends_with(printed), "{url}: {text}");
        }
    }

    #[test]
    fn a_redacted_url_keeps_the_typo_and_drops_the_password() {
        // The query is where a password sits when the URL carries none, so no
        // part of it is printed: `?sslmode=require` goes with it.
        assert_eq!(
            redact("postgresql://flakeradar:s3cret@db:5432/flakeradar?sslmode=require"),
            "postgresql://flakeradar:***@db:5432/flakeradar"
        );
        // No userinfo at all, and a URL with no scheme: neither can panic on the
        // way through. With no `@`, a port reads like a password, so it is
        // hidden; a URL with no `:` has nothing to hide.
        assert_eq!(
            redact("postgresql://db:5432/flakeradar"),
            "postgresql://db:***"
        );
        assert_eq!(
            redact("postgresql://d b/flakeradar"),
            "postgresql://d b/flakeradar"
        );
        assert_eq!(redact("not a url"), "an unparseable URL");
    }

    #[test]
    fn a_password_holding_a_query_or_fragment_character_is_still_removed() {
        // The userinfo runs to the last `@` in the whole string, so a `?` or `#`
        // inside the password is not taken for the start of a query; cutting
        // the query first would print the password's tail. Such a URL reads
        // the same as one whose query holds an `@`, so the host is hidden too.
        for (url, printed) in [
            ("postgresql://u:pa#ss@db:5432/d", "postgresql://u:***@***"),
            ("postgresql://u:pa?ss@db:5432/d", "postgresql://u:***@***"),
            ("postgresql://u:s3cret/x?y@db/d", "postgresql://u:***@***"),
            (
                "postgresql://u:pa#ss@db:5432/d?sslmode=require#frag",
                "postgresql://u:***@***",
            ),
        ] {
            assert_eq!(redact(url), printed, "{url}");
        }
    }

    #[test]
    fn a_url_missing_its_at_sign_does_not_print_its_password() {
        // Without the `@host`, `user:password` reads like `host:port`, so what
        // follows the first `:` is hidden in both.
        for (url, printed) in [
            (
                "postgresql://flakeradar:s3cret",
                "postgresql://flakeradar:***",
            ),
            (
                "postgresql://flakeradar:s3cret/flakeradar",
                "postgresql://flakeradar:***",
            ),
            ("postgresql://db:notaport/d", "postgresql://db:***"),
        ] {
            let text = redact(url);
            assert!(!text.contains("s3cret"), "{url}: {text}");
            assert_eq!(text, printed, "{url}");
        }
    }

    #[test]
    fn an_at_sign_in_the_query_does_not_print_the_query() {
        // With no userinfo, the last `@` sits in the query: what precedes it is
        // not a user, and what follows it is the password's tail, not a host.
        for (url, printed) in [
            (
                "postgresql://d b/flakeradar?password=s3cret@x",
                "postgresql://d b:***@***",
            ),
            (
                "postgresql://db:5432/d?password=s3c@ret",
                "postgresql://db:***@***",
            ),
            (
                "postgresql://db#password=s3c@ret",
                "postgresql://db:***@***",
            ),
            (
                "postgresql://u:pw@db/d?password=s3c@ret",
                "postgresql://u:***@***",
            ),
        ] {
            let text = redact(url);
            assert!(!text.contains("s3c"), "{url}: {text}");
            assert!(!text.contains("ret"), "{url}: {text}");
            assert_eq!(text, printed, "{url}");
        }
    }

    #[test]
    fn rejects_a_timeout_postgres_cannot_take() {
        // `1e30` overflows a `Duration` and `1e-12` rounds to nothing, so neither
        // is a timeout PostgreSQL can be given: both are refused before any
        // `Duration` is built from them.
        for value in [0.0, -1.0, f64::NAN, f64::INFINITY, 1e30, 1e-12] {
            assert!(matches!(
                DbSettings::new(value),
                Err(DbError::InvalidTimeout { .. })
            ));
        }
    }

    #[test]
    fn keeps_a_valid_timeout() {
        let timeout = DbSettings::new(1.5)
            .ok()
            .map(|settings| settings.command_timeout().as_secs_f64());
        assert_eq!(timeout, Some(1.5));
    }
}
