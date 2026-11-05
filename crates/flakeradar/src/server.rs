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
    /// cannot be reached, the migration is refused or fails, or the address
    /// cannot be bound. Each carries the setting or address it failed on.
    pub async fn start(settings: Settings) -> anyhow::Result<Self> {
        let pool = flakeradar_db::connect(&settings.database_url, settings.database_settings()?)
            .await
            .context("connecting to the database")?;
        flakeradar_db::run_migrations(&pool)
            .await
            .context("migrating the database")?;
        let app = build_app(AppState::new(pool.clone(), Arc::new(settings.clone())));
        let requested = format!("{}:{}", settings.host, settings.port);
        let listener = TcpListener::bind(&requested)
            .await
            .with_context(|| format!("binding {requested}"))?;
        let address = listener
            .local_addr()
            .with_context(|| format!("reading the address of {requested}"))?;
        // The address is worth logging on its own: port 0 means the kernel chose
        // the port, and an operator reading the log needs the number it chose.
        tracing::info!(address = %address, "flakeradar listening");
        // The worker starts only once the socket is bound: a start-up that fails
        // before this point has no lock to hold and no task to leak.
        let lock = AdvisoryLock::new(settings.database_url.clone(), settings.database_settings()?);
        let processing = settings.processing();
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
        // same interval the reference used for both.
        let github: Arc<dyn LeaderTask> = Arc::new(GitHubWorker::new(
            pool.clone(),
            Arc::new(settings.clone()),
            Api::production(settings.github_token.clone()),
            std::time::Duration::try_from_secs_f64(settings.prune_interval_seconds)
                .unwrap_or_default(),
        ));
        worker = worker.with_leader_task(github);
        // The Notification task exists only when an endpoint is configured.
        let urls = flakeradar_core::parse_urls(settings.webhook_urls.expose());
        if !urls.is_empty() {
            let webhook: Arc<dyn LeaderTask> = Arc::new(WebhookWorker::new(
                urls,
                settings.webhook_format,
                &settings.public_url,
            ));
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
        let result = axum::serve(self.listener, self.app)
            .with_graceful_shutdown(shutdown)
            .await
            .context("serving requests");
        self.worker.stop().await;
        if let Err(error) = self.pool.close().await {
            tracing::warn!(error = %error, "closing the pool failed");
        }
        tracing::info!("flakeradar stopped");
        result
    }
}
