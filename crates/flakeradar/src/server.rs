//! The running server: the pool, the migration, the socket and the shutdown.
//!
//! [`Server::start`] does everything the process must do before it answers a
//! request, and [`Server::serve`] answers them until its shutdown future
//! resolves. The binary calls both; a test calls them too, on a database of its
//! own and a port the kernel picks, so the path a deployment takes is the path
//! the suite proves.

// Rust guideline compliant 2026-07-21

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use axum::Router;
use sea_orm::DatabaseConnection;
use tokio::net::TcpListener;

use crate::app::{AppState, build_app};
use crate::github::Api;
use crate::github_worker::GitHubWorker;
use crate::settings::Settings;
use crate::webhooks::WebhookWorker;
use crate::worker::{AdvisoryLock, LeaderTask, ReportWorker, RetentionPrune};

/// A server that has migrated its database and bound its socket.
///
/// Building one binds the port, so a caller learns the address it serves on
/// before the first request arrives and a test can use port 0.
#[derive(Debug)]
pub struct Server {
    listener: TcpListener,
    address: SocketAddr,
    app: Router,
    pool: DatabaseConnection,
    worker: Arc<ReportWorker>,
}

/// The Notification task, built only when at least one Webhook URL is configured.
///
/// No URLs means no task at all, not an idle one: a task with nothing to deliver
/// would take part of every leader loop for no reader.
fn webhook_task(settings: &Settings) -> anyhow::Result<Option<Arc<WebhookWorker>>> {
    let urls = flakeradar_core::notify::parse_urls(settings.webhook_urls.expose());
    if urls.is_empty() {
        Ok(None)
    } else {
        Ok(Some(Arc::new(WebhookWorker::new(
            urls,
            settings.webhook_format,
            &settings.public_url,
        )?)))
    }
}

/// How long the graceful drain waits for in-flight requests before it is abandoned.
///
/// `axum::serve`'s graceful shutdown waits for open requests indefinitely, so a
/// client stalled mid-upload would otherwise keep the process from reaching the
/// ordered worker stop below, and the orchestrator's kill would take the place
/// of a clean release. The repo's deployment files set no termination grace
/// period of their own, so the period this bound must beat is Kubernetes's
/// default `terminationGracePeriodSeconds` of 30 s; at 25 s the drain is
/// abandoned, the lock is released and the process exits a full grace period
/// before the orchestrator sends SIGTERM. Raising it above 30 s would let the
/// orchestrator kill the process mid-drain.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(25);

impl Server {
    /// Connects, migrates the database, builds the app and binds the socket.
    ///
    /// The migration runs before the bind, so a process that cannot bring its
    /// database up to the baseline schema fails at start-up instead of answering
    /// every request with an error.
    ///
    /// # Errors
    ///
    /// Returns an error when the database URL is not PostgreSQL, the server
    /// cannot be reached, the migration is refused or fails, an HTTP client
    /// cannot be built, or the address cannot be bound. Each carries the setting
    /// or address it failed on.
    pub async fn start(settings: Settings) -> anyhow::Result<Self> {
        Self::start_with_request_signal(settings, None).await
    }

    /// [`Server::start`], reporting each arriving request to `signal` when one is
    /// given.
    ///
    /// The seam exists for the graceful-drain test, which has to start a shutdown
    /// while an upload is still being read; [`AppState::with_request_signal`] says
    /// what no other observable can. A deployment calls [`Server::start`] and gives
    /// no signal.
    ///
    /// # Errors
    ///
    /// As [`Server::start`].
    #[doc(hidden)]
    pub async fn start_with_request_signal(
        settings: Settings,
        signal: Option<Arc<tokio::sync::Notify>>,
    ) -> anyhow::Result<Self> {
        let pool = flakeradar_db::connect(
            settings.database_url.expose(),
            settings.database_settings()?,
        )
        .await
        .context("connecting to the database")?;
        flakeradar_db::run_migrations(&pool)
            .await
            .context("migrating the database")?;
        let mut state = AppState::new(pool.clone(), Arc::new(settings.clone()))?
            .with_assets(crate::assets::embedded());
        if let Some(signal) = signal {
            state = state.with_request_signal(signal);
        }
        let app = build_app(state);
        let requested = format!("{}:{}", settings.host, settings.port);
        let listener = TcpListener::bind(&requested)
            .await
            .with_context(|| format!("binding {requested}"))?;
        let address = listener
            .local_addr()
            .with_context(|| format!("reading the address of {requested}"))?;
        // The address is worth logging on its own: port 0 means the kernel chose
        // the port, and an operator reading the log needs the number it chose.
        tracing::info!(
            name: "server.listening",
            address = %address,
            "flakeradar listening",
        );
        // The worker starts only once the socket is bound: a start-up that fails
        // before this point has no lock to hold and no task to leak.
        let lock = AdvisoryLock::new(
            settings.database_url.expose(),
            settings.database_settings()?,
        );
        let clock: flakeradar_db::clock::SharedClock = Arc::new(flakeradar_db::clock::SystemClock);
        let processing = settings.processing(Arc::clone(&clock));
        let mut worker = ReportWorker::new(
            pool.clone(),
            Arc::new(lock),
            processing.clone(),
            settings.worker(),
        )
        .with_pruner(Arc::new(RetentionPrune::new(
            settings.retention(),
            processing,
        )));
        // The GitHub task re-checks stored links once per prune interval, the
        // same cadence retention runs on, so one loop drives both.
        let github: Arc<dyn LeaderTask> = Arc::new(
            GitHubWorker::new(
                pool.clone(),
                Arc::new(settings.clone()),
                Api::production(settings.github_token.clone())?,
                settings.prune_interval,
            )
            .with_clock(clock),
        );
        worker = worker.with_leader_task(github);
        if let Some(webhook) = webhook_task(&settings)? {
            worker = worker.with_leader_task(webhook);
        }
        let worker = Arc::new(worker);
        Arc::clone(&worker).start().await;
        Ok(Self {
            listener,
            address,
            app,
            pool,
            worker,
        })
    }

