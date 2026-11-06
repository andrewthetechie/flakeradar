//! Score history (ADR 0007): the daily snapshot write, and the reads from it.
//!
//! The behaviour is the frozen Python server's `test_score_history.py`: the
//! write replaces the score and Proven flake count and adds the day's counts,
//! and the past-score lookup and the per-row lateral condition must agree. The
//! trend cases are pure and live with `trend_for` in `flakeradar-core`.

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when a query misbehaves"
)]

use chrono::{Duration, NaiveDate};
use flakeradar_core::EntityKind;
use flakeradar_core::TREND_DAYS;
use flakeradar_db::entities::{job_score_history, pipelines, test_score_history};
use flakeradar_db::{
    HISTORY_DAYS, HistoryUpdate, newest_history_before, past_scores, window_rows,
    write_today_history,
};
use flakeradar_test_support::{
    TestDb, make_job, make_job_score, make_pipeline, make_project, make_test_case, make_test_score,
};
use sea_orm::{ConnectionTrait, EntityTrait};

/// The fixed "today" the tests write and read, so a cut-off is exact.
fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 20).expect("a fixed day")
}

/// One `write_today_history` update.
fn update(id: i32, score: f64, confirmed: i32, counts: (i32, i32)) -> HistoryUpdate {
    HistoryUpdate {
        id,
        flakiness_score: score,
        confirmed_flake_count: confirmed,
        executions: counts.0,
        failures: counts.1,
    }
}

#[tokio::test]
async fn write_today_history_replaces_score_and_adds_counts_for_a_test() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .build(db.connection())
        .await
        .expect("a Project");
    let test_case = make_test_case(&project)
        .build(db.connection())
        .await
        .expect("a Test");
    write_today_history(
        db.connection(),
        EntityKind::Test,
        &[update(test_case.id, 0.5, 2, (3, 1))],
        today(),
    )
    .await
    .expect("the first write");
    write_today_history(
        db.connection(),
        EntityKind::Test,
        &[update(test_case.id, 0.25, 4, (2, 1))],
        today(),
    )
    .await
    .expect("the second write");
    let rows = test_score_history::Entity::find()
        .all(db.connection())
        .await
        .expect("the history");
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.day, today());
    assert_eq!(row.flakiness_score, 0.25); // replaced, not averaged
    assert_eq!(row.confirmed_flake_count, 4);
    assert_eq!((row.executions, row.failures), (5, 2)); // added
}

#[tokio::test]
async fn write_today_history_replaces_score_and_adds_counts_for_a_job() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline = make_pipeline()
        .build(db.connection())
        .await
        .expect("a Pipeline");
    let job = make_job(&pipeline)
        .build(db.connection())
        .await
        .expect("a Job");
    write_today_history(
        db.connection(),
        EntityKind::Job,
        &[update(job.id, 0.5, 2, (3, 1))],
        today(),
    )
    .await
    .expect("the first write");
    write_today_history(
        db.connection(),
        EntityKind::Job,
        &[update(job.id, 0.25, 4, (2, 1))],
        today(),
    )
    .await
    .expect("the second write");
    let rows = job_score_history::Entity::find()
        .all(db.connection())
        .await
        .expect("the history");
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.day, today());
    assert_eq!(row.flakiness_score, 0.25);
    assert_eq!(row.confirmed_flake_count, 4);
    assert_eq!((row.executions, row.failures), (5, 2));
}

#[tokio::test]
async fn write_today_history_defaults_missing_counts_to_zero() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .build(db.connection())
        .await
        .expect("a Project");
    let no_counts = make_test_case(&project)
        .name("no_day_counts")
        .build(db.connection())
        .await
        .expect("a Test");
    write_today_history(
        db.connection(),
        EntityKind::Test,
        &[update(no_counts.id, 0.75, 1, (0, 0))],
        today(),
    )
    .await
    .expect("the write");
    let rows = test_score_history::Entity::find()
        .all(db.connection())
        .await
        .expect("the history");
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].executions, rows[0].failures), (0, 0));
    assert_eq!(rows[0].flakiness_score, 0.75);
    assert_eq!(rows[0].confirmed_flake_count, 1);

    // A same-day rescore with no counts rewrites the day's score without adding counts.
    write_today_history(
        db.connection(),
        EntityKind::Test,
        &[update(no_counts.id, 0.1, 0, (0, 0))],
        today(),
    )
    .await
    .expect("the second write");
    let rows = test_score_history::Entity::find()
        .all(db.connection())
        .await
        .expect("the history");
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].executions, rows[0].failures), (0, 0));
    assert_eq!(rows[0].flakiness_score, 0.1);
    assert_eq!(rows[0].confirmed_flake_count, 0);
}

