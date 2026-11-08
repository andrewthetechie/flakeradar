//! The SeaORM spike: every hard query the later tasks need, proven against a
//! real database.
//!
//! Each query shows whether the builder expresses it readably, or whether it
//! needs a raw `Statement`.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when the query it just wrote returns the wrong rows"
)]
use chrono::{Duration, Utc};
use flakeradar_db::entities::{reports, test_cases, test_score_history};
use flakeradar_db::{DbSettings, connect_session};
use flakeradar_test_support::{
    TestDb, make_execution, make_job, make_job_execution, make_pipeline, make_project, make_run,
    make_test_case, make_test_score,
};
use sea_orm::sea_query::{
    Alias, Expr, ExprTrait, LockBehavior, LockType, OnConflict, Order, Query, SelectExprTrait,
    WindowStatement,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, DbBackend,
    EntityTrait, QueryFilter, QueryOrder, QuerySelect, QueryTrait, Set, Statement,
    TransactionTrait,
};

/// A pending JUnit Report for `project`.
async fn pending_report(
    db: &DatabaseConnection,
    project: &flakeradar_db::entities::projects::Model,
) -> reports::Model {
    reports::ActiveModel {
        repo_id: Set(project.repo_id),
        project_id: Set(Some(project.id)),
        commit_sha: Set("sha1".to_owned()),
        branch: Set("main".to_owned()),
        body: Set(b"<testsuites/>".to_vec()),
        created_at: Set(Utc::now().fixed_offset()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("a pending Report")
}

/// Runs one raw statement and returns its rows.
async fn raw_rows(
    db: &impl ConnectionTrait,
    sql: &str,
    values: Vec<sea_orm::Value>,
) -> Vec<sea_orm::QueryResult> {
    db.query_all_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .await
    .expect("the raw statement runs")
}

/// `values` as a PostgreSQL `integer[]`, which a chunk of ids binds as.
fn int_array(values: Vec<i32>) -> sea_orm::Value {
    sea_orm::Value::Array(
        sea_orm::sea_query::ArrayType::Int,
        Some(Box::new(
            values
                .into_iter()
                .map(|id| sea_orm::Value::Int(Some(id)))
                .collect(),
        )),
    )
}

/// Whether the connection took the advisory lock `key`.
async fn try_lock(db: &impl ConnectionTrait, key: i64) -> bool {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT pg_try_advisory_lock($1) AS locked",
            [key.into()],
        ))
        .await
        .expect("the lock query runs")
        .expect("one row");
    row.try_get("", "locked").expect("a flag")
}

/// Releases the advisory lock `key` on the connection that holds it.
async fn unlock(db: &impl ConnectionTrait, key: i64) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT pg_advisory_unlock($1)",
        [key.into()],
    ))
    .await
    .expect("the lock is released");
}

#[tokio::test]
async fn claim_next_report_locks_one_pending_report_and_skips_locked_ones() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = make_project().create(&db).await;
    let first = pending_report(db.connection(), &project).await;
    let second = pending_report(db.connection(), &project).await;

    let claim = || {
        reports::Entity::find()
            .filter(reports::Column::Status.eq("pending"))
            .order_by_asc(reports::Column::Id)
            .limit(1)
            .lock_with_behavior(LockType::Update, LockBehavior::SkipLocked)
    };

    let transaction = db.connection().begin().await.expect("a transaction");
    let claimed_inside = claim()
        .one(&transaction)
        .await
        .expect("the claim runs")
        .expect("a pending Report");
    assert_eq!(claimed_inside.id, first.id);

    let claimed_elsewhere = claim()
        .one(db.connection())
        .await
        .expect("the second claim runs")
        .expect("the next pending Report");
    assert_eq!(
        claimed_elsewhere.id, second.id,
        "the locked Report is skipped, not waited for"
    );
    transaction.rollback().await.expect("the transaction ends");
}

