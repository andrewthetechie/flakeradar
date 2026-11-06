//! The GitHub REST client, and the one classifier of what it answered.
//!
//! Every call is one request against the REST API, read as an [`Answer`] whatever
//! the status; a request that nothing answers is the only error. [`Answer::classify`]
//! turns that answer into the one [`Verdict`] every operation reads, so a status is
//! judged in exactly one place.
//!
//! Nothing here logs the token: the credential lives in [`Secret`] and reaches a
//! request header only. `reqwest` errors have their URL removed before they reach a
//! log line, for the reason the Webhook sender does.

// Rust guideline compliant 2026-07-21

use std::fmt;

use anyhow::Context as _;
use flakeradar_core::dto::MAX_ISSUE_NUMBER;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use reqwest::{Client, Method};
use serde_json::{Value, json};

use super::policy::Verdict;
use crate::settings::Secret;

/// The GitHub REST base URL production talks to.
pub const API_BASE: &str = "https://api.github.com";

/// The issue API version every request asks for.
pub const API_VERSION: &str = "2022-11-28";

/// The request timeout every call uses.
///
/// One call is one small REST request against GitHub's API, which answers in
/// well under a second; fifteen seconds bounds a filing batch's stall on a
/// black-holed connection. It is also the lateness a stop can tolerate: the
/// issue-creating `POST` is the one call left to finish, so it is the longest a
/// shutdown waits. Raising it delays shutdown and blocks the filing batch;
/// lowering it fails a request a slow GitHub would have answered.
pub const TIMEOUT_SECONDS: u64 = 15;

/// How many characters of a response body a log line keeps.
///
/// A failed call logs what the endpoint said, which is a JSON error object or an
/// HTML error page: 300 code points holds the message and drops the page. The
/// text can quote a request, so a longer excerpt is also more log to scrub.
/// Raising it puts whole error pages in the log; lowering it cuts the message
/// that says why GitHub refused.
pub const RESPONSE_TEXT_MAX: usize = 300;

/// One HTTP answer, whatever its status.
///
/// The body is read as text and as the JSON object it holds, and only
/// [`Answer::new`] can build one, so the two always describe the same answer.
#[derive(Debug, Clone)]
pub struct Answer {
    status: u16,
    headers: HeaderMap,
    text: String,
    body: Option<Value>,
}

/// A request that never reached GitHub.
#[derive(Debug, thiserror::Error)]
#[error("GitHub request failed: {message}")]
pub struct TransportError {
    /// The failure text, with the URL removed so a token-bearing base URL cannot leak.
    pub message: String,
}

impl Answer {
    /// One answer, with its body parsed the single time every accessor reads it.
    #[must_use]
    pub fn new(status: u16, headers: HeaderMap, text: String) -> Self {
        let body = serde_json::from_str::<Value>(&text)
            .ok()
            .filter(Value::is_object);
        Self {
            status,
            headers,
            text,
            body,
        }
    }

    /// The status code the answer carried.
    #[must_use]
    pub fn status(&self) -> u16 {
        self.status
    }

    /// Classifies this answer into the one verdict every operation reads.
    ///
    /// `success` is the status this call's success carries: a `201` for a create or
    /// a comment, a `200` for a read or a close. Any other 2xx is
    /// [`Verdict::Other`], which each operation treats as a failure of its own.
    ///
    /// The rate limit is decided first, by [`Answer::rate_limited`], so a batch
    /// stops on exactly the answers it stops on elsewhere.
    #[must_use]
    pub fn classify(&self, success: Success) -> Verdict {
        if self.rate_limited() {
            return Verdict::RateLimited;
        }
        match self.status {
            401 => Verdict::Unauthorized,
            403 => Verdict::RepoDenied,
            404 => Verdict::NotFound,
            410 => Verdict::Gone,
            status if (200..300).contains(&status) => {
                if status == success.status() {
                    Verdict::Accepted
                } else {
                    Verdict::Other
                }
            }
            _ => Verdict::Other,
        }
    }

    /// The body as the JSON object it holds, or `None` when it is not one.
    #[must_use]
    pub fn json_object(&self) -> Option<&Value> {
        self.body.as_ref()
    }

    /// The number of a created issue, or `None` when the body does not carry one.
    #[must_use]
    pub fn issue_number(&self) -> Option<i32> {
        let number = self.json_object()?.get("number")?.as_i64()?;
        (1..=MAX_ISSUE_NUMBER)
            .contains(&number)
            .then(|| i32::try_from(number).ok())
            .flatten()
    }

    /// The state a fetched issue reports.
    #[must_use]
    pub fn issue_state(&self) -> Option<String> {
        let state = self.json_object()?.get("state")?.as_str()?.to_owned();
        Some(state)
    }

    /// Why a fetched issue changed state.
    #[must_use]
    pub fn state_reason(&self) -> Option<String> {
        let reason = self
            .json_object()?
            .get("state_reason")?
            .as_str()?
            .to_owned();
        Some(reason)
    }

    /// Whether GitHub is throttling: 429, or a 403 that is a rate limit.
    ///
    /// A 403 also means *permission*, and treating those as rate limits would
    /// stop the batch for every other Repo, so a 403 counts only when the
    /// remaining count is zero, a `retry-after` header is present, or the body
    /// says "rate limit".
    #[must_use]
    pub fn rate_limited(&self) -> bool {
        if self.status == 429 {
            return true;
        }
        if self.status != 403 {
            return false;
        }
        let remaining = self
            .headers
            .get("x-ratelimit-remaining")
            .and_then(|value| value.to_str().ok());
        if remaining == Some("0") || self.headers.contains_key("retry-after") {
            return true;
        }
        let message = self
            .json_object()
            .and_then(|body| body.get("message"))
            .map(|message| message.to_string().to_lowercase())
            .unwrap_or_default();
        message.contains("rate limit")
    }