#[tokio::test]
async fn a_same_day_rescore_with_no_counts_adds_nothing() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .build(db.connection())
        .await
        .expect("a Project");
    let covered = make_test_case(&project)
        .name("covered")
        .build(db.connection())
        .await
        .expect("a Test");
    let uncovered = make_test_case(&project)
        .name("uncovered")
        .build(db.connection())
        .await
        .expect("a Test");
    write_today_history(
        db.connection(),
        EntityKind::Test,
        &[
            update(covered.id, 0.2, 0, (2, 1)),
            update(uncovered.id, 0.4, 1, (0, 0)),
        ],
        today(),
    )
    .await
    .expect("the write");
    let by_id = |rows: Vec<test_score_history::Model>| {
        rows.into_iter()
            .map(|row| (row.test_case_id, row))
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    let rows = by_id(
        test_score_history::Entity::find()
            .all(db.connection())
            .await
            .expect("the history"),
    );
    assert_eq!(
        (rows[&covered.id].executions, rows[&covered.id].failures),
        (2, 1)
    );
    assert_eq!(
        (rows[&uncovered.id].executions, rows[&uncovered.id].failures),
        (0, 0)
    );

    // The uncovered owner hits the conflict path too: its counts stay (0, 0), its score is replaced.
    write_today_history(
        db.connection(),
        EntityKind::Test,
        &[update(uncovered.id, 0.6, 3, (0, 0))],
        today(),
    )
    .await
    .expect("the fourth write");
    let rows = by_id(
        test_score_history::Entity::find()
            .all(db.connection())
            .await
            .expect("the history"),
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(
        (rows[&uncovered.id].executions, rows[&uncovered.id].failures),
        (0, 0)
    );
    assert_eq!(rows[&uncovered.id].flakiness_score, 0.6);
    assert_eq!(rows[&uncovered.id].confirmed_flake_count, 3);
}

#[tokio::test]
async fn the_history_upsert_keys_the_conflict_on_owner_and_day() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .build(db.connection())
        .await
        .expect("a Project");
    let test_case = make_test_case(&project)
        .build(db.connection())
        .await
        .expect("a Test");
    let yesterday = today() - Duration::days(1);
    make_test_score(&test_case, yesterday, 0.9)
        .build(db.connection())
        .await
        .expect("yesterday's row");
    write_today_history(
        db.connection(),
        EntityKind::Test,
        &[update(test_case.id, 0.5, 2, (3, 1))],
        today(),
    )
    .await
    .expect("today's first write");
    write_today_history(
        db.connection(),
        EntityKind::Test,
        &[update(test_case.id, 0.1, 0, (2, 1))],
        today(),
    )
    .await
    .expect("today's second write");
    let mut rows = test_score_history::Entity::find()
        .all(db.connection())
        .await
        .expect("the history");
    rows.sort_by_key(|row| std::cmp::Reverse(row.day));
    assert_eq!(rows.len(), 2);
    let (today_row, past_row) = (&rows[0], &rows[1]);
    assert_eq!(today_row.day, today());
    assert_eq!(today_row.flakiness_score, 0.1); // replaced
    assert_eq!(today_row.confirmed_flake_count, 0);
    assert_eq!((today_row.executions, today_row.failures), (5, 2)); // added
    assert_eq!(past_row.day, yesterday);
    assert_eq!(past_row.flakiness_score, 0.9); // a different day is untouched
    assert_eq!(past_row.confirmed_flake_count, 0);
    assert_eq!((past_row.executions, past_row.failures), (0, 0));
}

/// Each owner's newest history at least `TREND_DAYS` old, read as the per-row
/// condition the listing queries use, keyed by owner.
async fn lateral_scores(
    db: &TestDb,
    kind: EntityKind,
    table: &str,
    ids: &[i32],
    today: NaiveDate,
) -> std::collections::BTreeMap<i32, Option<f64>> {
    let condition = newest_history_before(kind, "o");
    let sql = format!(
        "SELECT o.id, ({condition}) AS score FROM {table} o \
          WHERE o.id = ANY($2::INTEGER[]) ORDER BY o.id"
    );
    let values = vec![
        sea_orm::Value::from(today - Duration::days(TREND_DAYS)),
        sea_orm::Value::Array(
            sea_orm::sea_query::ArrayType::Int,
            Some(Box::new(
                ids.iter().copied().map(sea_orm::Value::from).collect(),
            )),
        ),
    ];
    let rows = db
        .connection()
        .query_all_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            sql,
            values,
        ))
        .await
        .expect("the lateral scores");
    rows.into_iter()
        .map(|row| {
            (
                row.try_get::<i32>("", "id").expect("the id"),
                row.try_get::<Option<f64>>("", "score").expect("the score"),
            )
        })
        .collect()
}