#[tokio::test]
async fn the_newest_window_ids_per_test_need_a_lateral_join() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = make_project().create(&db).await;
    let test_case = make_test_case(&project).create(&db).await;
    let run = make_run(&project).create(&db).await;
    let mut ids = Vec::new();
    for _ in 0..5 {
        ids.push(make_execution(&test_case, &run).create(&db).await.id);
    }

    let rows = raw_rows(
        db.connection(),
        "SELECT w.test_case_id, ids.id \
         FROM unnest($1::int[]) AS w(test_case_id) \
         CROSS JOIN LATERAL ( \
             SELECT te.id FROM test_executions te \
             WHERE te.test_case_id = w.test_case_id \
             ORDER BY te.id DESC LIMIT $2 \
         ) AS ids",
        vec![int_array(vec![test_case.id]), 3_i32.into()],
    )
    .await;

    let mut found: Vec<i64> = rows
        .iter()
        .map(|row| row.try_get::<i64>("", "id").expect("an id"))
        .collect();
    found.sort_unstable();
    let mut newest: Vec<i64> = ids.iter().rev().take(3).copied().collect();
    newest.sort_unstable();
    assert_eq!(found, newest);
}

#[tokio::test]
async fn the_newest_window_ids_per_job_need_a_lateral_join() {
    let db = TestDb::new().await.expect("a migrated test database");
    let pipeline = make_pipeline().create(&db).await;
    let job = make_job(&pipeline).create(&db).await;
    let mut ids = Vec::new();
    for attempt in 0..4 {
        ids.push(
            make_job_execution(&job)
                .ci_job_id(&format!("{attempt}"))
                .create(&db)
                .await
                .id,
        );
    }

    let rows = raw_rows(
        db.connection(),
        "SELECT w.job_id, ids.id \
         FROM unnest($1::int[]) AS w(job_id) \
         CROSS JOIN LATERAL ( \
             SELECT je.id FROM job_executions je \
             WHERE je.job_id = w.job_id \
             ORDER BY je.id DESC LIMIT $2 \
         ) AS ids",
        vec![int_array(vec![job.id]), 2_i32.into()],
    )
    .await;

    let mut found: Vec<i64> = rows
        .iter()
        .map(|row| row.try_get::<i64>("", "id").expect("an id"))
        .collect();
    found.sort_unstable();
    let mut newest: Vec<i64> = ids.iter().rev().take(2).copied().collect();
    newest.sort_unstable();
    assert_eq!(found, newest);
}

#[tokio::test]
async fn past_scores_use_a_row_number_window() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = make_project().create(&db).await;
    let test_case = make_test_case(&project).create(&db).await;
    let today = Utc::now().date_naive();
    for (days_ago, score) in [(20, 0.5), (10, 0.4), (1, 0.9)] {
        make_test_score(&test_case, today - Duration::days(days_ago), score)
            .create(&db)
            .await;
    }
    let cutoff = today - Duration::days(7);

    let mut window = WindowStatement::partition_by(test_score_history::Column::TestCaseId);
    window.order_by(test_score_history::Column::Day, Order::Desc);
    let ranked = test_score_history::Entity::find()
        .select_only()
        .exprs([
            Expr::col(test_score_history::Column::TestCaseId).alias("entity_id"),
            Expr::col(test_score_history::Column::FlakinessScore).alias("score"),
            Expr::cust("ROW_NUMBER()").over(window).alias("rn"),
        ])
        .filter(test_score_history::Column::TestCaseId.eq(test_case.id))
        .filter(test_score_history::Column::Day.lte(cutoff))
        .into_query();

    let mut outer = Query::select();
    outer
        .column(Alias::new("entity_id"))
        .column(Alias::new("score"))
        .from_subquery(ranked, Alias::new("ranked"))
        .and_where(Expr::col(Alias::new("rn")).eq(1));
    let rows = db
        .connection()
        .query_all_raw(DbBackend::Postgres.build(&outer))
        .await
        .expect("the window query runs");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].try_get::<f64>("", "score").ok(), Some(0.4));
}