    /// The address the socket is bound to.
    ///
    /// This is the address after port 0 has been resolved, so it is the one a
    /// client connects to.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.address
    }

    /// Assembles a [`Server`] around a bound socket, app, pool and worker.
    ///
    /// [`Server::start`] always builds its own worker with the real processor;
    /// this constructor is how a test reaches [`Server::serve`] with a worker
    /// whose processor it controls, such as one that panics on its first claim.
    /// It never starts the worker; the caller does, before `serve` is asked to
    /// select on its end.
    ///
    /// # Errors
    ///
    /// Returns an error when the socket's bound address cannot be read.
    #[doc(hidden)]
    pub fn from_parts(
        listener: TcpListener,
        app: Router,
        pool: DatabaseConnection,
        worker: Arc<ReportWorker>,
    ) -> anyhow::Result<Self> {
        let address = listener.local_addr().context("reading the bound address")?;
        Ok(Self {
            listener,
            address,
            app,
            pool,
            worker,
        })
    }

    /// Serves requests until `shutdown` resolves, then stops the worker and closes the pool.
    ///
    /// Shutting the worker down after the socket stops accepting requests is
    /// what makes an in-flight upload finish: the processor keeps draining the
    /// queue until the last request has been answered, then leadership ends.
    ///
    /// # Errors
    ///
    /// Returns an error when the socket fails while serving. Stopping the worker
    /// and closing the pool afterwards are best effort: the process is on its
    /// way out, and a failure there changes nothing a caller could act on.
    pub async fn serve(
        self,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> anyhow::Result<()> {
        let serve = async {
            axum::serve(self.listener, self.app)
                .with_graceful_shutdown(shutdown)
                .await
        };
        tokio::pin!(serve);
        let worker_ended = self.worker.ended();
        tokio::pin!(worker_ended);
        let drain = tokio::time::timeout(DRAIN_TIMEOUT, &mut serve);
        tokio::pin!(drain);

        // The socket stops accepting on the shutdown signal, and the worker
        // loop is selected on too, so a loop that died without a stop — a panic
        // — is an error the process exits on. The graceful drain is bounded, so
        // a client stalled mid-upload cannot hold the ordered stop open forever.
        let outcome = tokio::select! {
            drain_answer = &mut drain => match drain_answer {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(anyhow::Error::new(error).context("serving requests")),
                Err(_elapsed) => {
                    tracing::warn!(
                        name: "server.drain_timed_out",
                        seconds = DRAIN_TIMEOUT.as_secs(),
                        "the graceful drain could not finish the in-flight request; abandoned",
                    );
                    None
                }
            },
            () = &mut worker_ended => {
                tracing::error!(
                    name: "worker.processor.dead",
                    "the Report worker loop ended before the shutdown signal",
                );
                Some(anyhow::anyhow!(
                    "the Report worker loop ended before the shutdown signal"
                ))
            }
        };
        self.worker.stop().await;
        if let Err(error) = self.pool.close().await {
            tracing::warn!(
                name: "server.pool_close_failed",
                error = %error,
                "closing the pool failed",
            );
        }
        tracing::info!(
            name: "server.stopped",
            "flakeradar stopped",
        );
        match outcome {
            None => Ok(()),
            Some(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::webhook_task;
    use crate::settings::Settings;

    /// Settings built from `pairs`.
    fn settings(pairs: &[(&str, &str)]) -> Settings {
        let values: BTreeMap<String, String> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        Settings::from_map(&values).expect("the test's settings are valid")
    }

    #[test]
    fn the_lifespan_builds_a_webhook_worker_only_when_urls_are_set() {
        assert!(
            webhook_task(&settings(&[("FLAKERADAR_API_TOKEN", "real-token")]))
                .expect("no URLs is not a failure")
                .is_none(),
            "no URLs means no Notification task at all"
        );
        // What the task does with the URLs it was given is `tests/webhooks.rs`'s
        // subject, where a real endpoint receives the batch.
        assert!(
            webhook_task(&settings(&[
                ("FLAKERADAR_API_TOKEN", "real-token"),
                ("FLAKERADAR_WEBHOOK_URLS", " https://hooks.example.com/a , "),
            ]))
            .expect("a configured endpoint builds the task")
            .is_some(),
            "one URL means one Notification task"
        );
    }
}
