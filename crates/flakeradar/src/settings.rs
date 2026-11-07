//! The server's configuration, read from `FLAKERADAR_*` environment variables.
//!
//! One field per variable, each with the name, default and accepted values this
//! API documents in `.env.example`. Values are read once at start-up and then
//! shared immutably, so no handler can observe a half-applied change and no test
//! needs a process-wide environment variable.

// Rust guideline compliant 2026-07-21

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::time::Duration;

use flakeradar_core::defaults;
use flakeradar_core::dto::lax_bool;
use flakeradar_core::lax::text_int;
use flakeradar_core::notify::WebhookFormat;
use serde_json::Value;
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
/// The API token, the GitHub token, the Webhook URLs (ADR 0009) and the database
/// URL's password are credentials, so they are wrapped rather than stored as
/// plain strings: a derived `Debug` on any struct holding one prints the
/// wrapper's redacted form, not the secret.
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
    /// A variable holds a value that parses but that the field could not mean.
    ///
    /// Taking every value the type admits is what a NaN decay did: it scored every
    /// Test flaky instead of refusing to start. Only non-secret fields are checked
    /// against a range, so the value here is never a token.
    #[error("{}: expected {expected}, got '{value}'", .variable)]
    OutOfRange {
        /// The variable's name, such as `FLAKERADAR_SCORE_DECAY`.
        variable: String,
        /// The value that was read.
        value: String,
        /// What the field accepts, in words.
        expected: String,
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
    /// A line of the `.env` file is not a `KEY=value` assignment.
    ///
    /// The line's text is deliberately absent, and so is the key: a `.env` holds
    /// tokens, and this message reaches the log line `main` writes.
    #[error("reading {path}: line {line} is not a FLAKERADAR_* assignment")]
    EnvFileLine {
        /// The file that could not be parsed.
        path: String,
        /// The 1-based number of the line that does not parse.
        line: usize,
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

/// Sets `slot` to `value`, which is `None` when its variable was unset.
fn set_if_some<T>(slot: &mut T, value: Option<T>) {
    if let Some(value) = value {
        *slot = value;
    }
}

/// Refuses a whole-number setting below `lowest`.
fn at_least<T: PartialOrd + fmt::Display>(
    field: &str,
    value: &T,
    lowest: &T,
) -> Result<(), SettingsError> {
    if value >= lowest {
        return Ok(());
    }
    Err(out_of_range(field, value, &format!("{lowest} or more")))
}

/// Refuses a value outside 0 to 1, which no score or weight can be.
fn fraction(field: &str, value: f64) -> Result<(), SettingsError> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        return Ok(());
    }
    Err(out_of_range(field, value, "a number from 0 to 1"))
}

/// What a field holding a wait accepts, in words.
const WAIT_EXPECTED: &str = "a number of seconds more than 0";

/// The wait `seconds` names, when a `Duration` holds it and it is not zero.
///
/// A finite value above 0 is not enough: `1e30` overflows a `Duration` and
/// `1e-12` rounds it to nothing, so either would have become a zero wait at the
/// point of use — a worker querying with no wait, a prune running on every loop.
/// Both are refused here, by name.
fn wait_of(field: &str, seconds: f64) -> Result<Duration, SettingsError> {
    Duration::try_from_secs_f64(seconds)
        .ok()
        .filter(|wait| !wait.is_zero())
        .ok_or_else(|| out_of_range(field, seconds, WAIT_EXPECTED))
}

/// The refusal for a value the field could never take.
fn out_of_range(field: &str, value: impl fmt::Display, expected: &str) -> SettingsError {
    SettingsError::OutOfRange {
        variable: variable(field),
        value: value.to_string(),
        expected: expected.to_owned(),
    }
}

/// The raw `FLAKERADAR_*` values, before they are parsed into [`Settings`].
struct Raw<'a> {
    values: &'a BTreeMap<String, String>,
}

