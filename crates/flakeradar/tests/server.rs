//! The server on a real socket, over a database that has no schema yet.
//!
//! This is the start-up a deployment takes: [`flakeradar::server::Server::start`]
//! connects, migrates, builds the app and binds a port, and nothing answers
//! until it has done all three. The port is 0, so the kernel picks one and the
//! test asks the server which it got; the database is [`TestDb::empty`], so the
//! migration has to run for any route that reads a table to answer.

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when the server does not answer as it must"
)]

use flakeradar::{server::Server, settings::Settings};
use flakeradar_test_support::{TestDb, settings, wait_until};

use sea_orm::{ConnectionTrait as _, EntityTrait as _, PaginatorTrait as _};
use serde_json::json;
use tokio::sync::oneshot;

/// Settings for a server on `db`, on the kernel's choice of port.
fn settings_for(db: &TestDb) -> Settings {
    settings(&[
        ("FLAKERADAR_DATABASE_URL", db.url()),
        ("FLAKERADAR_HOST", "127.0.0.1"),
        ("FLAKERADAR_PORT", "0"),
        ("FLAKERADAR_API_TOKEN", "a-real-token"),
    ])
}

#[tokio::test]
async fn a_fresh_database_is_migrated_and_the_shell_answers() {
    let db = TestDb::empty().await.expect("a test database is available");
    let server = Server::start(settings_for(&db))
        .await
        .expect("the server starts");
    let address = server.local_addr();
    assert_ne!(address.port(), 0, "port 0 is resolved before it is served");
    let (stop, stopped) = oneshot::channel();
    let serving = tokio::spawn(server.serve(async move {
        let _ = stopped.await;
    }));

    let base = format!("http://{address}");
    let client = reqwest::Client::new();

    let reply = client
        .get(format!("{base}/api/health"))
        .send()
        .await
        .expect("the socket answers");
    assert_eq!(reply.status(), 200);
    assert_eq!(reply.text().await.expect("a body"), r#"{"status":"ok"}"#);

    // The readiness probe queries `reports`, which only exists once the baseline
    // migration has run, so a 200 with `database: true` proves the fresh database
    // was migrated by start-up.
    let reply = client
        .get(format!("{base}/api/health/ready"))
        .send()
        .await
        .expect("the socket answers");
    assert_eq!(reply.status(), 200);
    let body: serde_json::Value = reply.json().await.expect("a JSON body");
    assert_eq!(body["database"], json!(true), "{body}");
    assert_eq!(body["pending_reports"], json!(0), "{body}");

    let reply = client
        .get(format!("{base}/docs"))
        .send()
        .await
        .expect("the socket answers");
    assert_eq!(reply.status(), 200);
    assert_eq!(
        reply
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("text/html; charset=utf-8")
    );
    let html = reply.text().await.expect("a body");
    assert!(html.contains("/docs/redoc.standalone.js"), "{html}");

    let reply = client
        .get(format!("{base}/openapi.json"))
        .send()
        .await
        .expect("the socket answers");
    assert_eq!(reply.status(), 200);
    let document: serde_json::Value = reply.json().await.expect("a JSON body");
    assert_eq!(document["info"]["title"], json!("FlakeRadar"));
    assert!(document["paths"].get("/api/health").is_some(), "{document}");

    let _ = stop.send(());
    serving
        .await
        .expect("the serving task ends")
        .expect("the server stops cleanly");
}

#[tokio::test]
async fn a_database_an_older_flakeradar_created_stops_the_start_up() {
    // A 1.x database is refused and not changed; FlakeRadar 2 has no Alembic
    // adoption path, and that refusal happens before the socket is bound.
    let db = TestDb::empty().await.expect("a test database is available");
    db.connection()
        .execute_unprepared("CREATE TABLE alembic_version (version_num varchar(32) NOT NULL)")
        .await
        .expect("the older migration table exists");
    let error = Server::start(settings_for(&db))
        .await
        .expect_err("an Alembic database is an error");
    let message = format!("{error:#}");
    assert!(message.contains("migrating the database"), "{message}");
    assert!(message.contains("alembic_version"), "{message}");
}

#[tokio::test]
async fn a_port_already_in_use_stops_the_start_up() {
    let db = TestDb::empty().await.expect("a test database is available");
    let taken = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a socket to hold");
    let port = taken.local_addr().expect("its address").port();
    let mut settings = settings_for(&db);
    settings.port = port;
    let error = Server::start(settings)
        .await
        .expect_err("a taken port is an error");
    let message = format!("{error:#}");
    assert!(
        message.contains(&format!("binding 127.0.0.1:{port}")),
        "{message}"
    );
    taken
        .local_addr()
        .expect("the holding socket is still open");
}

/// Whether another session can take the Report processor lock right now.
async fn processor_lock_is_free(db: &TestDb) -> bool {
    use flakeradar_db::{DbSettings, WORKER_LOCK_KEY, connect};
    use sea_orm::Statement;

    let pool = connect(db.url(), DbSettings::default())
        .await
        .expect("the probe's pool");
    let statement = Statement::from_sql_and_values(
        sea_orm::DbBackend::Postgres,
        "SELECT pg_try_advisory_lock($1) AS locked",
        [WORKER_LOCK_KEY.into()],
    );
    let row = pool
        .query_one_raw(statement)
        .await
        .expect("the probe query runs")
        .expect("the probe query answers");
    let free: bool = sea_orm::TryGetable::try_get(&row, "", "locked").expect("a flag");
    if free {
        let statement = Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            "SELECT pg_advisory_unlock($1)",
            [WORKER_LOCK_KEY.into()],
        );
        pool.execute_raw(statement)
            .await
            .expect("the probe releases the lock");
    }
    pool.close().await.expect("the probe's pool closes");
    free
}

