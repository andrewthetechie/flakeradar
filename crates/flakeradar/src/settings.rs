//! The server's configuration, read from `FLAKERADAR_*` environment variables.
//!
//! One field per variable, with the same names, defaults and accepted values as
//! the frozen Python server's `config.py`. Values are read once at start-up and
//! then shared immutably, so no handler can observe a half-applied change and no
//! test needs a process-wide environment variable.

// Rust guideline compliant 2026-07-21

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use flakeradar_core::WebhookFormat;
use thiserror::Error;

/// The token a fresh install ships with, which start-up refuses to keep.
///
/// A real deployment must set `FLAKERADAR_API_TOKEN`; the default exists only so
/// a throwaway local demo can start with `FLAKERADAR_ALLOW_INSECURE=1`.
pub const DEFAULT_INSECURE_TOKEN: &str = "changeme";

/// The database a fresh install expects.
pub const DEFAULT_DATABASE_URL: &str =
    "postgresql+asyncpg://flakeradar:flakeradar@localhost:5432/flakeradar";

/// The address the server binds when nothing says otherwise.
///
/// Loopback, because every read API is unauthenticated by design: a self-hosted
/// instance must not be reachable from the network until its operator asks for
/// it. The container image sets `FLAKERADAR_HOST=0.0.0.0` instead.
pub const DEFAULT_HOST: &str = "127.0.0.1";

/// The port the server binds when nothing says otherwise.
pub const DEFAULT_PORT: u16 = 8000;

/// A configuration value that must never reach a log line.
///
/// The API token and the Webhook URLs are credentials (ADR 0009), so they are
/// wrapped rather than stored as plain strings: a derived `Debug` on any struct
/// holding one prints the wrapper's redacted form, not the secret.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wraps `value` as a secret.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The secret itself, for the one place that compares or parses it.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(..)")
    }
}

/// Why the configuration cannot be used.
#[derive(Debug, Error)]
pub enum SettingsError {
    /// A variable holds a value the field cannot take.
    #[error("{}: expected {expected}, got '{value}'", .variable)]
    InvalidValue {
        /// The variable's name, such as `FLAKERADAR_SCORE_WINDOW`.
        variable: String,
        /// The value that was read.
        value: String,
        /// What the field accepts, in words.
        expected: &'static str,
    },
    /// The database URL is not a PostgreSQL URL.
    #[error(
        "FLAKERADAR_DATABASE_URL must be a PostgreSQL URL (postgresql+asyncpg://user:pass@host:5432/db)"
    )]
    NotPostgres,
    /// The `.env` file exists but cannot be read.
    #[error("reading {path}: {source}")]
    EnvFile {
        /// The file that could not be read.
        path: String,
        /// Why it could not be read.
        source: std::io::Error,
    },
    /// The shipped default token is still in use without the opt-in.
    #[error(
        "FLAKERADAR_API_TOKEN is still the default 'changeme'. Set a real token (see .env.example) \
         or set FLAKERADAR_ALLOW_INSECURE=1 for a throwaway local demo."
    )]
    InsecureToken,
}

/// The variable holding one setting.
fn variable(field: &str) -> String {
    format!("FLAKERADAR_{field}")
}

/// The raw `FLAKERADAR_*` values, before they are parsed into [`Settings`].
struct Raw<'a> {
    values: &'a BTreeMap<String, String>,
}