impl Raw<'_> {
    /// The value of `field` exactly as the environment held it, or `None` when
    /// the variable is unset.
    ///
    /// Nothing is trimmed: a `str` field keeps every space an operator typed, so a
    /// token or a URL is read back byte for byte.
    fn raw(&self, field: &str) -> Option<&str> {
        self.values.get(&variable(field)).map(String::as_str)
    }

    /// The value of `field` with surrounding whitespace removed.
    ///
    /// Only the fields that are numbers use this: a number read from text ignores
    /// leading and trailing whitespace, so ` 30 ` and `30` are the same timeout.
    fn trimmed(&self, field: &str) -> Option<&str> {
        self.raw(field).map(str::trim)
    }

    /// The value of `field`, or `None` when it is unset.
    fn text(&self, field: &str) -> Option<String> {
        self.raw(field).map(str::to_owned)
    }

    /// The value of `field` parsed by `T`, or `None` when it is unset.
    fn parsed<T: std::str::FromStr>(
        &self,
        field: &str,
        expected: &'static str,
    ) -> Result<Option<T>, SettingsError> {
        match self.trimmed(field) {
            None => Ok(None),
            Some(text) => text
                .parse()
                .map(Some)
                .map_err(|_error| SettingsError::InvalidValue {
                    variable: variable(field),
                    value: text.to_owned(),
                    expected,
                }),
        }
    }

    /// The value of `field` parsed as a whole number, or `None` when it is unset.
    ///
    /// The spelling is the request body's, read by the same reader: a trailing
    /// zero fraction and underscores between digits are part of a whole number,
    /// so `5.0`, `1_000` and `+5` are 5, 1000 and 5.
    fn whole<T: TryFrom<i64>>(&self, field: &str) -> Result<Option<T>, SettingsError> {
        let Some(text) = self.trimmed(field) else {
            return Ok(None);
        };
        let value = text_int(text).and_then(|value| T::try_from(value).ok());
        value.map(Some).ok_or_else(|| SettingsError::InvalidValue {
            variable: variable(field),
            value: text.to_owned(),
            expected: "a whole number",
        })
    }

    /// The value of `field` parsed as a number, or `None` when it is unset.
    fn float(&self, field: &str) -> Result<Option<f64>, SettingsError> {
        self.parsed(field, "a number")
    }

    /// The value of `field` parsed as a number of seconds one of the waits that
    /// no `Duration` is built from is held as, or `None` when it is unset.
    ///
    /// [`Raw::wait`] is the reader for a field that *is* a `Duration`; this one
    /// keeps the seconds a budget and a statement timeout are spent in, and
    /// applies the same rule so the refusal names the text as well.
    fn wait_seconds(&self, field: &str) -> Result<Option<f64>, SettingsError> {
        let Some(text) = self.trimmed(field) else {
            return Ok(None);
        };
        let seconds: f64 = text.parse().map_err(|_error| SettingsError::InvalidValue {
            variable: variable(field),
            value: text.to_owned(),
            expected: "a number",
        })?;
        wait_of(field, seconds).map_err(|_refusal| SettingsError::OutOfRange {
            variable: variable(field),
            value: text.to_owned(),
            expected: WAIT_EXPECTED.to_owned(),
        })?;
        Ok(Some(seconds))
    }

    /// The value of `field` parsed as a wait, or `None` when it is unset.
    ///
    /// The refusal names the text the operator typed rather than the float it
    /// parsed to, so `1e30` is the value the message quotes.
    fn wait(&self, field: &str) -> Result<Option<Duration>, SettingsError> {
        let Some(text) = self.trimmed(field) else {
            return Ok(None);
        };
        let seconds: f64 = text.parse().map_err(|_error| SettingsError::InvalidValue {
            variable: variable(field),
            value: text.to_owned(),
            expected: "a number",
        })?;
        Duration::try_from_secs_f64(seconds)
            .ok()
            .filter(|wait| !wait.is_zero())
            .map(Some)
            .ok_or_else(|| SettingsError::OutOfRange {
                variable: variable(field),
                value: text.to_owned(),
                expected: WAIT_EXPECTED.to_owned(),
            })
    }

    /// The value of `field` parsed as a TCP port, or `None` when it is unset.
    fn port(&self, field: &str) -> Result<Option<u16>, SettingsError> {
        self.parsed(field, "a port number between 0 and 65535")
    }

    /// The value of `field` parsed as a boolean, or `None` when it is unset.
    ///
    /// The spelling is the request body's, read by the same reader: `1`/`0`,
    /// `true`/`false`, `t`/`f`, `yes`/`no` and `on`/`off`, in any case. The
    /// spelling is compared exactly, so a padded `" yes "` is not one of them and
    /// is refused: a value nobody meant to set must not switch a feature on.
    fn boolean(&self, field: &str) -> Result<Option<bool>, SettingsError> {
        let Some(text) = self.raw(field) else {
            return Ok(None);
        };
        let read = lax_bool::read(&Value::from(text));
        read.map(Some)
            .map_err(|_refusal| SettingsError::InvalidValue {
                variable: variable(field),
                value: text.to_owned(),
                expected: "a boolean (1/0, true/false, t/f, yes/no, y/n or on/off)",
            })
    }

    /// The value of `field` parsed as a Webhook format, or `None` when unset.
    ///
    /// The two names are compared exactly, so a padded `" slack "` is refused.
    fn webhook_format(&self, field: &str) -> Result<Option<WebhookFormat>, SettingsError> {
        match self.raw(field) {
            None => Ok(None),
            Some("json") => Ok(Some(WebhookFormat::Json)),
            Some("slack") => Ok(Some(WebhookFormat::Slack)),
            Some(other) => Err(SettingsError::InvalidValue {
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
    ///
    /// It is a [`Secret`] because it holds the database password.
    pub database_url: Secret,
    /// How many recent Executions the Flakiness score weighs.
    pub score_window: usize,
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
    /// The Report processor's idle poll interval.
    pub worker_poll: Duration,
    /// The statement timeout every pooled connection is set to, in seconds.
    pub db_command_timeout_seconds: f64,
    /// How often a Report may be re-processed after a transient failure.
    pub max_report_retries: i32,
    /// How many days processed Reports are kept.
    pub report_retention_days: i64,
    /// How many days failed Reports are kept; `0` keeps them until a human acts.
    pub failed_report_retention_days: i64,
    /// How many days Executions, and Runs left empty, are kept.
    pub execution_retention_days: i64,
    /// How many days Score history rows are kept.
    pub score_history_retention_days: i64,
    /// How often the processor prunes.
    pub prune_interval: Duration,
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
            database_url: Secret::new(DEFAULT_DATABASE_URL),
            score_window: defaults::SCORE_WINDOW,
            score_decay: defaults::SCORE_DECAY,
            broken_streak: defaults::BROKEN_STREAK,
            github_token: Secret::new(""),
            github_issue_label: "flakeradar".to_owned(),
            github_issue_min_score: 0.30,
            github_issue_min_proven_flakes: 0,
            github_issue_min_failures: 0,
            github_sync_batch: 2000,
            github_fixed_streak: 0,
            github_close_fixed: false,
            public_url: String::new(),
            flake_threshold: defaults::FLAKE_THRESHOLD,
            webhook_urls: Secret::new(""),
            webhook_format: WebhookFormat::Json,
            cors_origins: "http://localhost:5173".to_owned(),
            worker_poll: Duration::from_secs(1),
            db_command_timeout_seconds: defaults::DB_COMMAND_TIMEOUT_SECONDS,
            max_report_retries: defaults::MAX_REPORT_RETRIES,
            report_retention_days: defaults::REPORT_RETENTION_DAYS,
            failed_report_retention_days: defaults::FAILED_REPORT_RETENTION_DAYS,
            execution_retention_days: defaults::EXECUTION_RETENTION_DAYS,
            score_history_retention_days: defaults::SCORE_HISTORY_RETENTION_DAYS,
            prune_interval: Duration::from_secs(3600),
            prune_batch: defaults::PRUNE_BATCH,
            prune_time_budget_seconds: defaults::PRUNE_TIME_BUDGET_SECONDS,
            host: DEFAULT_HOST.to_owned(),
            port: DEFAULT_PORT,
        }
    }
}

impl Settings {
    /// The read layer's view of these settings, on `clock`.
    ///
    /// The caller brings the clock: one process reads every instant through
    /// [`crate::app::AppState`]'s, so a test can put a whole server on a fixed day
    /// instead of only the layer it called directly.
    #[must_use]
    pub fn reads(
        &self,
        clock: flakeradar_db::clock::SharedClock,
    ) -> flakeradar_db::reads::ReadSettings {
        flakeradar_db::reads::ReadSettings {
            flake_threshold: self.flake_threshold,
            broken_streak: self.broken_streak,
            score_window: self.score_window,
            clock,
        }
    }

    /// The processor's view of these settings, on `clock`.
    ///
    /// The database layer cannot depend on this module, so it takes only the
    /// values the processor reads.
    #[must_use]
    pub fn processing(
        &self,
        clock: flakeradar_db::clock::SharedClock,
    ) -> flakeradar_db::processing::ProcessingSettings {
        flakeradar_db::processing::ProcessingSettings {
            score_window: self.score_window,
            score_decay: self.score_decay,
            broken_streak: self.broken_streak,
            flake_threshold: self.flake_threshold,
            max_report_retries: self.max_report_retries,
            clock,
        }
    }

    /// Reads the settings, overlaid on the defaults.
    ///
    /// `values` holds `FLAKERADAR_*` variables only; anything else is ignored, so a
    /// process environment full of unrelated variables configures nothing here. A
    /// variable that is absent keeps its default, so a test states only what it
    /// changes, and every default is stated once: in [`Settings::default`].
    ///
    /// # Errors
    ///
    /// Returns [`SettingsError::InvalidValue`] naming the variable when a value
    /// cannot be parsed, [`SettingsError::OutOfRange`] when it parses but that
    /// field could never take it, and [`SettingsError::NotPostgres`] when the
    /// database URL is not PostgreSQL.
    pub fn from_map(values: &BTreeMap<String, String>) -> Result<Self, SettingsError> {
        let raw = Raw { values };
        let mut settings = Self::default();
        set_if_some(
            &mut settings.api_token,
            raw.text("API_TOKEN").map(Secret::new),
        );
        set_if_some(
            &mut settings.database_url,
            raw.text("DATABASE_URL").map(Secret::new),
        );
        set_if_some(&mut settings.score_window, raw.whole("SCORE_WINDOW")?);
        set_if_some(&mut settings.score_decay, raw.float("SCORE_DECAY")?);
        set_if_some(&mut settings.broken_streak, raw.whole("BROKEN_STREAK")?);
        set_if_some(
            &mut settings.github_token,
            raw.text("GITHUB_TOKEN").map(Secret::new),
        );
        set_if_some(
            &mut settings.github_issue_label,
            raw.text("GITHUB_ISSUE_LABEL"),
        );
        set_if_some(
            &mut settings.github_issue_min_score,
            raw.float("GITHUB_ISSUE_MIN_SCORE")?,
        );
        set_if_some(
            &mut settings.github_issue_min_proven_flakes,
            raw.whole("GITHUB_ISSUE_MIN_PROVEN_FLAKES")?,
        );
        set_if_some(
            &mut settings.github_issue_min_failures,
            raw.whole("GITHUB_ISSUE_MIN_FAILURES")?,
        );
        set_if_some(
            &mut settings.github_sync_batch,
            raw.whole("GITHUB_SYNC_BATCH")?,
        );
        set_if_some(
            &mut settings.github_fixed_streak,
            raw.whole("GITHUB_FIXED_STREAK")?,
        );
        set_if_some(
            &mut settings.github_close_fixed,
            raw.boolean("GITHUB_CLOSE_FIXED")?,
        );
        set_if_some(&mut settings.public_url, raw.text("PUBLIC_URL"));
        set_if_some(&mut settings.flake_threshold, raw.float("FLAKE_THRESHOLD")?);
        set_if_some(
            &mut settings.webhook_urls,
            raw.text("WEBHOOK_URLS").map(Secret::new),
        );
        set_if_some(
            &mut settings.webhook_format,
            raw.webhook_format("WEBHOOK_FORMAT")?,
        );
        set_if_some(&mut settings.cors_origins, raw.text("CORS_ORIGINS"));
        set_if_some(&mut settings.worker_poll, raw.wait("WORKER_POLL_SECONDS")?);
        set_if_some(
            &mut settings.db_command_timeout_seconds,
            raw.wait_seconds("DB_COMMAND_TIMEOUT_SECONDS")?,
        );
        set_if_some(
            &mut settings.max_report_retries,
            raw.whole("MAX_REPORT_RETRIES")?,
        );
        set_if_some(
            &mut settings.report_retention_days,
            raw.whole("REPORT_RETENTION_DAYS")?,
        );
        set_if_some(
            &mut settings.failed_report_retention_days,
            raw.whole("FAILED_REPORT_RETENTION_DAYS")?,
        );
        set_if_some(
            &mut settings.execution_retention_days,
            raw.whole("EXECUTION_RETENTION_DAYS")?,
        );
        set_if_some(
            &mut settings.score_history_retention_days,
            raw.whole("SCORE_HISTORY_RETENTION_DAYS")?,
        );
        set_if_some(
            &mut settings.prune_interval,
            raw.wait("PRUNE_INTERVAL_SECONDS")?,
        );
        set_if_some(&mut settings.prune_batch, raw.whole("PRUNE_BATCH")?);
        set_if_some(
            &mut settings.prune_time_budget_seconds,
            raw.wait_seconds("PRUNE_TIME_BUDGET_SECONDS")?,
        );
        set_if_some(&mut settings.host, raw.text("HOST"));
        set_if_some(&mut settings.port, raw.port("PORT")?);
        settings.validate()?;
        settings.check_database_url()?;
        Ok(settings)
    }

    /// Reads `.env` from `path`, with real variables winning.
    ///
    /// The file is only a convenience: a value in `process` overrides it, and a
    /// missing file is not an error. `FLAKERADAR_ALLOW_INSECURE` is not read
    /// here: [`check_api_token`] reads it from the process environment, so a value
    /// in `.env` can never lift the default-token refusal.
    ///
    /// # Errors
    ///
    /// Returns [`SettingsError::EnvFile`] when the file exists but cannot be read
    /// or parsed, and the same errors [`Settings::from_map`] gives.
    pub fn from_env() -> Result<Self, SettingsError> {
        Self::from_map(&environment_values(Path::new(".env"), std::env::vars())?)
    }

    /// Refuses a value that no configuration of its field could mean.
    ///
    /// The fields' meanings are narrower than their types: a NaN decay scored
    /// every Test 1.0 because `f64::min` answers the other operand, a window of 0
    /// weighed no Execution at all, and a poll of 0 made the worker query with no
    /// wait. Such a value stops the process at start-up, and the error names the
    /// variable rather than the value of a secret.
    ///
    /// # Errors
    ///
    /// Returns [`SettingsError::OutOfRange`] naming the variable and the range
    /// the field accepts.
    fn validate(&self) -> Result<(), SettingsError> {
        at_least("SCORE_WINDOW", &self.score_window, &1)?;
        fraction("SCORE_DECAY", self.score_decay)?;
        at_least("BROKEN_STREAK", &self.broken_streak, &0)?;
        fraction("GITHUB_ISSUE_MIN_SCORE", self.github_issue_min_score)?;
        at_least(
            "GITHUB_ISSUE_MIN_PROVEN_FLAKES",
            &self.github_issue_min_proven_flakes,
            &0,
        )?;
        at_least(
            "GITHUB_ISSUE_MIN_FAILURES",
            &self.github_issue_min_failures,
            &0,
        )?;
        at_least("GITHUB_SYNC_BATCH", &self.github_sync_batch, &0)?;
        at_least("GITHUB_FIXED_STREAK", &self.github_fixed_streak, &0)?;
        fraction("FLAKE_THRESHOLD", self.flake_threshold)?;
        wait_of(
            "DB_COMMAND_TIMEOUT_SECONDS",
            self.db_command_timeout_seconds,
        )?;
        at_least("MAX_REPORT_RETRIES", &self.max_report_retries, &0)?;
        at_least("REPORT_RETENTION_DAYS", &self.report_retention_days, &0)?;
        at_least(
            "FAILED_REPORT_RETENTION_DAYS",
            &self.failed_report_retention_days,
            &0,
        )?;
        at_least(
            "EXECUTION_RETENTION_DAYS",
            &self.execution_retention_days,
            &0,
        )?;
        at_least(
            "SCORE_HISTORY_RETENTION_DAYS",
            &self.score_history_retention_days,
            &0,
        )?;
        at_least("PRUNE_BATCH", &self.prune_batch, &1)?;
        wait_of("PRUNE_TIME_BUDGET_SECONDS", self.prune_time_budget_seconds)?;
        Ok(())
    }

    /// Refuses a non-PostgreSQL database URL.
    ///
    /// # Errors
    ///
    /// Returns [`SettingsError::NotPostgres`] for any other scheme. The three
    /// PostgreSQL spellings are all accepted, because
    /// [`flakeradar_db::connect`] rewrites them. The URL is not repeated in the
    /// error: it holds the database password.
    fn check_database_url(&self) -> Result<(), SettingsError> {
        for prefix in ["postgresql://", "postgres://", "postgresql+asyncpg://"] {
            if self.database_url.expose().starts_with(prefix) {
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
    pub fn retention(&self) -> flakeradar_db::retention::RetentionSettings {
        flakeradar_db::retention::RetentionSettings {
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

    /// The CORS origins, split on commas.
    ///
    /// A space after a comma stays part of the origin, so an operator who typed one
    /// configured an origin no browser sends. Blank entries are dropped because no
    /// `Origin` header is empty.
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
/// refusal and never configures the server. It is read from the process
/// environment rather than from `.env`, so a file in the checkout cannot lift the
/// refusal on its own.
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
        tracing::warn!(
            name: "settings.token_default",
            { token.is_default = true },
            "FLAKERADAR_API_TOKEN is the default 'changeme' — set a real token.",
        );
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
/// Returns [`SettingsError::InsecureToken`] when the token is the default and
/// `allow_insecure` is not `1`.
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
/// Returns [`SettingsError::EnvFile`] when the file exists but cannot be read, and
/// [`SettingsError::EnvFileLine`] when a line of it is not an assignment. A
/// missing file contributes nothing.
fn environment_values(
    path: &Path,
    process: impl Iterator<Item = (String, String)>,
) -> Result<BTreeMap<String, String>, SettingsError> {
    let mut values = BTreeMap::new();
    if path.is_file() {
        values.extend(dotenv_values(path)?);
    }
    values.extend(process);
    Ok(values)
}

/// Every variable the `.env` file at `path` sets.
///
/// A line that does not parse is refused by its number and never by its text:
/// the file holds tokens, and this error is what `main` logs.
fn dotenv_values(path: &Path) -> Result<BTreeMap<String, String>, SettingsError> {
    let file = std::fs::File::open(path).map_err(|source| SettingsError::EnvFile {
        path: path.display().to_string(),
        source,
    })?;
    let mut lines = LineCounted::new(std::io::BufReader::new(file));
    let mut values = BTreeMap::new();
    let mut failure = None;
    for entry in dotenvy::from_read_iter(&mut lines) {
        match entry {
            Ok((key, value)) => {
                let _ = values.insert(key, value);
            }
            Err(error) => {
                failure = Some(error);
                break;
            }
        }
    }
    if let Some(error) = failure {
        return Err(dotenv_error(path, &lines, error));
    }
    Ok(values)
}

/// The settings error a `.env` read failure deserves.
fn dotenv_error(
    path: &Path,
    lines: &LineCounted<impl std::io::BufRead>,
    error: dotenvy::Error,
) -> SettingsError {
    let path = path.display().to_string();
    match error {
        dotenvy::Error::LineParse(text, _index) => SettingsError::EnvFileLine {
            path,
            line: lines.first_line_of(&text),
        },
        other => SettingsError::EnvFile {
            path,
            source: std::io::Error::other(other.to_string()),
        },
    }
}

/// A reader that counts the lines it has handed out.
///
/// `dotenvy` reports a malformed `.env` line by its text and its position within
/// that text, never by its line number, so the number comes from here: the reader
/// hands out one line per `read` call, which keeps `dotenvy`'s buffer from
/// running ahead of the line it is parsing.
struct LineCounted<R> {
    inner: R,
    /// How many line breaks `inner` has handed out.
    breaks: usize,
    /// Whether the last byte handed out was a line break.
    ended_line: bool,
}

impl<R: std::io::BufRead> LineCounted<R> {
    /// Reads `inner` one line at a time.
    fn new(inner: R) -> Self {
        Self {
            inner,
            breaks: 0,
            ended_line: false,
        }
    }

    /// The 1-based number of the line `text` starts on.
    ///
    /// `text` is what `dotenvy` refused: either a fragment of the line it was
    /// parsing, or every line it had read when a quote was left open. Both end at
    /// the last byte read, so the fragment's first line is that byte's line minus
    /// the line breaks inside the fragment.
    fn first_line_of(&self, text: &str) -> usize {
        let current = if self.ended_line {
            self.breaks
        } else {
            self.breaks.saturating_add(1)
        };
        let inside = text.matches('\n').count();
        let first = if text.ends_with('\n') {
            current.saturating_sub(inside).saturating_add(1)
        } else {
            current.saturating_sub(inside)
        };
        first.max(1)
    }
}

impl<R: std::io::BufRead> std::io::Read for LineCounted<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let available = self.inner.fill_buf()?;
        if available.is_empty() || buf.is_empty() {
            return Ok(0);
        }
        // Up to and including the next line break, so a caller's 8 KiB buffer
        // never holds a second line; a line longer than the buffer is split.
        let up_to_break = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index.saturating_add(1));
        let read = up_to_break.min(buf.len());
        for (slot, byte) in buf.iter_mut().zip(available.iter().take(read)) {
            *slot = *byte;
        }
        self.breaks += available
            .iter()
            .take(read)
            .filter(|byte| **byte == b'\n')
            .count();
        self.ended_line = available.iter().take(read).next_back() == Some(&b'\n');
        self.inner.consume(read);
        Ok(read)
    }
}

#[cfg(test)]
mod tests {
    //! Settings parsing needs no server, so it is checked here.

    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use anyhow::Context as _;

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
    fn every_setting_defaults_to_the_documented_value() {
        // The expected values are what `.env.example` and the README state, not a
        // second copy of `Settings::default`: this test is the contract with the
        // deployment, so a literal here is the requirement and not a restatement
        // of the code under it.
        let settings = Settings::default();
        assert_eq!(settings.api_token.expose(), DEFAULT_INSECURE_TOKEN);
        assert_eq!(settings.database_url.expose(), DEFAULT_DATABASE_URL);
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
            flakeradar_core::notify::WebhookFormat::Json
        );
        assert_eq!(settings.cors_origins, "http://localhost:5173");
        assert_eq!(settings.worker_poll, Duration::from_secs(1));
        assert_eq!(settings.db_command_timeout_seconds, 30.0);
        assert_eq!(settings.max_report_retries, 3);
        assert_eq!(settings.report_retention_days, 7);
        assert_eq!(settings.failed_report_retention_days, 0);
        assert_eq!(settings.execution_retention_days, 90);
        assert_eq!(settings.score_history_retention_days, 365);
        assert_eq!(settings.prune_interval, Duration::from_secs(3600));
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
        // A `str` field keeps the spaces the environment held: the token is a
        // credential, so nothing rewrites it.
        assert_eq!(settings.api_token.expose(), " token ");
        assert_eq!(settings.database_url.expose(), "postgres://u@h/d");
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
            flakeradar_core::notify::WebhookFormat::Slack
        );
        // The space after the comma is part of the second origin: splitting is all
        // this reader does.
        assert_eq!(
            settings.cors_origin_list(),
            ["https://a.example.com", " https://b.example.com"]
        );
        assert_eq!(settings.worker_poll, Duration::from_millis(50));
        assert_eq!(settings.db_command_timeout_seconds, 5.0);
        assert_eq!(settings.max_report_retries, 9);
        assert_eq!(settings.report_retention_days, 1);
        assert_eq!(settings.failed_report_retention_days, 2);
        assert_eq!(settings.execution_retention_days, 3);
        assert_eq!(settings.score_history_retention_days, 4);
        assert_eq!(settings.prune_interval, Duration::from_secs(6));
        assert_eq!(settings.prune_batch, 7);
        assert_eq!(settings.prune_time_budget_seconds, 8.0);
        assert_eq!(settings.host, "0.0.0.0");
        assert_eq!(settings.port, 9000);
    }

    #[test]
    fn a_boolean_takes_every_spelling_it_documents() {
        // The short `t`/`f` and `y`/`n` forms are how an operator writes a flag
        // in a shell, so a shell spelling starts the server.
        for text in ["1", "true", "TRUE", "yes", "On", "t", "T", "y", "Y"] {
            let settings = Settings::from_map(&map(&[("FLAKERADAR_GITHUB_CLOSE_FIXED", text)]))
                .expect("the value is a boolean");
            assert!(settings.github_close_fixed, "{text}");
        }
        for text in ["0", "false", "No", "OFF", "f", "F", "n", "N"] {
            let settings = Settings::from_map(&map(&[("FLAKERADAR_GITHUB_CLOSE_FIXED", text)]))
                .expect("the value is a boolean");
            assert!(!settings.github_close_fixed, "{text}");
        }
    }

    #[test]
    fn a_whole_number_takes_every_spelling_the_wire_does() {
        // The spellings a request body takes are the spellings an environment
        // variable takes: a trailing zero fraction and underscores between
        // digits are both part of a whole number.
        for (text, expected) in [
            ("5", 5),
            ("5.0", 5),
            ("5.000", 5),
            ("+5", 5),
            ("05", 5),
            (" 5 ", 5),
            ("1_000", 1000),
        ] {
            let settings = Settings::from_map(&map(&[("FLAKERADAR_SCORE_WINDOW", text)]))
                .expect("the value is a whole number");
            assert_eq!(settings.score_window, expected, "{text}");
        }
        for text in ["0x10", "1.5", "1e3", "", "many", "1__0"] {
            let error = Settings::from_map(&map(&[("FLAKERADAR_SCORE_WINDOW", text)]))
                .expect_err("the value is no whole number");
            assert!(
                error.to_string().contains("FLAKERADAR_SCORE_WINDOW"),
                "{text}"
            );
        }
    }

    #[test]
    fn a_padded_boolean_is_refused() {
        // The spelling is compared exactly, so
        // `FLAKERADAR_GITHUB_CLOSE_FIXED=" true "` is an error rather than true,
        // and the server does not start.
        for text in [" yes ", "true ", " on"] {
            let error = Settings::from_map(&map(&[("FLAKERADAR_GITHUB_CLOSE_FIXED", text)]))
                .expect_err("a padded spelling is not a boolean");
            assert!(error.to_string().contains("FLAKERADAR_GITHUB_CLOSE_FIXED"));
        }
    }

    #[test]
    fn a_number_ignores_surrounding_space() {
        let settings = Settings::from_map(&map(&[
            ("FLAKERADAR_SCORE_WINDOW", " 7 "),
            ("FLAKERADAR_SCORE_DECAY", "0.5\t"),
            ("FLAKERADAR_PORT", " 9000\n"),
        ]))
        .expect("a number read from text ignores surrounding whitespace");
        assert_eq!(settings.score_window, 7);
        assert_eq!(settings.score_decay, 0.5);
        assert_eq!(settings.port, 9000);
    }

    #[test]
    fn a_webhook_format_must_be_one_of_the_two_names_exactly() {
        let error = Settings::from_map(&map(&[("FLAKERADAR_WEBHOOK_FORMAT", " json ")]))
            .expect_err("the two names are compared exactly");
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
            // A wait that is no number at all, which is what an operator who
            // writes a unit gets.
            ("FLAKERADAR_WORKER_POLL_SECONDS", "5s"),
            ("FLAKERADAR_DB_COMMAND_TIMEOUT_SECONDS", "soon"),
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
    fn a_non_postgres_url_is_refused_with_the_documented_text() {
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
            assert_eq!(settings.database_url.expose(), url);
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
            // The database URL holds the password, so it is a credential too.
            (
                "FLAKERADAR_DATABASE_URL",
                "postgresql://flakeradar:s3cret-password@db:5432/flakeradar",
            ),
        ]))
        .expect("the fixture is valid");
        let printed = format!("{settings:?}");
        assert!(!printed.contains("super-secret-token"), "{printed}");
        assert!(!printed.contains("ghp_secret"), "{printed}");
        assert!(!printed.contains("hooks.example.com"), "{printed}");
        assert!(!printed.contains("s3cret-password"), "{printed}");
        assert!(printed.contains("Secret(..)"), "{printed}");
        // The wrapper redacts whatever it holds, including the empty default.
        assert_eq!(format!("{:?}", Secret::new("s3cret")), "Secret(..)");
    }

    #[test]
    fn the_lock_session_never_prints_its_database_password() {
        let lock = crate::worker::AdvisoryLock::new(
            "postgresql://flakeradar:s3cret-password@db:5432/flakeradar",
            flakeradar_db::DbSettings::default(),
        );
        let printed = format!("{lock:?}");
        assert!(!printed.contains("s3cret-password"), "{printed}");
        assert!(printed.contains("Secret(..)"), "{printed}");
    }

    #[test]
    fn cors_origins_split_on_commas_without_trimming() {
        let settings = Settings::from_map(&map(&[(
            "FLAKERADAR_CORS_ORIGINS",
            "http://localhost:5173,, https://a.example.com ,",
        )]))
        .expect("the fixture is valid");
        // Splitting keeps every space, so an origin written with one never
        // matched the header a browser sends.
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
        // The warning is the only notice an operator gets that the instance is
        // open to anyone who reaches it.
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
        let secure = Settings {
            api_token: Secret::new("a-real-token"),
            ..Settings::default()
        };
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
        let error = Settings::from_map(&map(&[("FLAKERADAR_DB_COMMAND_TIMEOUT_SECONDS", "0")]))
            .expect_err("a zero timeout would cancel every statement");
        assert!(
            error
                .to_string()
                .contains("FLAKERADAR_DB_COMMAND_TIMEOUT_SECONDS"),
            "{error}"
        );
        // The pool's own settings refuse it as well, for a caller that builds
        // them without going through the environment.
        flakeradar_db::DbSettings::new(0.0).expect_err("a zero timeout is refused");
        assert_eq!(
            Settings::default()
                .database_settings()
                .ok()
                .map(flakeradar_db::DbSettings::command_timeout),
            Some(Duration::from_secs(30))
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

    #[test]
    fn a_malformed_dotenv_line_names_its_number_and_never_its_text() {
        // With and without a final newline: both report the line the refused text
        // starts on, and neither repeats the text.
        for (name, contents) in [
            (
                "terminated",
                "FLAKERADAR_PORT=9000\ns3cret-token is not an assignment\n",
            ),
            (
                "unterminated",
                "FLAKERADAR_PORT=9000\ns3cret-token is not an assignment",
            ),
        ] {
            let path = dotenv_fixture(name, contents);
            let error = environment_values(&path, std::iter::empty())
                .expect_err("a malformed line is refused");
            let text = error.to_string();
            assert!(text.contains("line 2"), "{name}: {text}");
            assert!(!text.contains("s3cret-token"), "{name}: {text}");
            std::fs::remove_file(&path).expect("the fixture is removed");
        }
    }

    #[test]
    fn a_dotenv_token_never_reaches_the_log_main_writes() {
        // The `main` path: read the settings, add the context it adds, log the
        // error the way it logs one. A `.env` holds tokens, and an unclosed
        // quote is the mistake `dotenvy` reports by printing the whole line.
        let path = dotenv_fixture(
            "token",
            "FLAKERADAR_PORT=9000\nFLAKERADAR_API_TOKEN=\"s3cret-token\n",
        );
        let logged = captured_log(|| {
            let error = environment_values(&path, std::iter::empty())
                .context("reading the settings")
                .expect_err("the unclosed quote is refused");
            tracing::error!(
                name: "server.exited",
                error = ?error,
                "flakeradar exited with a failure",
            );
        });
        assert!(logged.contains("flakeradar-env-"), "{logged}");
        assert!(logged.contains("line 2"), "{logged}");
        assert!(!logged.contains("s3cret-token"), "{logged}");
        std::fs::remove_file(&path).expect("the fixture is removed");
    }

    #[test]
    fn a_value_no_configuration_of_its_field_could_mean_is_refused() {
        let cases: &[(&str, &str)] = &[
            ("FLAKERADAR_SCORE_DECAY", "NaN"),
            ("FLAKERADAR_SCORE_DECAY", "-0.5"),
            ("FLAKERADAR_SCORE_DECAY", "1.5"),
            ("FLAKERADAR_SCORE_DECAY", "inf"),
            ("FLAKERADAR_FLAKE_THRESHOLD", "-3"),
            ("FLAKERADAR_FLAKE_THRESHOLD", "NaN"),
            ("FLAKERADAR_GITHUB_ISSUE_MIN_SCORE", "2"),
            ("FLAKERADAR_SCORE_WINDOW", "-1"),
            ("FLAKERADAR_SCORE_WINDOW", "0"),
            ("FLAKERADAR_WORKER_POLL_SECONDS", "-1"),
            ("FLAKERADAR_WORKER_POLL_SECONDS", "0"),
            ("FLAKERADAR_WORKER_POLL_SECONDS", "NaN"),
            ("FLAKERADAR_PRUNE_INTERVAL_SECONDS", "-3600"),
            ("FLAKERADAR_PRUNE_TIME_BUDGET_SECONDS", "0"),
            ("FLAKERADAR_DB_COMMAND_TIMEOUT_SECONDS", "-1"),
            ("FLAKERADAR_BROKEN_STREAK", "-1"),
            ("FLAKERADAR_GITHUB_FIXED_STREAK", "-2"),
            ("FLAKERADAR_GITHUB_SYNC_BATCH", "-5"),
            ("FLAKERADAR_GITHUB_ISSUE_MIN_PROVEN_FLAKES", "-1"),
            ("FLAKERADAR_GITHUB_ISSUE_MIN_FAILURES", "-1"),
            ("FLAKERADAR_MAX_REPORT_RETRIES", "-1"),
            ("FLAKERADAR_REPORT_RETENTION_DAYS", "-1"),
            ("FLAKERADAR_FAILED_REPORT_RETENTION_DAYS", "-1"),
            ("FLAKERADAR_EXECUTION_RETENTION_DAYS", "-30"),
            ("FLAKERADAR_SCORE_HISTORY_RETENTION_DAYS", "-1"),
            ("FLAKERADAR_PRUNE_BATCH", "0"),
        ];
        for (variable, value) in cases {
            let error = Settings::from_map(&map(&[(variable, value)]))
                .expect_err("the value is outside what the field means");
            let text = error.to_string();
            assert!(text.contains(variable), "{variable}={value}: {text}");
            assert!(
                text.contains(value),
                "the refusal names the value it read: {variable}={value}: {text}"
            );
        }
    }

    #[test]
    fn a_wait_no_duration_can_hold_is_refused_by_name() {
        // A finite value above 0 is still no wait a `Duration` holds: `1e30`
        // overflows it and `1e-12` rounds to nothing, so either would have become
        // a zero wait at the point of use — a worker querying with no wait and a
        // prune running on every loop. Both are refused at start-up, by name.
        for (variable, value) in [
            ("FLAKERADAR_WORKER_POLL_SECONDS", "1e30"),
            ("FLAKERADAR_WORKER_POLL_SECONDS", "1e-12"),
            ("FLAKERADAR_PRUNE_INTERVAL_SECONDS", "1e30"),
            ("FLAKERADAR_PRUNE_INTERVAL_SECONDS", "1e-12"),
            ("FLAKERADAR_DB_COMMAND_TIMEOUT_SECONDS", "1e30"),
            ("FLAKERADAR_DB_COMMAND_TIMEOUT_SECONDS", "1e-12"),
            ("FLAKERADAR_PRUNE_TIME_BUDGET_SECONDS", "1e30"),
        ] {
            let error = Settings::from_map(&map(&[(variable, value)]))
                .expect_err("no Duration holds this wait");
            assert!(
                matches!(error, SettingsError::OutOfRange { .. }),
                "{variable}={value}: {error}"
            );
            assert!(
                error.to_string().contains(variable),
                "{variable}={value}: {error}"
            );
        }
        // The refusal quotes the value the operator typed, not a rendering of it.
        for (variable, value) in [
            ("FLAKERADAR_WORKER_POLL_SECONDS", "1e30"),
            ("FLAKERADAR_DB_COMMAND_TIMEOUT_SECONDS", "1e30"),
            ("FLAKERADAR_DB_COMMAND_TIMEOUT_SECONDS", "1e-12"),
            ("FLAKERADAR_PRUNE_TIME_BUDGET_SECONDS", "1e30"),
        ] {
            let error = Settings::from_map(&map(&[(variable, value)]))
                .expect_err("no Duration holds this wait");
            assert_eq!(
                error.to_string(),
                format!("{variable}: expected a number of seconds more than 0, got '{value}'")
            );
        }
    }

    #[test]
    fn a_database_url_error_never_carries_the_password() {
        let error = Settings::from_map(&map(&[(
            "FLAKERADAR_DATABASE_URL",
            "sqlite://flakeradar:s3cret-password@db/flakeradar",
        )]))
        .expect_err("sqlite is not PostgreSQL");
        assert!(!error.to_string().contains("s3cret-password"), "{error}");
    }

    #[test]
    fn the_settings_views_and_retention_agree_on_every_default() {
        let settings = Settings::default();
        let clock: flakeradar_db::clock::SharedClock = Arc::new(flakeradar_db::clock::SystemClock);
        let processing = flakeradar_db::processing::ProcessingSettings::default();
        let reads = flakeradar_db::reads::ReadSettings::default();
        let retention = flakeradar_db::retention::RetentionSettings::default();
        // The views are the same types on both sides, so a default that lives in
        // one place and is restated in another cannot drift unnoticed.
        assert_eq!(
            settings.processing(Arc::clone(&clock)).score_window,
            processing.score_window
        );
        assert_eq!(
            settings.reads(Arc::clone(&clock)).score_window,
            reads.score_window
        );
        assert_eq!(
            settings.processing(Arc::clone(&clock)).score_decay,
            processing.score_decay
        );
        assert_eq!(
            settings.processing(Arc::clone(&clock)).broken_streak,
            processing.broken_streak
        );
        assert_eq!(
            settings.reads(Arc::clone(&clock)).broken_streak,
            reads.broken_streak
        );
        assert_eq!(
            settings.processing(Arc::clone(&clock)).flake_threshold,
            processing.flake_threshold
        );
        assert_eq!(
            settings.reads(Arc::clone(&clock)).flake_threshold,
            reads.flake_threshold
        );
        assert_eq!(
            settings.processing(Arc::clone(&clock)).max_report_retries,
            processing.max_report_retries
        );
        assert_eq!(settings.retention(), retention);
    }
}