    /// The body text a log line keeps.
    #[must_use]
    pub fn short_text(&self) -> String {
        self.text.chars().take(RESPONSE_TEXT_MAX).collect()
    }

    /// The remaining rate limit, as a log line prints it.
    #[must_use]
    pub fn remaining_text(&self) -> String {
        self.headers
            .get("x-ratelimit-remaining")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("unknown")
            .to_owned()
    }
}

/// The status one call's success carries, and nothing else.
///
/// GitHub answers `201` to a create or a comment and `200` to a read or a close; a
/// 2xx that is not the one the call asks for is a failure the caller reads itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Success {
    /// A create or a comment: `201`.
    Created,
    /// A read or a close: `200`.
    Fetched,
}

impl Success {
    /// The status code this success carries.
    #[must_use]
    fn status(self) -> u16 {
        match self {
            Self::Created => 201,
            Self::Fetched => 200,
        }
    }
}

/// The GitHub REST client, with its base URL injectable for a test.
///
/// The credential is a [`Secret`], so a `Debug` line cannot print it, and the
/// token only ever reaches a request header.
#[derive(Clone)]
pub struct Api {
    client: Client,
    base: String,
    token: Secret,
}

impl fmt::Debug for Api {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Api")
            .field("base", &self.base)
            .field("token", &self.token)
            .finish_non_exhaustive()
    }
}

/// The HTTP client every GitHub request goes through.
///
/// The timeout is what keeps a slow or unreachable GitHub API from holding a
/// batch, and the user agent is what every endpoint expects to be told.
fn github_client() -> anyhow::Result<Client> {
    Ok(Client::builder()
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECONDS))
        .user_agent(format!("flakeradar/{}", env!("CARGO_PKG_VERSION")))
        .build()?)
}

impl Api {
    /// A client over `base`, presenting `token`.
    ///
    /// # Errors
    ///
    /// Returns an error when the client cannot be built, which is the TLS backend
    /// failing to initialize. A client without the timeout would break this
    /// module's error policy, so none is used.
    pub fn new(token: Secret, base: &str) -> anyhow::Result<Self> {
        Self::with_client_build(token, base, github_client)
    }

    /// The same client, with `build` making the HTTP client instead.
    ///
    /// The seam is for the one failure a test cannot stage: only a TLS backend
    /// that cannot initialize fails a build of a healthy [`Client`], so a test
    /// asks `build` for the error and asserts the constructor carries it rather
    /// than answering a client without the timeout.
    ///
    /// # Errors
    ///
    /// Returns `build`'s error, contexted with what could not be built.
    pub fn with_client_build(
        token: Secret,
        base: &str,
        build: impl FnOnce() -> anyhow::Result<Client>,
    ) -> anyhow::Result<Self> {
        let client = build().context("building the GitHub HTTP client")?;
        Ok(Self {
            client,
            base: base.trim_end_matches('/').to_owned(),
            token,
        })
    }

    /// A client against the production REST API.
    ///
    /// # Errors
    ///
    /// Returns an error when the client cannot be built; see [`Api::new`].
    pub fn production(token: Secret) -> anyhow::Result<Self> {
        Self::new(token, API_BASE)
    }

    /// Whether a token is configured at all.
    #[must_use]
    pub fn configured(&self) -> bool {
        !self.token.expose().is_empty()
    }

    /// The headers every request carries.
    fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(
            "X-GitHub-Api-Version",
            HeaderValue::from_static(API_VERSION),
        );
        if let Ok(value) = HeaderValue::from_str(&format!("Bearer {}", self.token.expose())) {
            headers.insert(AUTHORIZATION, value);
        }
        headers
    }

    /// Sends one request and reads its answer.
    pub(super) async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Answer, TransportError> {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.base))
            .headers(self.headers());
        if let Some(body) = body {
            request = request.header(CONTENT_TYPE, "application/json").json(&body);
        }
        let response = request.send().await.map_err(|error| TransportError {
            message: error.without_url().to_string(),
        })?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let text = response.text().await.map_err(|error| TransportError {
            message: error.without_url().to_string(),
        })?;
        Ok(Answer::new(status, headers, text))
    }

    /// Creates one issue in `repo`.
    pub(super) async fn create_issue(
        &self,
        repo: &str,
        title: &str,
        body: &str,
        label: &str,
    ) -> Result<Answer, TransportError> {
        self.send(
            Method::POST,
            &format!("/repos/{repo}/issues"),
            Some(json!({"title": title, "body": body, "labels": [label]})),
        )
        .await
    }

    /// Fetches one issue.
    pub(super) async fn get_issue(
        &self,
        repo: &str,
        number: i32,
    ) -> Result<Answer, TransportError> {
        self.send(Method::GET, &format!("/repos/{repo}/issues/{number}"), None)
            .await
    }

    /// Posts one comment on an issue.
    pub(super) async fn comment(
        &self,
        repo: &str,
        number: i32,
        body: &str,
    ) -> Result<Answer, TransportError> {
        self.send(
            Method::POST,
            &format!("/repos/{repo}/issues/{number}/comments"),
            Some(json!({"body": body})),
        )
        .await
    }

    /// Closes one issue, with an optional state reason.
    pub(super) async fn close_issue(
        &self,
        repo: &str,
        number: i32,
        reason: Option<&str>,
    ) -> Result<Answer, TransportError> {
        let body = match reason {
            Some(reason) => json!({"state": "closed", "state_reason": reason}),
            None => json!({"state": "closed"}),
        };
        self.send(
            Method::PATCH,
            &format!("/repos/{repo}/issues/{number}"),
            Some(body),
        )
        .await
    }
}