impl Raw<'_> {
    /// The value of `field` exactly as the environment held it, or `None` when
    /// the variable is unset.
    ///
    /// Nothing is trimmed: the frozen server's `str` fields kept every space an
    /// operator typed, so a token or a URL is read back byte for byte.
    fn raw(&self, field: &str) -> Option<&str> {
        self.values.get(&variable(field)).map(String::as_str)
    }

    /// The value of `field` with surrounding whitespace removed.
    ///
    /// Only the fields Python parsed with `int()` or `float()` use this: those
    /// two ignore leading and trailing whitespace, and so did the frozen server.
    fn trimmed(&self, field: &str) -> Option<&str> {
        self.raw(field).map(str::trim)
    }

    /// The value of `field`, or `default` when it is unset.
    fn text(&self, field: &str, default: &str) -> String {
        self.raw(field).unwrap_or(default).to_owned()
    }

    /// The value of `field` parsed as a signed integer.
    fn int(&self, field: &str, default: i64) -> Result<i64, SettingsError> {
        match self.trimmed(field) {
            None => Ok(default),
            Some(text) => text.parse().map_err(|_error| SettingsError::InvalidValue {
                variable: variable(field),
                value: text.to_owned(),
                expected: "a whole number",
            }),
        }
    }

    /// The value of `field` parsed as a number.
    fn float(&self, field: &str, default: f64) -> Result<f64, SettingsError> {
        match self.trimmed(field) {
            None => Ok(default),
            Some(text) => text.parse().map_err(|_error| SettingsError::InvalidValue {
                variable: variable(field),
                value: text.to_owned(),
                expected: "a number",
            }),
        }
    }

    /// The value of `field` parsed as a boolean.
    ///
    /// Accepts `1`/`0`, `true`/`false`, `yes`/`no` and `on`/`off`, in any case,
    /// which is the set Pydantic accepted for the frozen server's `bool` fields.
    /// Pydantic compared those spellings exactly, so `" yes "` is not one of
    /// them and is refused here too.
    fn boolean(&self, field: &str, default: bool) -> Result<bool, SettingsError> {
        match self.raw(field) {
            None => Ok(default),
            Some(text) => match text.to_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => Ok(true),
                "0" | "false" | "no" | "off" => Ok(false),
                _ => Err(SettingsError::InvalidValue {
                    variable: variable(field),
                    value: text.to_owned(),
                    expected: "a boolean (1/0, true/false, yes/no or on/off)",
                }),
            },
        }
    }

    /// The value of `field` parsed as a TCP port.
    fn port(&self, field: &str, default: u16) -> Result<u16, SettingsError> {
        match self.trimmed(field) {
            None => Ok(default),
            Some(text) => text.parse().map_err(|_error| SettingsError::InvalidValue {
                variable: variable(field),
                value: text.to_owned(),
                expected: "a port number between 0 and 65535",
            }),
        }
    }

    /// The value of `field` parsed as a Webhook format.
    ///
    /// The two names are compared exactly, as Pydantic's `Literal` compared
    /// them, so a padded `" slack "` is refused.
    fn webhook_format(&self, field: &str) -> Result<WebhookFormat, SettingsError> {
        match self.text(field, "json").as_str() {
            "json" => Ok(WebhookFormat::Json),
            "slack" => Ok(WebhookFormat::Slack),
            other => Err(SettingsError::InvalidValue {
                variable: variable(field),
                value: other.to_owned(),
                expected: "one of json, slack",
            }),
        }
    }
}

/// Everything the server is configured with.
///
/// Each field is one `FLAKERADAR_*` variable, named after it in upper case. The
/// fields are public because they are read-only configuration: a [`Settings`]
/// value is built once and then shared as `Arc<Settings>`, so nothing can change
/// it under a running request.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// The shared token CI systems send, and MCP clients send as a Bearer token.
    pub api_token: Secret,
    /// The PostgreSQL URL, in any of the spellings [`flakeradar_db::connect`] takes.
    pub database_url: String,
    /// How many recent Executions the Flakiness score weighs.
    pub score_window: i64,
    /// The per-step decay of that weight, so recent flips count for more.
    pub score_decay: f64,
    /// Consecutive default-branch failures at which a Test counts as broken.
    pub broken_streak: i64,
    /// The GitHub token; empty disables the GitHub integration.
    pub github_token: Secret,
    /// The label applied to every auto-filed GitHub issue.
    pub github_issue_label: String,
    /// The Flakiness score a Test must reach before FlakeRadar files an issue.
    pub github_issue_min_score: f64,
    /// The Proven flake count a Test must reach before an issue is filed. `0` skips the gate.
    pub github_issue_min_proven_flakes: i64,
    /// The recent failure count a Test must reach before an issue is filed. `0` skips it.
    pub github_issue_min_failures: i64,
    /// How many linked Tests, and separately Jobs, one maintenance sync checks.
    pub github_sync_batch: i64,
    /// The Clean streak at which FlakeRadar comments once that a fix appears to hold.
    pub github_fixed_streak: i64,
    /// Whether to also close the issue whose fix appears to hold.
    pub github_close_fixed: bool,
    /// The public base URL of this instance, used for deep links. Empty means none.
    pub public_url: String,
    /// The Flakiness score at or above which a Test or Job counts as flaky.
    pub flake_threshold: f64,
    /// The comma-separated Webhook URLs Notifications are posted to.
    pub webhook_urls: Secret,
    /// The body format every Webhook URL receives.
    pub webhook_format: WebhookFormat,
    /// The origins `Access-Control-Allow-Origin` may name, comma separated.
    pub cors_origins: String,
    /// The Report processor's idle poll interval, in seconds.
    pub worker_poll_seconds: f64,
    /// The statement timeout every pooled connection is set to, in seconds.
    pub db_command_timeout_seconds: f64,
    /// How often a Report may be re-processed after a transient failure.
    pub max_report_retries: i64,
    /// How many days processed Reports are kept.
    pub report_retention_days: i64,
    /// How many days failed Reports are kept; `0` keeps them until a human acts.
    pub failed_report_retention_days: i64,
    /// How many days Executions, and Runs left empty, are kept.
    pub execution_retention_days: i64,
    /// How many days Score history rows are kept.
    pub score_history_retention_days: i64,
    /// How often the processor prunes, in seconds.
    pub prune_interval_seconds: f64,
    /// How many rows one prune batch deletes per table.
    pub prune_batch: i64,
    /// The wall-clock budget of one prune run, in seconds.
    pub prune_time_budget_seconds: f64,
    /// The address the server binds.
    pub host: String,
    /// The port the server binds.
    pub port: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            api_token: Secret::new(DEFAULT_INSECURE_TOKEN),
            database_url: DEFAULT_DATABASE_URL.to_owned(),
            score_window: 50,
            score_decay: 0.85,
            broken_streak: 5,
            github_token: Secret::new(""),
            github_issue_label: "flakeradar".to_owned(),
            github_issue_min_score: 0.30,
            github_issue_min_proven_flakes: 0,
            github_issue_min_failures: 0,
            github_sync_batch: 2000,
            github_fixed_streak: 0,
            github_close_fixed: false,
            public_url: String::new(),
            flake_threshold: 0.30,
            webhook_urls: Secret::new(""),
            webhook_format: WebhookFormat::Json,
            cors_origins: "http://localhost:5173".to_owned(),
            worker_poll_seconds: 1.0,
            db_command_timeout_seconds: 30.0,
            max_report_retries: 3,
            report_retention_days: 7,
            failed_report_retention_days: 0,
            execution_retention_days: 90,
            score_history_retention_days: 365,
            prune_interval_seconds: 3600.0,
            prune_batch: 5000,
            prune_time_budget_seconds: 60.0,
            host: DEFAULT_HOST.to_owned(),
            port: DEFAULT_PORT,
        }
    }
}

