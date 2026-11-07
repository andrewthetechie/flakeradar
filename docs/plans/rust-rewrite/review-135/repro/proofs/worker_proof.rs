//! Proofs for S1/H1 and H5, rewritten after the fix: a Notification task that was
//! stopped runs again after the next election, and no `Debug` line prints the
//! database password.
//!
//! At `f9330fd` the second `start` re-entered on the first run's cancelled token
//! and the task exited at once, and `Settings`/`AdvisoryLock` printed the password.

use flakeradar::settings::Settings;
use flakeradar::webhooks::WebhookWorker;
use flakeradar::worker::{AdvisoryLock, LeaderTask};
use flakeradar_core::classify::FailureCategory;
use flakeradar_core::model::{EntityKind, ReportStatus, Tier};
use flakeradar_core::notify::{TierChange, WebhookFormat};
use flakeradar_db::DbSettings;
use flakeradar_db::processing::ProcessOutcome;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// A loopback endpoint that answers every POST and counts it.
fn counting_endpoint() -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let address = listener.local_addr().expect("the bound address");
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&hits);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut buffer = [0_u8; 4096];
            let _ = stream.read(&mut buffer);
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok");
            counted.fetch_add(1, Ordering::SeqCst);
        }
    });
    (format!("http://{address}/hook"), hits)
}

fn outcome() -> ProcessOutcome {
    ProcessOutcome {
        report_id: 1,
        status: ReportStatus::Processed,
        run_id: None,
        counts: None,
        touched_test_ids: Vec::new(),
        error: None,
        touched_job_ids: Vec::new(),
        tier_changes: vec![TierChange {
            kind: EntityKind::Test,
            id: 1,
            repo: "a/b".to_owned(),
            project: Some("p".to_owned()),
            name: "t".to_owned(),
            flakiness_score: 0.9,
            tier: Tier::Flaky,
            proven_flake_count: 1,
            classname: Some("c".to_owned()),
            pipeline: None,
            failure_category: Some(FailureCategory::Other),
        }],
        retry_count: 0,
    }
}

async fn wait_for(hits: &AtomicUsize, want: usize, within: Duration) -> usize {
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        if hits.load(Ordering::SeqCst) >= want {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    hits.load(Ordering::SeqCst)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_notification_task_delivers_again_after_a_restart() {
    let (url, hits) = counting_endpoint();
    let worker = Arc::new(
        WebhookWorker::with_retry_delays(vec![url], WebhookFormat::Json, "", [0.0, 0.0])
            .expect("the client builds"),
    );
    LeaderTask::start(worker.as_ref()).await;
    LeaderTask::enqueue(worker.as_ref(), &outcome()).expect("the batch is queued");
    let first = wait_for(&hits, 1, Duration::from_secs(10)).await;
    println!("first term: deliveries = {first}");
    assert_eq!(first, 1);

    LeaderTask::stop(worker.as_ref()).await.expect("the task stops");
    LeaderTask::start(worker.as_ref()).await; // re-elected
    LeaderTask::enqueue(worker.as_ref(), &outcome()).expect("the batch is queued");
    let second = wait_for(&hits, 2, Duration::from_secs(10)).await;
    println!("second term: deliveries = {second}");
    assert_eq!(
        second, 2,
        "the restarted task must deliver again, not exit on the old cancelled token"
    );
    LeaderTask::stop(worker.as_ref()).await.expect("the task stops");
}

#[test]
fn database_password_never_reaches_a_debug_line() {
    let mut values = BTreeMap::new();
    values.insert(
        "FLAKERADAR_DATABASE_URL".to_owned(),
        "postgresql://flakeradar:s3cret-password@db:5432/flakeradar".to_owned(),
    );
    let settings = Settings::from_map(&values).expect("the settings parse");

    let printed = format!("{settings:?}");
    println!("Settings Debug: {printed}");
    assert!(!printed.contains("s3cret-password"));

    let lock = AdvisoryLock::new(
        settings.database_url.expose().to_owned(),
        DbSettings::new(settings.db_command_timeout_seconds).expect("a timeout"),
    );
    let printed = format!("{lock:?}");
    println!("AdvisoryLock Debug: {printed}");
    assert!(!printed.contains("s3cret-password"));
}