#[tokio::test]
async fn newest_history_before_feeds_a_filtered_paged_leaderboard() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = make_project().create(&db).await;
    let today = Utc::now().date_naive();
    let mut flaky = Vec::new();
    for name in ["stable", "was_flaky", "new"] {
        let test_case = make_test_case(&project).name(name).create(&db).await;
        flaky.push(test_case.id);
    }
    // The first Test was stable a week ago and the second was flaky then; the
    // third has no history that old, so its row is NULL.
    make_test_score(
        &test_case_of(&db, flaky[0]).await,
        today - Duration::days(30),
        0.0,
    )
    .create(&db)
    .await;
    make_test_score(
        &test_case_of(&db, flaky[1]).await,
        today - Duration::days(30),
        0.2,
    )
    .create(&db)
    .await;

    let sql = "SELECT t.id, h.score FROM test_cases t \
               LEFT JOIN LATERAL ( \
                   SELECT h.flakiness_score AS score FROM test_score_history h \
                   WHERE h.test_case_id = t.id AND h.day <= $1 \
                   ORDER BY h.day DESC LIMIT 1 \
               ) AS h ON TRUE \
               WHERE h.score IS NULL OR h.score < $2 \
               ORDER BY t.id LIMIT $3 OFFSET $4";
    let values = vec![
        (today - Duration::days(7)).into(),
        0.1_f64.into(),
        10_i64.into(),
        0_i64.into(),
    ];
    let rows = raw_rows(db.connection(), sql, values).await;

    let found: Vec<i32> = rows
        .iter()
        .map(|row| row.try_get::<i32>("", "id").expect("an id"))
        .collect();
    assert_eq!(found, [flaky[0], flaky[2]]);

    let count_sql = sql
        .replace("SELECT t.id, h.score", "SELECT count(*) AS id")
        .replace("ORDER BY t.id LIMIT $3 OFFSET $4", "");
    let counted = raw_rows(
        db.connection(),
        &count_sql,
        vec![(today - Duration::days(7)).into(), 0.1_f64.into()],
    )
    .await;
    assert_eq!(counted[0].try_get::<i64>("", "id").ok(), Some(2));
}

#[tokio::test]
async fn upsert_history_accumulates_through_on_conflict() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = make_project().create(&db).await;
    let test_case = make_test_case(&project).create(&db).await;
    let today = Utc::now().date_naive();
    let conflict = OnConflict::columns([
        test_score_history::Column::TestCaseId,
        test_score_history::Column::Day,
    ])
    .value(
        test_score_history::Column::FlakinessScore,
        Expr::cust("EXCLUDED.flakiness_score"),
    )
    .value(
        test_score_history::Column::ConfirmedFlakeCount,
        Expr::cust("EXCLUDED.confirmed_flake_count"),
    )
    .value(
        test_score_history::Column::Executions,
        // The column is qualified because an unqualified one in `DO UPDATE` is
        // ambiguous between the stored row and `EXCLUDED`.
        Expr::col((
            test_score_history::Entity,
            test_score_history::Column::Executions,
        ))
        .add(Expr::cust("EXCLUDED.executions")),
    )
    .value(
        test_score_history::Column::Failures,
        Expr::col((
            test_score_history::Entity,
            test_score_history::Column::Failures,
        ))
        .add(Expr::cust("EXCLUDED.failures")),
    )
    .to_owned();

    for (score, confirmed, executions, failures) in [(0.5, 1, 2, 1), (0.6, 2, 3, 1)] {
        test_score_history::Entity::insert(test_score_history::ActiveModel {
            test_case_id: Set(test_case.id),
            day: Set(today),
            flakiness_score: Set(score),
            confirmed_flake_count: Set(confirmed),
            executions: Set(executions),
            failures: Set(failures),
        })
        .on_conflict(conflict.clone())
        .exec(db.connection())
        .await
        .expect("the upsert runs");
    }

    let stored = test_score_history::Entity::find_by_id((test_case.id, today))
        .one(db.connection())
        .await
        .expect("the row is readable")
        .expect("the row exists");
    assert_eq!(stored.flakiness_score, 0.6);
    assert_eq!(stored.confirmed_flake_count, 2);
    assert_eq!(stored.executions, 5);
    assert_eq!(stored.failures, 2);
}