impl Settings {
    /// The processor's view of these settings, with the system clock.
    ///
    /// The database layer cannot depend on this module, so it takes only the
    /// values the processor reads.
    #[must_use]
    pub fn processing(&self) -> flakeradar_db::ProcessingSettings {
        flakeradar_db::ProcessingSettings {
            score_window: usize::try_from(self.score_window).unwrap_or_default(),
            score_decay: self.score_decay,
            broken_streak: self.broken_streak,
            flake_threshold: self.flake_threshold,
            max_report_retries: i32::try_from(self.max_report_retries).unwrap_or(i32::MAX),
            clock: std::sync::Arc::new(flakeradar_db::SystemClock),
        }
    }

    /// Reads the settings, overlaid on the defaults.
    ///
    /// `values` holds `FLAKERADAR_*` variables only; anything else is ignored,
    /// exactly as the frozen server's `extra="ignore"` did. A variable that is
    /// absent keeps its default, so a test states only what it changes.
    ///
    /// # Errors
    ///
    /// Returns [`SettingsError::InvalidValue`] naming the variable when a value
    /// cannot be parsed, and [`SettingsError::NotPostgres`] when the database
    /// URL is not PostgreSQL.
    pub fn from_map(values: &BTreeMap<String, String>) -> Result<Self, SettingsError> {
        let raw = Raw { values };
        let settings = Self {
            api_token: Secret::new(raw.text("API_TOKEN", DEFAULT_INSECURE_TOKEN)),
            database_url: raw.text("DATABASE_URL", DEFAULT_DATABASE_URL),
            score_window: raw.int("SCORE_WINDOW", 50)?,
            score_decay: raw.float("SCORE_DECAY", 0.85)?,
            broken_streak: raw.int("BROKEN_STREAK", 5)?,
            github_token: Secret::new(raw.text("GITHUB_TOKEN", "")),
            github_issue_label: raw.text("GITHUB_ISSUE_LABEL", "flakeradar"),
            github_issue_min_score: raw.float("GITHUB_ISSUE_MIN_SCORE", 0.30)?,
            github_issue_min_proven_flakes: raw.int("GITHUB_ISSUE_MIN_PROVEN_FLAKES", 0)?,
            github_issue_min_failures: raw.int("GITHUB_ISSUE_MIN_FAILURES", 0)?,
            github_sync_batch: raw.int("GITHUB_SYNC_BATCH", 2000)?,
            github_fixed_streak: raw.int("GITHUB_FIXED_STREAK", 0)?,
            github_close_fixed: raw.boolean("GITHUB_CLOSE_FIXED", false)?,
            public_url: raw.text("PUBLIC_URL", ""),
            flake_threshold: raw.float("FLAKE_THRESHOLD", 0.30)?,
            webhook_urls: Secret::new(raw.text("WEBHOOK_URLS", "")),
            webhook_format: raw.webhook_format("WEBHOOK_FORMAT")?,
            cors_origins: raw.text("CORS_ORIGINS", "http://localhost:5173"),
            worker_poll_seconds: raw.float("WORKER_POLL_SECONDS", 1.0)?,
            db_command_timeout_seconds: raw.float("DB_COMMAND_TIMEOUT_SECONDS", 30.0)?,
            max_report_retries: raw.int("MAX_REPORT_RETRIES", 3)?,
            report_retention_days: raw.int("REPORT_RETENTION_DAYS", 7)?,
            failed_report_retention_days: raw.int("FAILED_REPORT_RETENTION_DAYS", 0)?,
            execution_retention_days: raw.int("EXECUTION_RETENTION_DAYS", 90)?,
            score_history_retention_days: raw.int("SCORE_HISTORY_RETENTION_DAYS", 365)?,
            prune_interval_seconds: raw.float("PRUNE_INTERVAL_SECONDS", 3600.0)?,
            prune_batch: raw.int("PRUNE_BATCH", 5000)?,
            prune_time_budget_seconds: raw.float("PRUNE_TIME_BUDGET_SECONDS", 60.0)?,
            host: raw.text("HOST", DEFAULT_HOST),
            port: raw.port("PORT", DEFAULT_PORT)?,
        };
        settings.check_database_url()?;
        Ok(settings)
    }

