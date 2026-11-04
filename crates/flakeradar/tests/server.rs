//! The server on a real socket, over a database that has no schema yet.
//!
//! This is the start-up a deployment takes: [`flakeradar::Server::start`]
//! connects, migrates, builds the app and binds a port, and nothing answers
//! until it has done all three. The port is 0, so the kernel picks one and the
//! test asks the server which it got; the database is [`TestDb::empty`], so the
//! migration has to run for any route that reads a table to answer.

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when the server does not answer as it must"
)]

mod common;

use flakeradar::{Server, Settings};
use flakeradar_test_support::TestDb;
use sea_orm::ConnectionTrait as _;
use serde_json::json;
use tokio::sync::oneshot;

/// Settings for a server on `db`, on the kernel's choice of port.
fn settings_for(db: &TestDb) -> Settings {
    common::settings(&[
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
    // The frozen server upgraded an Alembic database; FlakeRadar 2 refuses it and
    // changes nothing, and that refusal happens before the socket is bound.
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
