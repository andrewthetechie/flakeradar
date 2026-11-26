//! The delivery mechanics both leader tasks share: the Webhook client, the
//! host-only endpoint label, and the classification of one `POST`.
//!
//! A Notification and the weekly digest differ in policy — how many attempts an
//! endpoint gets, which refusals are retryable, whether the endpoints are fanned
//! out or taken in order — and agree on everything else. This module holds that
//! "everything else", so the rule that a Webhook URL is a credential (ADR 0009)
//! is stated, and redacted, in exactly one place.
//!
//! Nothing here logs. A delivery answer carries only endpoint-safe identifiers —
//! the host, the status, the response headers, a `reqwest` error with its URL
//! removed — and each caller logs them under its own static message, so one
//! outcome is still one log event and the sender cannot turn a task's wording
//! into data. Retry policy, attempt counts, the retryability split, fan-out and
//! sequencing stay with the callers: [`WebhookSender::post`] answers one `POST`
//! to one URL and stops there.

// Rust guideline compliant 2026-07-21

use std::time::Duration;

use serde_json::Value;

use crate::webhooks::WEBHOOK_TIMEOUT_SECONDS;

/// The endpoint identifier a log line may name: `url`'s host, or a generic label.
///
/// A Webhook URL is a credential — the path of a Slack hook URL *is* the token —
/// so no log line carries it (ADR 0009). A URL that cannot be parsed gets the
/// generic label rather than its own text, which is what keeps an operator's typo
/// from printing the secret it may contain.
#[must_use]
pub fn endpoint(url: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_owned))
        .unwrap_or_else(|| "endpoint".to_owned())
}

/// The HTTP client every Webhook goes through.
///
/// The timeout is what keeps one slow endpoint from parking a leader task's queue,
/// and the user agent is what every endpoint expects to be told. This is the seam
/// a worker builds its client through, so a test can hand it a build that fails.
///
/// # Errors
///
/// Returns an error when the HTTP client cannot be built, which is the TLS backend
/// failing to initialize; a client without the timeout would break this module's
/// error policy, so none is used.
pub fn client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(WEBHOOK_TIMEOUT_SECONDS))
        .user_agent(format!("flakeradar/{}", env!("CARGO_PKG_VERSION")))
        .build()?)
}

/// What one `POST` answered, carrying only endpoint-safe identifiers.
///
/// A refusal arrives as the status and the response headers rather than as a
/// parsed wait: how long to honour a `retry-after`, and whether to wait at all, is
/// the caller's policy.
#[derive(Debug)]
pub enum Delivery {
    /// The endpoint took the body.
    Sent {
        /// The host the body went to, or the generic label.
        endpoint: String,
    },
    /// A refusal: the status and the response headers, so the caller can read
    /// `retry-after` under its own cap.
    Refused {
        /// The host the body went to, or the generic label.
        endpoint: String,
        /// The status the endpoint refused with.
        status: u16,
        /// The refusal's headers, unparsed.
        headers: reqwest::header::HeaderMap,
    },
    /// No answer at all: a transport failure whose URL has already been removed.
    Unreachable {
        /// The host the body went to, or the generic label.
        endpoint: String,
        /// The `reqwest` error, with its URL removed.
        error: String,
    },
}

/// One Webhook endpoint's delivery: a client, and exactly one `POST` through it.
///
/// It holds no URL, so its derived `Debug` cannot print a credential either.
#[derive(Debug)]
pub struct WebhookSender {
    client: reqwest::Client,
}

impl WebhookSender {
    /// A sender with the Webhook client of [`client`].
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be built; see [`client`].
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self { client: client()? })
    }

    /// The same sender, delivering through a client the caller built.
    ///
    /// The seam is for a worker that owns its own build error context — only a TLS
    /// backend that cannot initialize fails a build of a healthy client, so this is
    /// how a test stages that failure.
    #[must_use]
    pub fn from_client(client: reqwest::Client) -> Self {
        Self { client }
    }

    /// Posts `body` to `url` once and says what the answer means. No retry, no
    /// fan-out.
    ///
    /// A transport error has its URL removed before it is returned, so the text
    /// cannot reach a log line carrying the credential (ADR 0009).
    #[must_use]
    pub async fn post(&self, url: &str, body: &Value) -> Delivery {
        let label = endpoint(url);
        match self.client.post(url).json(body).send().await {
            Ok(response) if response.status().is_success() => Delivery::Sent { endpoint: label },
            Ok(response) => Delivery::Refused {
                endpoint: label,
                status: response.status().as_u16(),
                headers: response.headers().clone(),
            },
            Err(error) => Delivery::Unreachable {
                endpoint: label,
                error: error.without_url().to_string(),
            },
        }
    }
}