    /// Reads `.env` from `path`, with real variables winning.
    ///
    /// The file is only a convenience: a value in `process` overrides it, and a
    /// missing file is not an error. `FLAKERADAR_ALLOW_INSECURE` is not read
    /// here: [`check_api_token`] reads it from the process environment, because
    /// the frozen server's `os.getenv` never saw a value that came from `.env`.
    ///
    /// # Errors
    ///
    /// Returns [`SettingsError::EnvFile`] when the file exists but cannot be read
    /// or parsed, and the same errors [`Settings::from_map`] gives.
    pub fn from_env() -> Result<Self, SettingsError> {
        Self::from_map(&environment_values(Path::new(".env"), std::env::vars())?)
    }

    /// The settings with an explicit token, for a test that needs a secure one.
    #[must_use]
    pub fn with_api_token(mut self, token: &str) -> Self {
        self.api_token = Secret::new(token);
        self
    }

    /// Refuses a non-PostgreSQL database URL.
    ///
    /// # Errors
    ///
    /// Returns [`SettingsError::NotPostgres`] for any other scheme. The three
    /// PostgreSQL spellings are all accepted, because
    /// [`flakeradar_db::connect`] rewrites them.
    fn check_database_url(&self) -> Result<(), SettingsError> {
        for prefix in ["postgresql://", "postgres://", "postgresql+asyncpg://"] {
            if self.database_url.starts_with(prefix) {
                return Ok(());
            }
        }
        Err(SettingsError::NotPostgres)
    }

    /// The retention settings one prune run reads.
    ///
    /// The database layer cannot depend on this module, so it takes only the
    /// values retention reads.
    #[must_use]
    pub fn retention(&self) -> flakeradar_db::RetentionSettings {
        flakeradar_db::RetentionSettings {
            report_days: self.report_retention_days,
            failed_report_days: self.failed_report_retention_days,
            execution_days: self.execution_retention_days,
            history_days: self.score_history_retention_days,
            batch: self.prune_batch,
            time_budget_seconds: self.prune_time_budget_seconds,
        }
    }

    /// The intervals the Report worker runs on.
    #[must_use]
    pub fn worker(&self) -> crate::worker::WorkerSettings {
        crate::worker::WorkerSettings::from_settings(self)
    }

    /// The database settings the connection pool is built with.
    ///
    /// # Errors
    ///
    /// Returns [`flakeradar_db::DbError::InvalidTimeout`] when the configured
    /// command timeout is not a positive number of seconds.
    pub fn database_settings(&self) -> Result<flakeradar_db::DbSettings, flakeradar_db::DbError> {
        flakeradar_db::DbSettings::new(self.db_command_timeout_seconds)
    }

    /// The CORS origins, split on commas exactly as the frozen server split them.
    ///
    /// A space after a comma stays part of the origin, so an operator who typed
    /// one configured an origin no browser sends, as they did in Python. Blank
    /// entries are dropped because no `Origin` header is empty.
    #[must_use]
    pub fn cors_origin_list(&self) -> Vec<String> {
        self.cors_origins
            .split(',')
            .filter(|origin| !origin.is_empty())
            .map(str::to_owned)
            .collect()
    }
}

/// The variable that allows the shipped default token for a throwaway demo.
///
/// It is not a [`Settings`] field, because it can only lift the default-token
/// refusal and never configures the server. The frozen server read it with
/// `os.getenv`, so a value in `.env` never lifted it there and does not here.
pub const ALLOW_INSECURE_VARIABLE: &str = "FLAKERADAR_ALLOW_INSECURE";

/// Refuses the shipped default token unless the operator opted in.
///
/// `allow_insecure` is the raw value of [`ALLOW_INSECURE_VARIABLE`], or `None`
/// when the variable is unset. When it opts in, a warning names the default
/// token, because the running instance is open to anyone who reaches it.
///
/// # Errors
///
/// Returns [`SettingsError::InsecureToken`] when `token` is the shipped default
/// and the opt-in is not `1`.
pub fn check_api_token(token: &str, allow_insecure: Option<&str>) -> Result<(), SettingsError> {
    if token != DEFAULT_INSECURE_TOKEN {
        return Ok(());
    }
    if allow_insecure.map(str::trim) == Some("1") {
        let message = "FLAKERADAR_API_TOKEN is the default 'changeme' — set a real token.";
        tracing::warn!(token.is_default = true, "{message}");
        return Ok(());
    }
    Err(SettingsError::InsecureToken)
}