#[tokio::test]
async fn score_history_reads_agree_for_a_test() {
    let db = TestDb::new().await.expect("a test database");
    let project = make_project()
        .build(db.connection())
        .await
        .expect("a Project");
    let seasoned = make_test_case(&project)
        .name("seasoned")
        .build(db.connection())
        .await
        .expect("a Test");
    let fresh = make_test_case(&project)
        .name("fresh")
        .build(db.connection())
        .await
        .expect("a Test");
    let today = today();
    let cutoff = today - Duration::days(TREND_DAYS);

    for (day, score) in [
        (cutoff, 0.2), // exactly on the cut-off: old, and the newest old row
        (cutoff + Duration::days(1), 0.9), // one day too recent to count
        (cutoff - Duration::days(3), 0.1), // older
        (cutoff - Duration::days(10), 0.05), // oldest of the old rows
        (today - Duration::days(HISTORY_DAYS), 0.4), // outside the detail window
        (today - Duration::days(HISTORY_DAYS - 1), 0.3), // first row inside it
    ] {
        make_test_score(&seasoned, day, score)
            .build(db.connection())
            .await
            .expect("a history row");
    }
    make_test_score(&fresh, today, 0.6)
        .build(db.connection())
        .await
        .expect("a history row");

    let ids = [seasoned.id, fresh.id];
    let past = past_scores(db.connection(), EntityKind::Test, &ids, today)
        .await
        .expect("the past scores");
    assert_eq!(past, std::collections::BTreeMap::from([(seasoned.id, 0.2)]));

    let lateral = lateral_scores(&db, EntityKind::Test, "test_cases", &ids, today).await;
    assert_eq!(
        lateral,
        [(seasoned.id, Some(0.2)), (fresh.id, None)]
            .into_iter()
            .collect()
    );
    // The two strategies agree owner by owner.
    assert_eq!(
        lateral
            .keys()
            .map(|owner| (*owner, past.get(owner).copied()))
            .collect::<std::collections::BTreeMap<_, _>>(),
        lateral
    );

    let rows = window_rows(db.connection(), EntityKind::Test, seasoned.id, today)
        .await
        .expect("the detail window");
    assert_eq!(
        rows.iter()
            .map(|row| (row.day, row.flakiness_score))
            .collect::<Vec<_>>(),
        vec![
            (today - Duration::days(HISTORY_DAYS - 1), 0.3),
            (cutoff - Duration::days(10), 0.05),
            (cutoff - Duration::days(3), 0.1),
            (cutoff, 0.2),
            (cutoff + Duration::days(1), 0.9),
        ] // the row exactly on the window edge is out; oldest first
    );
    assert_eq!(
        window_rows(db.connection(), EntityKind::Test, fresh.id, today)
            .await
            .expect("the detail window")
            .iter()
            .map(|row| row.day)
            .collect::<Vec<_>>(),
        vec![today]
    );
}

#[tokio::test]
async fn score_history_reads_agree_for_a_job() {
    let db = TestDb::new().await.expect("a test database");
    let pipeline: pipelines::Model = make_pipeline()
        .build(db.connection())
        .await
        .expect("a Pipeline");
    let seasoned = make_job(&pipeline)
        .name("seasoned")
        .build(db.connection())
        .await
        .expect("a Job");
    let fresh = make_job(&pipeline)
        .name("fresh")
        .build(db.connection())
        .await
        .expect("a Job");
    let today = today();
    let cutoff = today - Duration::days(TREND_DAYS);

    for (day, score) in [
        (cutoff, 0.2),
        (cutoff + Duration::days(1), 0.9),
        (cutoff - Duration::days(3), 0.1),
        (cutoff - Duration::days(10), 0.05),
        (today - Duration::days(HISTORY_DAYS), 0.4),
        (today - Duration::days(HISTORY_DAYS - 1), 0.3),
    ] {
        make_job_score(&seasoned, day, score)
            .build(db.connection())
            .await
            .expect("a history row");
    }
    make_job_score(&fresh, today, 0.6)
        .build(db.connection())
        .await
        .expect("a history row");

    let ids = [seasoned.id, fresh.id];
    let past = past_scores(db.connection(), EntityKind::Job, &ids, today)
        .await
        .expect("the past scores");
    assert_eq!(past, std::collections::BTreeMap::from([(seasoned.id, 0.2)]));

    let lateral = lateral_scores(&db, EntityKind::Job, "jobs", &ids, today).await;
    assert_eq!(
        lateral,
        [(seasoned.id, Some(0.2)), (fresh.id, None)]
            .into_iter()
            .collect()
    );
    assert_eq!(
        lateral
            .keys()
            .map(|owner| (*owner, past.get(owner).copied()))
            .collect::<std::collections::BTreeMap<_, _>>(),
        lateral
    );

    let rows = window_rows(db.connection(), EntityKind::Job, seasoned.id, today)
        .await
        .expect("the detail window");
    assert_eq!(
        rows.iter()
            .map(|row| (row.day, row.flakiness_score))
            .collect::<Vec<_>>(),
        vec![
            (today - Duration::days(HISTORY_DAYS - 1), 0.3),
            (cutoff - Duration::days(10), 0.05),
            (cutoff - Duration::days(3), 0.1),
            (cutoff, 0.2),
            (cutoff + Duration::days(1), 0.9),
        ]
    );
    assert_eq!(
        window_rows(db.connection(), EntityKind::Job, fresh.id, today)
            .await
            .expect("the detail window")
            .iter()
            .map(|row| row.day)
            .collect::<Vec<_>>(),
        vec![today]
    );
}