#[tokio::test]
async fn a_server_with_webhook_urls_starts_and_stops_the_notification_task() {
    let db = TestDb::new().await.expect("a test database");
    let settings = settings(&[
        ("FLAKERADAR_DATABASE_URL", db.url()),
        ("FLAKERADAR_HOST", "127.0.0.1"),
        ("FLAKERADAR_PORT", "0"),
        ("FLAKERADAR_API_TOKEN", "a-real-token"),
        ("FLAKERADAR_WEBHOOK_URLS", "http://127.0.0.1:1/hook"),
    ]);
    let server = Server::start(settings).await.expect("the server starts");
    let address = server.local_addr();
    let (stop, stopped) = oneshot::channel();
    let serving = tokio::spawn(server.serve(async move {
        let _ = stopped.await;
    }));

    let reply = reqwest::Client::new()
        .get(format!("http://{address}/api/health"))
        .send()
        .await
        .expect("the socket answers");
    assert_eq!(reply.status(), 200);

    let _ = stop.send(());
    serving
        .await
        .expect("the serving task ends")
        .expect("the server stops cleanly with the Notification task running");
}

#[tokio::test]
async fn the_readiness_probe_reports_the_worker_leading_and_releases_it_on_shutdown() {
    let db = TestDb::new().await.expect("a test database");
    let server = Server::start(settings_for(&db))
        .await
        .expect("the server starts");
    let address = server.local_addr();
    let (stop, stopped) = oneshot::channel();
    let serving = tokio::spawn(server.serve(async move {
        let _ = stopped.await;
    }));

    // The worker elects itself on its first poll, so the probe reports the leader
    // once that has happened; the wait bounds a failure, not a duration.
    let client = reqwest::Client::new();
    let answers = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    wait_until("the worker leads the running server", || {
        let client = &client;
        let answers = std::sync::Arc::clone(&answers);
        async move {
            let body: serde_json::Value = client
                .get(format!("http://{address}/api/health/ready"))
                .send()
                .await
                .expect("the socket answers")
                .json()
                .await
                .expect("a JSON body");
            let leading = body["processor_leader"] == json!(true);
            answers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(body);
            leading
        }
    })
    .await;
    let body = {
        let answers = answers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        answers.last().cloned().expect("the probe answered")
    };
    assert_eq!(body["database"], json!(true), "{body}");

    let _ = stop.send(());
    serving
        .await
        .expect("the serving task ends")
        .expect("the server stops cleanly");

    // After the shutdown the lock is free again, so the next readiness probe any
    // process answers says no leader.
    assert!(
        processor_lock_is_free(&db).await,
        "the worker released its lock"
    );
}

#[tokio::test]
async fn a_shutdown_finishes_an_in_flight_request_and_stops_the_worker() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let db = TestDb::new().await.expect("a test database");
    let server = Server::start(settings_for(&db))
        .await
        .expect("the server starts");
    let address = server.local_addr();
    let (stop, stopped) = oneshot::channel();
    let serving = tokio::spawn(server.serve(async move {
        let _ = stopped.await;
    }));

    let body = flakeradar_test_support::make_junit(&[("t", "passed")]);
    // Half the body is sent first, so the upload is still being read when the
    // shutdown starts: the handler finishes and answers.
    let head = format!(
        "POST /api/ingest?repo=o/r&commit_sha=a1 HTTP/1.1\r\nHost: {address}\r\n\
         X-API-Key: a-real-token\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let mut stream = tokio::net::TcpStream::connect(address)
        .await
        .expect("the socket connects");
    stream
        .write_all(head.as_bytes())
        .await
        .expect("the request head is written");
    let (sent, rest) = body.split_at(body.len() / 2);
    stream.write_all(sent).await.expect("half the body is sent");
    for _ in 0..4 {
        tokio::task::yield_now().await;
    }

    let _ = stop.send(());
    stream
        .write_all(rest)
        .await
        .expect("the rest of the body is sent");
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .expect("the response is read");
    let text = String::from_utf8_lossy(&response);
    assert!(text.starts_with("HTTP/1.1 202 Accepted"), "{text}");

    serving
        .await
        .expect("the serving task ends")
        .expect("the server stops cleanly");
    let reports: u64 = flakeradar_db::entities::reports::Entity::find()
        .count(db.connection())
        .await
        .expect("the Reports count");
    assert_eq!(reports, 1, "the in-flight upload was queued");
    assert!(
        processor_lock_is_free(&db).await,
        "the worker released its lock"
    );
}