/// Whether a [`Settings`] token is the shipped default.
#[must_use]
pub fn is_default_token(token: &str) -> bool {
    token == DEFAULT_INSECURE_TOKEN
}

/// Refuses to serve with the shipped default token, unless the operator opted in.
///
/// This is the start-up gate the binary runs before it touches the database, so
/// an instance left on `changeme` never answers a request.
///
/// # Errors
///
/// Returns [`SettingsError::InsecureToken`] with the message the frozen server
/// printed, when the token is the default and `allow_insecure` is not `1`.
pub fn assert_secure_token(
    settings: &Settings,
    allow_insecure: Option<&str>,
) -> Result<(), SettingsError> {
    check_api_token(settings.api_token.expose(), allow_insecure)
}

/// The variables from the `.env` file at `path` overlaid with `process`.
///
/// `process` wins, so a deployment's real environment always beats a developer's
/// file. Every variable is collected and unknown ones are ignored by
/// [`Settings::from_map`].
///
/// # Errors
///
/// Returns [`SettingsError::EnvFile`] when the file exists but cannot be read or
/// parsed. A missing file contributes nothing.
fn environment_values(
    path: &Path,
    process: impl Iterator<Item = (String, String)>,
) -> Result<BTreeMap<String, String>, SettingsError> {
    let mut values = BTreeMap::new();
    if path.is_file() {
        let entries = dotenvy::from_path_iter(path).map_err(|source| SettingsError::EnvFile {
            path: path.display().to_string(),
            source: std::io::Error::other(source.to_string()),
        })?;
        for entry in entries {
            let (key, value) = entry.map_err(|source| SettingsError::EnvFile {
                path: path.display().to_string(),
                source: std::io::Error::other(source.to_string()),
            })?;
            let _ = values.insert(key, value);
        }
    }
    values.extend(process);
    Ok(values)
}

#[cfg(test)]
mod tests {
    //! Settings parsing needs no server, so it is checked here.

    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use super::{
        DEFAULT_DATABASE_URL, DEFAULT_HOST, DEFAULT_INSECURE_TOKEN, DEFAULT_PORT, Secret, Settings,
        SettingsError, assert_secure_token, check_api_token, environment_values, is_default_token,
    };

