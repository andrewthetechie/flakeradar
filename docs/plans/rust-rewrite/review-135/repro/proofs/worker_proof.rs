use std::sync::Arc;
use std::time::Duration;

use flakeradar::Settings;
use flakeradar::webhooks::WebhookWorker;
use flakeradar::worker::{AdvisoryLock, LeaderTask};
use flakeradar_core::{Kind, Tier, TierChange, WebhookFormat};
use flakeradar_db::{DbSettings, ProcessOutcome};

fn outcome() -> ProcessOutcome {
    ProcessOutcome {
        report_id: 1,
        status: "processed".to_owned(),
        run_id: None,
        counts: None,
        touched_test_ids: Vec::new(),
        error: None,
        touched_job_ids: Vec::new(),
        tier_changes: vec![TierChange {
            kind: Kind::Test,
            id: 1,
            repo: "a/b".to_owned(),
            project: Some("p".to_owned()),
            name: "t".to_owned(),
            flakiness_score: 0.9,
            tier: Tier::Flaky,
            confirmed_flake_count: 1,
            classname: Some("c".to_owned()),
            pipeline: None,
            failure_category: None,
        }],
        retry_count: 0,
    }
}

async fn drained(worker: &WebhookWorker, within: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        if worker.queued() == 0 {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    worker.queued() == 0
}

#[tokio::test(flavor = "multi_thread")]
async fn a_webhook_worker_never_works_again_after_one_stop() {
    // Port 9 refuses at once, so a delivery ends quickly; only the queue matters.
    let worker = Arc::new(WebhookWorker::with_retry_delays(
        vec!["http://127.0.0.1:9/hook".to_owned()],
        WebhookFormat::Json,
        "",
        [0.0, 0.0],
    ));
    LeaderTask::start(worker.as_ref()).await;
    LeaderTask::enqueue(worker.as_ref(), &outcome()).unwrap();
    let first = drained(&worker, Duration::from_secs(5)).await;
    println!("first term: queue drained = {first}");
    assert!(first);

    LeaderTask::stop(worker.as_ref()).await.unwrap();
    LeaderTask::start(worker.as_ref()).await; // re-elected
    LeaderTask::enqueue(worker.as_ref(), &outcome()).unwrap();
    let second = drained(&worker, Duration::from_secs(5)).await;
    println!("second term: queue drained = {second}, queued = {}", worker.queued());
    assert!(!second, "the restarted task exited at once on the old cancelled token");
}

#[test]
fn database_password_in_debug() {
    let mut values = std::collections::BTreeMap::new();
    values.insert(
        "FLAKERADAR_DATABASE_URL".to_owned(),
        "postgresql://flakeradar:s3cret-password@db:5432/flakeradar".to_owned(),
    );
    let settings = Settings::from_map(&values).unwrap();
    let printed = format!("{settings:?}");
    println!("Settings Debug contains the password: {}", printed.contains("s3cret-password"));
    assert!(printed.contains("s3cret-password"));

    let lock = AdvisoryLock::new(settings.database_url.clone(), DbSettings::default());
    let printed = format!("{lock:?}");
    println!("AdvisoryLock Debug: {printed}");
    assert!(printed.contains("s3cret-password"));
}