#[tokio::test]
async fn a_bulk_update_by_primary_key_sets_optional_columns_per_row() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = make_project().create(&db).await;
    let first = make_test_case(&project).name("first").create(&db).await;
    let second = make_test_case(&project).name("second").create(&db).await;
    let untouched = make_test_case(&project).name("untouched").create(&db).await;

    let statement = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE test_cases AS t \
         SET file = v.file, line = v.line, flakiness_score = v.score \
         FROM (VALUES ($1::int, $2::text, $3::int, $4::double precision), \
                      ($5::int, $6::text, $7::int, $8::double precision)) \
              AS v(id, file, line, score) \
         WHERE t.id = v.id",
        vec![
            first.id.into(),
            "tests/first.py".into(),
            10_i32.into(),
            0.25_f64.into(),
            second.id.into(),
            sea_orm::Value::String(None),
            20_i32.into(),
            0.5_f64.into(),
        ],
    );
    db.connection()
        .execute_raw(statement)
        .await
        .expect("the bulk update runs");

    let first = test_cases::Entity::find_by_id(first.id)
        .one(db.connection())
        .await
        .expect("readable")
        .expect("the row exists");
    let second = test_cases::Entity::find_by_id(second.id)
        .one(db.connection())
        .await
        .expect("readable")
        .expect("the row exists");
    let untouched = test_cases::Entity::find_by_id(untouched.id)
        .one(db.connection())
        .await
        .expect("readable")
        .expect("the row exists");
    assert_eq!(first.file.as_deref(), Some("tests/first.py"));
    assert_eq!(first.line, Some(10));
    assert_eq!(second.file, None);
    assert_eq!(second.line, Some(20));
    assert_eq!(second.flakiness_score, 0.5);
    assert_eq!(untouched.file, None);
    assert_eq!(untouched.flakiness_score, 0.0);
}

#[tokio::test]
async fn deleting_a_history_batch_uses_a_tuple_in_a_limited_subquery() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = make_project().create(&db).await;
    let test_case = make_test_case(&project).create(&db).await;
    let today = Utc::now().date_naive();
    for days_ago in [30, 20, 10, 1] {
        make_test_score(&test_case, today - Duration::days(days_ago), 0.1)
            .create(&db)
            .await;
    }
    let cutoff = today - Duration::days(7);

    let subquery = Query::select()
        .column(test_score_history::Column::TestCaseId)
        .column(test_score_history::Column::Day)
        .from(test_score_history::Entity)
        .and_where(test_score_history::Column::Day.lt(cutoff))
        .order_by(test_score_history::Column::Day, Order::Asc)
        .limit(2)
        .to_owned();
    let tuple = [
        Expr::col(test_score_history::Column::TestCaseId),
        Expr::col(test_score_history::Column::Day),
    ];
    let deleted = test_score_history::Entity::delete_many()
        .filter(Expr::tuple(tuple).in_subquery(subquery))
        .exec(db.connection())
        .await
        .expect("the batch delete runs")
        .rows_affected;
    assert_eq!(deleted, 2);
    assert_eq!(
        test_score_history::Entity::find()
            .all(db.connection())
            .await
            .expect("the remaining rows are readable")
            .len(),
        2
    );
}

const WORKER_KEY: i64 = 726_300_002;

#[tokio::test]
async fn one_dedicated_connection_holds_the_worker_lock() {
    let db = TestDb::new().await.expect("a migrated test database");
    // A pool of exactly one connection is the dedicated connection the worker
    // holds: every statement goes to that one session, so the lock survives and
    // the unlock reaches the session that took it.
    let leader = connect_session(db.url(), DbSettings::default())
        .await
        .expect("the leader's session");
    assert!(try_lock(&leader, WORKER_KEY).await);
    assert!(
        !try_lock(db.connection(), WORKER_KEY).await,
        "a second replica cannot take the lock"
    );

    unlock(&leader, WORKER_KEY).await;
    assert!(
        try_lock(db.connection(), WORKER_KEY).await,
        "the lock is free after the leader releases it"
    );
    unlock(db.connection(), WORKER_KEY).await;
}

/// The Test row with `id`.
async fn test_case_of(db: &TestDb, id: i32) -> test_cases::Model {
    test_cases::Entity::find_by_id(id)
        .one(db.connection())
        .await
        .expect("readable")
        .expect("the row exists")
}