    /// A settings map holding `pairs`.
    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    /// The log lines `body` writes, captured from a subscriber installed for it.
    fn captured_log(body: impl FnOnce()) -> String {
        /// An `io::Write` that appends to the shared buffer.
        struct Sink(Arc<Mutex<Vec<u8>>>);

        impl std::io::Write for Sink {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .extend_from_slice(buf);
                Ok(buf.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&lines);
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || Sink(Arc::clone(&sink)))
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);
        body();
        drop(guard);
        let buffer = lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        String::from_utf8_lossy(&buffer).into_owned()
    }

    #[test]
    fn every_setting_defaults_to_the_frozen_servers_value() {
        let settings = Settings::default();
        assert_eq!(settings.api_token.expose(), DEFAULT_INSECURE_TOKEN);
        assert_eq!(settings.database_url, DEFAULT_DATABASE_URL);
        assert_eq!(settings.score_window, 50);
        assert_eq!(settings.score_decay, 0.85);
        assert_eq!(settings.broken_streak, 5);
        assert_eq!(settings.github_token.expose(), "");
        assert_eq!(settings.github_issue_label, "flakeradar");
        assert_eq!(settings.github_issue_min_score, 0.30);
        assert_eq!(settings.github_issue_min_proven_flakes, 0);
        assert_eq!(settings.github_issue_min_failures, 0);
        assert_eq!(settings.github_sync_batch, 2000);
        assert_eq!(settings.github_fixed_streak, 0);
        assert!(!settings.github_close_fixed);
        assert_eq!(settings.public_url, "");
        assert_eq!(settings.flake_threshold, 0.30);
        assert_eq!(settings.webhook_urls.expose(), "");
        assert_eq!(
            settings.webhook_format,
            flakeradar_core::WebhookFormat::Json
        );
        assert_eq!(settings.cors_origins, "http://localhost:5173");
        assert_eq!(settings.worker_poll_seconds, 1.0);
        assert_eq!(settings.db_command_timeout_seconds, 30.0);
        assert_eq!(settings.max_report_retries, 3);
        assert_eq!(settings.report_retention_days, 7);
        assert_eq!(settings.failed_report_retention_days, 0);
        assert_eq!(settings.execution_retention_days, 90);
        assert_eq!(settings.score_history_retention_days, 365);
        assert_eq!(settings.prune_interval_seconds, 3600.0);
        assert_eq!(settings.prune_batch, 5000);
        assert_eq!(settings.prune_time_budget_seconds, 60.0);
        assert_eq!(settings.host, DEFAULT_HOST);
        assert_eq!(settings.port, DEFAULT_PORT);
        assert_eq!(Settings::from_map(&BTreeMap::new()).ok(), Some(settings));
    }

    #[test]
    fn every_setting_can_be_set_from_its_variable() {
        let values = map(&[
            ("FLAKERADAR_API_TOKEN", " token "),
            ("FLAKERADAR_DATABASE_URL", "postgres://u@h/d"),
            ("FLAKERADAR_SCORE_WINDOW", "7"),
            ("FLAKERADAR_SCORE_DECAY", "0.5"),
            ("FLAKERADAR_BROKEN_STREAK", "2"),
            ("FLAKERADAR_GITHUB_TOKEN", "gh"),
            ("FLAKERADAR_GITHUB_ISSUE_LABEL", "flaky"),
            ("FLAKERADAR_GITHUB_ISSUE_MIN_SCORE", "0.4"),
            ("FLAKERADAR_GITHUB_ISSUE_MIN_PROVEN_FLAKES", "3"),
            ("FLAKERADAR_GITHUB_ISSUE_MIN_FAILURES", "4"),
            ("FLAKERADAR_GITHUB_SYNC_BATCH", "10"),
            ("FLAKERADAR_GITHUB_FIXED_STREAK", "11"),
            ("FLAKERADAR_GITHUB_CLOSE_FIXED", "true"),
            ("FLAKERADAR_PUBLIC_URL", "https://flake.example.com"),
            ("FLAKERADAR_FLAKE_THRESHOLD", "0.5"),
            ("FLAKERADAR_WEBHOOK_URLS", "https://hooks.example.com/a"),
            ("FLAKERADAR_WEBHOOK_FORMAT", "slack"),
            (
                "FLAKERADAR_CORS_ORIGINS",
                "https://a.example.com, https://b.example.com",
            ),
            ("FLAKERADAR_WORKER_POLL_SECONDS", "0.05"),
            ("FLAKERADAR_DB_COMMAND_TIMEOUT_SECONDS", "5"),
            ("FLAKERADAR_MAX_REPORT_RETRIES", "9"),
            ("FLAKERADAR_REPORT_RETENTION_DAYS", "1"),
            ("FLAKERADAR_FAILED_REPORT_RETENTION_DAYS", "2"),
            ("FLAKERADAR_EXECUTION_RETENTION_DAYS", "3"),
            ("FLAKERADAR_SCORE_HISTORY_RETENTION_DAYS", "4"),
            ("FLAKERADAR_PRUNE_INTERVAL_SECONDS", "6"),
            ("FLAKERADAR_PRUNE_BATCH", "7"),
            ("FLAKERADAR_PRUNE_TIME_BUDGET_SECONDS", "8"),
            ("FLAKERADAR_HOST", "0.0.0.0"),
            ("FLAKERADAR_PORT", "9000"),
            ("SOME_OTHER_VARIABLE", "ignored"),
        ]);
        let settings = Settings::from_map(&values).expect("the fixture is valid");
        // A `str` field keeps the spaces the environment held, as Pydantic kept
        // them: the token is a credential, so nothing rewrites it.
        assert_eq!(settings.api_token.expose(), " token ");
        assert_eq!(settings.database_url, "postgres://u@h/d");
        assert_eq!(settings.score_window, 7);
        assert_eq!(settings.score_decay, 0.5);
        assert_eq!(settings.broken_streak, 2);
        assert_eq!(settings.github_token.expose(), "gh");
        assert_eq!(settings.github_issue_label, "flaky");
        assert_eq!(settings.github_issue_min_score, 0.4);
        assert_eq!(settings.github_issue_min_proven_flakes, 3);
        assert_eq!(settings.github_issue_min_failures, 4);
        assert_eq!(settings.github_sync_batch, 10);
        assert_eq!(settings.github_fixed_streak, 11);
        assert!(settings.github_close_fixed);
        assert_eq!(settings.public_url, "https://flake.example.com");
        assert_eq!(settings.flake_threshold, 0.5);
        assert_eq!(
            settings.webhook_urls.expose(),
            "https://hooks.example.com/a"
        );
        assert_eq!(
            settings.webhook_format,
            flakeradar_core::WebhookFormat::Slack
        );
        // The space after the comma is part of the second origin, as it was in
        // Python: splitting is all the frozen server did.
        assert_eq!(
            settings.cors_origin_list(),
            ["https://a.example.com", " https://b.example.com"]
        );
        assert_eq!(settings.worker_poll_seconds, 0.05);
        assert_eq!(settings.db_command_timeout_seconds, 5.0);
        assert_eq!(settings.max_report_retries, 9);
        assert_eq!(settings.report_retention_days, 1);
        assert_eq!(settings.failed_report_retention_days, 2);
        assert_eq!(settings.execution_retention_days, 3);
        assert_eq!(settings.score_history_retention_days, 4);
        assert_eq!(settings.prune_interval_seconds, 6.0);
        assert_eq!(settings.prune_batch, 7);
        assert_eq!(settings.prune_time_budget_seconds, 8.0);
        assert_eq!(settings.host, "0.0.0.0");
        assert_eq!(settings.port, 9000);
    }

    #[test]
    fn a_boolean_takes_every_spelling_it_documents() {
        for text in ["1", "true", "TRUE", "yes", "On"] {
            let settings = Settings::from_map(&map(&[("FLAKERADAR_GITHUB_CLOSE_FIXED", text)]))
                .expect("the value is a boolean");
            assert!(settings.github_close_fixed, "{text}");
        }
        for text in ["0", "false", "No", "OFF"] {
            let settings = Settings::from_map(&map(&[("FLAKERADAR_GITHUB_CLOSE_FIXED", text)]))
                .expect("the value is a boolean");
            assert!(!settings.github_close_fixed, "{text}");
        }
    }

    #[test]
    fn a_padded_boolean_is_refused_as_pydantic_refused_it() {
        // Pydantic compared the spelling exactly, so `FLAKERADAR_GITHUB_CLOSE_FIXED=" true "`
        // was an error rather than true, and the server would not start.
        for text in [" yes ", "true ", " on"] {
            let error = Settings::from_map(&map(&[("FLAKERADAR_GITHUB_CLOSE_FIXED", text)]))
                .expect_err("a padded spelling is not a boolean");
            assert!(error.to_string().contains("FLAKERADAR_GITHUB_CLOSE_FIXED"));
        }
    }

    #[test]
    fn a_number_ignores_surrounding_space_as_pythons_parsers_did() {
        let settings = Settings::from_map(&map(&[
            ("FLAKERADAR_SCORE_WINDOW", " 7 "),
            ("FLAKERADAR_SCORE_DECAY", "0.5\t"),
            ("FLAKERADAR_PORT", " 9000\n"),
        ]))
        .expect("Python's int() and float() ignore surrounding whitespace");
        assert_eq!(settings.score_window, 7);
        assert_eq!(settings.score_decay, 0.5);
        assert_eq!(settings.port, 9000);
    }

    #[test]
    fn a_webhook_format_must_be_one_of_the_two_names_exactly() {
        let error = Settings::from_map(&map(&[("FLAKERADAR_WEBHOOK_FORMAT", " json ")]))
            .expect_err("Pydantic's Literal compared the name exactly");
        assert!(error.to_string().contains("FLAKERADAR_WEBHOOK_FORMAT"));
    }

    #[test]
    fn a_bad_value_names_the_variable_it_came_from() {
        let cases: &[(&str, &str)] = &[
            ("FLAKERADAR_SCORE_WINDOW", "many"),
            ("FLAKERADAR_SCORE_DECAY", "high"),
            ("FLAKERADAR_GITHUB_CLOSE_FIXED", "perhaps"),
            ("FLAKERADAR_PORT", "65536"),
            ("FLAKERADAR_PORT", "eighty"),
            ("FLAKERADAR_WEBHOOK_FORMAT", "xml"),
            ("FLAKERADAR_PRUNE_BATCH", "1.5"),
        ];
        for (variable, value) in cases {
            let error =
                Settings::from_map(&map(&[(variable, value)])).expect_err("the value is invalid");
            match &error {
                SettingsError::InvalidValue {
                    variable: named, ..
                } => assert_eq!(named, variable),
                other => panic!("unexpected error for {variable}: {other}"),
            }
            assert!(error.to_string().contains(variable), "{error}");
            assert!(error.to_string().contains(value), "{error}");
        }
    }

    #[test]
    fn an_empty_value_is_not_a_default() {
        let error = Settings::from_map(&map(&[("FLAKERADAR_SCORE_WINDOW", "")]))
            .expect_err("an empty integer is invalid");
        assert!(matches!(error, SettingsError::InvalidValue { .. }));
    }

    #[test]
    fn a_non_postgres_url_is_refused_with_pythons_text() {
        let error = Settings::from_map(&map(&[("FLAKERADAR_DATABASE_URL", "sqlite:///x.db")]))
            .expect_err("sqlite is not PostgreSQL");
        assert_eq!(
            error.to_string(),
            "FLAKERADAR_DATABASE_URL must be a PostgreSQL URL \
             (postgresql+asyncpg://user:pass@host:5432/db)"
        );
        for url in [
            "postgresql://u:p@h:5432/d",
            "postgres://u:p@h:5432/d",
            "postgresql+asyncpg://u:p@h:5432/d",
        ] {
            let settings = Settings::from_map(&map(&[("FLAKERADAR_DATABASE_URL", url)]))
                .expect("a PostgreSQL URL is accepted");
            assert_eq!(settings.database_url, url);
        }
    }

    #[test]
    fn a_secret_never_appears_in_a_debug_line() {
        let settings = Settings::from_map(&map(&[
            ("FLAKERADAR_API_TOKEN", "super-secret-token"),
            ("FLAKERADAR_GITHUB_TOKEN", "ghp_secret"),
            (
                "FLAKERADAR_WEBHOOK_URLS",
                "https://hooks.example.com/T000/B000/xyz",
            ),
        ]))
        .expect("the fixture is valid");
        let printed = format!("{settings:?}");
        assert!(!printed.contains("super-secret-token"), "{printed}");
        assert!(!printed.contains("ghp_secret"), "{printed}");
        assert!(!printed.contains("hooks.example.com"), "{printed}");
        assert!(printed.contains("Secret(..)"), "{printed}");
        // The wrapper redacts whatever it holds, including the empty default.
        assert_eq!(format!("{:?}", Secret::new("s3cret")), "Secret(..)");
    }

    #[test]
    fn cors_origins_split_on_commas_without_trimming() {
        let settings = Settings::from_map(&map(&[(
            "FLAKERADAR_CORS_ORIGINS",
            "http://localhost:5173,, https://a.example.com ,",
        )]))
        .expect("the fixture is valid");
        // Python split the value and kept every space, so an origin written with
        // one never matched the header a browser sends.
        assert_eq!(
            settings.cors_origin_list(),
            ["http://localhost:5173", " https://a.example.com "]
        );
        assert_eq!(
            Settings::default().cors_origin_list(),
            ["http://localhost:5173"]
        );
    }

    #[test]
    fn a_real_token_needs_no_opt_in() {
        check_api_token("a-real-token", None).expect("a real token is allowed");
        assert!(!is_default_token("a-real-token"));
        assert!(is_default_token(DEFAULT_INSECURE_TOKEN));
    }

    #[test]
    fn the_default_token_is_allowed_when_explicitly_opted_in() {
        let logged = captured_log(|| {
            check_api_token(DEFAULT_INSECURE_TOKEN, Some(" 1 "))
                .expect("the opt-in allows the default");
        });
        // The frozen server warned that the instance is open to anyone who
        // reaches it, which is the only notice an operator gets.
        assert!(
            logged.contains("FLAKERADAR_API_TOKEN is the default 'changeme'"),
            "the opt-in warns about the default token: {logged}"
        );
    }

    #[test]
    fn the_default_token_is_refused_when_opted_out() {
        for allow in [Some("0"), Some(""), Some("true"), None] {
            let error = check_api_token(DEFAULT_INSECURE_TOKEN, allow)
                .expect_err("the default token is refused");
            assert!(
                error
                    .to_string()
                    .starts_with("FLAKERADAR_API_TOKEN is still the default")
            );
        }
    }

    #[test]
    fn the_start_up_gate_reads_the_settings_token() {
        let secure = Settings::default().with_api_token("a-real-token");
        assert_secure_token(&secure, None).expect("a real token is allowed");

        let shipped = Settings::default();
        assert!(is_default_token(shipped.api_token.expose()));
        let error = assert_secure_token(&shipped, None).expect_err("the default token is refused");
        assert_eq!(
            error.to_string(),
            "FLAKERADAR_API_TOKEN is still the default 'changeme'. Set a real token (see \
             .env.example) or set FLAKERADAR_ALLOW_INSECURE=1 for a throwaway local demo."
        );
        assert_secure_token(&shipped, Some("1")).expect("the opt-in allows the default");
    }

    #[test]
    fn a_command_timeout_of_zero_is_refused() {
        let settings = Settings::from_map(&map(&[("FLAKERADAR_DB_COMMAND_TIMEOUT_SECONDS", "0")]))
            .expect("the fixture is valid");
        settings
            .database_settings()
            .expect_err("a zero timeout is refused");
        assert_eq!(
            Settings::default()
                .database_settings()
                .ok()
                .map(flakeradar_db::DbSettings::command_timeout),
            Some(std::time::Duration::from_secs(30))
        );
    }

    /// The path of a `.env` fixture in the temporary directory.
    fn dotenv_fixture(name: &str, contents: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("flakeradar-env-{}-{name}", std::process::id()));
        std::fs::write(&path, contents).expect("the fixture is written");
        path
    }

    #[test]
    fn dotenv_values_are_read_and_the_environment_wins() {
        let path = dotenv_fixture("merge", "FLAKERADAR_SCORE_WINDOW=7\nFLAKERADAR_PORT=9000\n");
        let process = vec![("FLAKERADAR_SCORE_WINDOW".to_owned(), "9".to_owned())];
        let values = environment_values(&path, process.into_iter()).expect("the fixture parses");
        let settings = Settings::from_map(&values).expect("the fixture is valid");
        assert_eq!(settings.score_window, 9);
        assert_eq!(settings.port, 9000);
        std::fs::remove_file(&path).expect("the fixture is removed");
    }

    #[test]
    fn a_missing_dotenv_file_is_not_an_error() {
        let path = std::env::temp_dir().join("flakeradar-env-absent");
        let values = environment_values(&path, std::iter::empty()).expect("a missing file is fine");
        assert!(values.is_empty());
    }

    #[test]
    fn a_malformed_dotenv_file_names_itself() {
        let path = dotenv_fixture("malformed", "this line has no separator\n");
        let error =
            environment_values(&path, std::iter::empty()).expect_err("a malformed line is refused");
        assert!(error.to_string().contains("flakeradar-env-"), "{error}");
        std::fs::remove_file(&path).expect("the fixture is removed");
    }
}
