//! The Issue link routes: link and unlink a GitHub issue on a Test or a Job.
//!
//! Ported from `backend/tests/test_issue_links_api.py` and
//! `backend/tests/test_issue_links_lifecycle.py`: the response shapes, the rules
//! that refuse a request, and the state the pair leaves behind.

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when a fixture or an answer does not behave"
)]

use axum::http::{Method, StatusCode};
use flakeradar_core::EntityKind;
use flakeradar_test_support::{
    Reply, TestDb, app, get, make_job, make_pipeline, make_project, make_test_case, post, request,
    request_with_headers, send, settings, system_clock,
};

use sea_orm::ConnectionTrait as _;
use sea_orm::DbBackend;
use sea_orm::Statement;
use serde_json::{Value, json};

/// The token every write route needs.
const TOKEN: &str = "test-token";

/// A request body header pair.
fn json_headers() -> [(&'static str, &'static str); 2] {
    [("X-API-Key", TOKEN), ("Content-Type", "application/json")]
}

/// Sends one `PUT` with a JSON body and the token.
async fn put(app: &axum::Router, uri: &str, body: &Value) -> Reply {
    let mut builder = axum::http::Request::builder().method(Method::PUT).uri(uri);
    for (name, value) in json_headers() {
        builder = builder.header(name, value);
    }
    send(
        app,
        builder
            .body(axum::body::Body::from(body.to_string()))
            .expect("the fixture request builds"),
    )
    .await
}

/// Sends one `DELETE` with the token.
async fn delete(app: &axum::Router, uri: &str) -> Reply {
    request_with_headers(app, Method::DELETE, uri, &[("X-API-Key", TOKEN)]).await
}

/// The `github_issue_*` columns of one Test, as the database stores them.
async fn test_link(db: &TestDb, id: i32) -> (Option<i32>, Option<bool>, bool, bool, bool) {
    let row = db
        .connection()
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT github_issue_number, github_issue_filed, github_issue_checked_at IS NOT NULL \
             AS checked, github_issue_closed, github_issue_fixed_noted_at IS NOT NULL AS noted \
             FROM test_cases WHERE id = $1",
            [id.into()],
        ))
        .await
        .expect("the row reads")
        .expect("the Test exists");
    (
        row.try_get("", "github_issue_number").expect("a number"),
        row.try_get("", "github_issue_filed").expect("filed"),
        row.try_get("", "checked").expect("checked"),
        row.try_get("", "github_issue_closed").expect("closed"),
        row.try_get("", "noted").expect("noted"),
    )
}

/// A Test whose Repo names the Project, so the derived issue url is `acme/app`.
async fn one_test(db: &TestDb) -> flakeradar_db::entities::test_cases::Model {
    let project = make_project()
        .repo("acme/app")
        .project("default")
        .build(db.connection())
        .await
        .expect("the Project inserts");
    make_test_case(&project)
        .name("test_total")
        .classname("tests.test_cart")
        .build(db.connection())
        .await
        .expect("the Test inserts")
}

/// A Job whose Pipeline is GitHub's, so a GitHub issue can be linked to it.
async fn one_job(db: &TestDb) -> flakeradar_db::entities::jobs::Model {
    let pipeline = make_pipeline()
        .repo("acme/app")
        .provider("github")
        .build(db.connection())
        .await
        .expect("the Pipeline inserts");
    make_job(&pipeline)
        .name("unit")
        .build(db.connection())
        .await
        .expect("the Job inserts")
}

/// The link columns of one Job, with its exact check stamp.
async fn job_link(
    db: &TestDb,
    id: i32,
) -> (
    Option<i32>,
    Option<bool>,
    Option<chrono::DateTime<chrono::FixedOffset>>,
    bool,
    bool,
) {
    let row = db
        .connection()
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT github_issue_number, github_issue_filed, github_issue_checked_at, \
             github_issue_closed, github_issue_fixed_noted_at IS NOT NULL AS noted \
             FROM jobs WHERE id = $1",
            [id.into()],
        ))
        .await
        .expect("the row reads")
        .expect("the Job exists");
    (
        row.try_get("", "github_issue_number").expect("a number"),
        row.try_get("", "github_issue_filed").expect("filed"),
        row.try_get("", "github_issue_checked_at").expect("a stamp"),
        row.try_get("", "github_issue_closed").expect("closed"),
        row.try_get("", "noted").expect("noted"),
    )
}

#[tokio::test]
async fn linking_a_test_issue_stores_the_pair_and_the_url() {
    let db = TestDb::new().await.expect("a test database");
    let test = one_test(&db).await;
    let app = app(&db, &[("FLAKERADAR_API_TOKEN", TOKEN)]);

    let reply = put(
        &app,
        &format!("/api/tests/{}/issue", test.id),
        &json!({"number": 12}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    let body = reply.json();
    assert_eq!(body["github_issue_number"], json!(12));
    assert_eq!(
        body["github_issue_url"],
        json!("https://github.com/acme/app/issues/12")
    );
    assert_eq!(body["github_issue_closed"], json!(false));

    // A front-door link is not FlakeRadar's own, so the admin delete never closes
    // it, and a new link is checked first.
    let (number, filed, checked, closed, noted) = test_link(&db, test.id).await;
    assert_eq!(number, Some(12));
    assert_eq!(filed, Some(false));
    assert!(!checked);
    assert!(!closed);
    assert!(!noted);

    let history = get(&app, &format!("/api/tests/{}/history", test.id)).await;
    assert_eq!(history.json()["test"]["github_issue_number"], json!(12));
    assert_eq!(history.json()["test"]["github_issue_closed"], json!(false));
}

#[tokio::test]
async fn the_same_number_twice_keeps_the_check_stamp() {
    let db = TestDb::new().await.expect("a test database");
    let test = one_test(&db).await;
    let app = app(&db, &[("FLAKERADAR_API_TOKEN", TOKEN)]);

    put(
        &app,
        &format!("/api/tests/{}/issue", test.id),
        &json!({"number": 12}),
    )
    .await;
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE test_cases SET github_issue_checked_at = now() WHERE id = $1",
            [test.id.into()],
        ))
        .await
        .expect("the stamp writes");

    let reply = put(
        &app,
        &format!("/api/tests/{}/issue", test.id),
        &json!({"number": 12}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.json()["github_issue_number"], json!(12));
    let (_, _, checked, _, _) = test_link(&db, test.id).await;
    assert!(
        checked,
        "the same number was just checked, so its stamp stays"
    );

    let replaced = put(
        &app,
        &format!("/api/tests/{}/issue?replace=true", test.id),
        &json!({"number": 44}),
    )
    .await;
    assert_eq!(replaced.status, StatusCode::OK);
    let (number, _, checked, _, _) = test_link(&db, test.id).await;
    assert_eq!(number, Some(44));
    assert!(!checked, "a different number starts checked-first again");
}

#[tokio::test]
async fn re_linking_a_declined_issue_reports_the_stored_closed_flag() {
    // GitHub reported the issue closed as not planned, so the link was kept and the
    // row flagged: a second link of the same number must answer that flag, not a
    // hard-coded false, or a client is told the filer will speak again.
    let db = TestDb::new().await.expect("a test database");
    let test = one_test(&db).await;
    let app = app(&db, &[("FLAKERADAR_API_TOKEN", TOKEN)]);
    put(
        &app,
        &format!("/api/tests/{}/issue", test.id),
        &json!({"number": 12}),
    )
    .await;
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE test_cases SET github_issue_checked_at = now(), github_issue_closed = TRUE \
             WHERE id = $1",
            [test.id.into()],
        ))
        .await
        .expect("the declined flag writes");

    let reply = put(
        &app,
        &format!("/api/tests/{}/issue", test.id),
        &json!({"number": 12}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        reply.json()["github_issue_closed"],
        json!(true),
        "the answer reports the stored flag"
    );
    let (_, _, _, closed, _) = test_link(&db, test.id).await;
    assert!(closed, "the stored flag is untouched by the re-link");
}

#[tokio::test]
async fn a_different_number_without_replace_is_a_conflict() {
    let db = TestDb::new().await.expect("a test database");
    let test = one_test(&db).await;
    let app = app(&db, &[("FLAKERADAR_API_TOKEN", TOKEN)]);

    put(
        &app,
        &format!("/api/tests/{}/issue", test.id),
        &json!({"number": 12}),
    )
    .await;
    let reply = put(
        &app,
        &format!("/api/tests/{}/issue", test.id),
        &json!({"number": 13}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert_eq!(
        reply.json()["detail"],
        json!("Test already links issue #12; pass replace=true to replace it")
    );
    assert_eq!(test_link(&db, test.id).await.0, Some(12));

    let replaced = put(
        &app,
        &format!("/api/tests/{}/issue?replace=true", test.id),
        &json!({"number": 13}),
    )
    .await;
    assert_eq!(replaced.status, StatusCode::OK);
    assert_eq!(replaced.json()["github_issue_number"], json!(13));
    assert_eq!(test_link(&db, test.id).await.0, Some(13));
}

#[tokio::test]
async fn replace_stores_the_new_number_and_resets_everything_that_went_with_the_old_one() {
    let db = TestDb::new().await.expect("a test database");
    let test = one_test(&db).await;
    let app = app(&db, &[("FLAKERADAR_API_TOKEN", TOKEN)]);

    put(
        &app,
        &format!("/api/tests/{}/issue", test.id),
        &json!({"number": 12}),
    )
    .await;
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE test_cases SET github_issue_checked_at = now(), github_issue_closed = TRUE, \
             github_issue_fixed_noted_at = now(), github_issue_filed = TRUE WHERE id = $1",
            [test.id.into()],
        ))
        .await
        .expect("the old link's flags write");

    let reply = put(
        &app,
        &format!("/api/tests/{}/issue?replace=true", test.id),
        &json!({"number": 13}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.json()["github_issue_number"], json!(13));
    let (number, filed, checked, closed, noted) = test_link(&db, test.id).await;
    assert_eq!(number, Some(13));
    assert_eq!(filed, Some(false));
    assert!(
        !checked,
        "a replaced link starts unchecked, so the sync checks it first"
    );
    assert!(!closed);
    assert!(!noted);
}

#[tokio::test]
async fn a_url_must_be_the_issue_in_the_tests_own_repo() {
    let db = TestDb::new().await.expect("a test database");
    let test = one_test(&db).await;
    let app = app(&db, &[("FLAKERADAR_API_TOKEN", TOKEN)]);

    // The trailing slash, the case and the surrounding whitespace all normalize.
    let accepted = put(
        &app,
        &format!("/api/tests/{}/issue", test.id),
        &json!({"number": 14, "url": "  HTTPS://GitHub.com/acme/app/issues/14/  "}),
    )
    .await;
    assert_eq!(accepted.status, StatusCode::OK);

    let refused = put(
        &app,
        &format!("/api/tests/{}/issue?replace=true", test.id),
        &json!({"number": 15, "url": "https://github.com/other/repo/issues/15"}),
    )
    .await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        refused.json()["detail"],
        json!(
            "url must be https://github.com/acme/app/issues/15: the issue must be in the Test's own Repo"
        )
    );

    let not_a_url = put(
        &app,
        &format!("/api/tests/{}/issue?replace=true", test.id),
        &json!({"number": 16, "url": "not a url"}),
    )
    .await;
    assert_eq!(not_a_url.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        not_a_url.json()["detail"],
        json!(
            "url must be https://github.com/acme/app/issues/16: the issue must be in the Test's own Repo"
        )
    );
    assert_eq!(
        test_link(&db, test.id).await.0,
        Some(14),
        "a refused url leaves the link alone"
    );
}

#[tokio::test]
async fn a_number_out_of_range_is_the_validation_error_pydantic_gave() {
    let db = TestDb::new().await.expect("a test database");
    let test = one_test(&db).await;
    let app = app(&db, &[("FLAKERADAR_API_TOKEN", TOKEN)]);

    let too_low = put(
        &app,
        &format!("/api/tests/{}/issue", test.id),
        &json!({"number": 0}),
    )
    .await;
    assert_eq!(too_low.status, StatusCode::UNPROCESSABLE_ENTITY);
    let detail = &too_low.json()["detail"][0];
    assert_eq!(detail["type"], json!("greater_than_equal"));
    assert_eq!(detail["loc"], json!(["body", "number"]));
    assert_eq!(detail["input"], json!(0));

    let too_high = put(
        &app,
        &format!("/api/tests/{}/issue", test.id),
        &json!({"number": 2_147_483_648_i64}),
    )
    .await;
    assert_eq!(too_high.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        too_high.json()["detail"][0]["type"],
        json!("less_than_equal")
    );

    let not_an_integer = put(
        &app,
        &format!("/api/tests/{}/issue", test.id),
        &json!({"number": "twelve"}),
    )
    .await;
    assert_eq!(not_an_integer.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        not_an_integer.json()["detail"][0]["type"],
        json!("int_parsing")
    );
    assert_eq!(
        not_an_integer.json()["detail"][0]["loc"],
        json!(["body", "number"])
    );

    for body in [
        json!({"number": -1}),
        json!({}),
        json!({"number": 1, "url": "x".repeat(2049)}),
    ] {
        let reply = put(&app, &format!("/api/tests/{}/issue", test.id), &body).await;
        assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    }
}

#[tokio::test]
async fn a_body_that_is_not_json_is_a_model_attributes_error() {
    let db = TestDb::new().await.expect("a test database");
    let test = one_test(&db).await;
    let app = app(&db, &[("FLAKERADAR_API_TOKEN", TOKEN)]);

    let reply = request_with_headers(
        &app,
        Method::PUT,
        &format!("/api/tests/{}/issue", test.id),
        &json_headers(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    let detail = &reply.json()["detail"][0];
    assert_eq!(detail["type"], json!("missing"));
    assert_eq!(detail["loc"], json!(["body"]));
}

#[tokio::test]
async fn unlinking_clears_both_halves_and_every_flag() {
    let db = TestDb::new().await.expect("a test database");
    let test = one_test(&db).await;
    let app = app(&db, &[("FLAKERADAR_API_TOKEN", TOKEN)]);

    put(
        &app,
        &format!("/api/tests/{}/issue", test.id),
        &json!({"number": 12}),
    )
    .await;
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE test_cases SET github_issue_checked_at = now(), github_issue_fixed_noted_at = now(), \
             github_issue_filed = TRUE, github_issue_closed = TRUE WHERE id = $1",
            [test.id.into()],
        ))
        .await
        .expect("the link's flags write");

    let reply = delete(&app, &format!("/api/tests/{}/issue", test.id)).await;
    assert_eq!(reply.status, StatusCode::OK);
    let body = reply.json();
    assert_eq!(body["github_issue_number"], Value::Null);
    assert_eq!(body["github_issue_url"], Value::Null);
    assert_eq!(body["github_issue_closed"], json!(false));
    let (number, filed, checked, closed, noted) = test_link(&db, test.id).await;
    assert_eq!(number, None);
    assert_eq!(filed, None, "an unlinked row carries no provenance");
    assert!(!checked);
    assert!(!closed);
    assert!(!noted);

    // A Test with no link unlinks just as well, and logs nothing.
    let again = delete(&app, &format!("/api/tests/{}/issue", test.id)).await;
    assert_eq!(again.status, StatusCode::OK);
    assert_eq!(again.json()["github_issue_number"], Value::Null);
}

#[tokio::test]
async fn an_unknown_test_or_job_is_a_404_naming_its_kind() {
    let db = TestDb::new().await.expect("a test database");
    let app = app(&db, &[("FLAKERADAR_API_TOKEN", TOKEN)]);

    let missing_test = put(&app, "/api/tests/999/issue", &json!({"number": 12})).await;
    assert_eq!(missing_test.status, StatusCode::NOT_FOUND);
    assert_eq!(missing_test.json()["detail"], json!("Test not found"));

    let missing_test_delete = delete(&app, "/api/tests/999/issue").await;
    assert_eq!(missing_test_delete.status, StatusCode::NOT_FOUND);
    assert_eq!(
        missing_test_delete.json()["detail"],
        json!("Test not found")
    );

    let missing_job = delete(&app, "/api/jobs/999/issue").await;
    assert_eq!(missing_job.status, StatusCode::NOT_FOUND);
    assert_eq!(missing_job.json()["detail"], json!("Job not found"));

    let missing_job_put = put(&app, "/api/jobs/999/issue", &json!({"number": 1})).await;
    assert_eq!(missing_job_put.status, StatusCode::NOT_FOUND);
    assert_eq!(missing_job_put.json()["detail"], json!("Job not found"));
}

#[tokio::test]
async fn only_a_github_pipelines_job_can_link_an_issue() {
    let db = TestDb::new().await.expect("a test database");
    let github = make_pipeline()
        .repo("acme/app")
        .provider("github")
        .build(db.connection())
        .await
        .expect("the Pipeline inserts");
    let job = make_job(&github)
        .name("unit")
        .build(db.connection())
        .await
        .expect("the Job inserts");
    let gitlab = make_pipeline()
        .repo("acme/app")
        .provider("gitlab")
        .name(".gitlab-ci.yml")
        .build(db.connection())
        .await
        .expect("the Pipeline inserts");
    let other = make_job(&gitlab)
        .name("deploy")
        .github_issue_number(3)
        .build(db.connection())
        .await
        .expect("the Job inserts");
    let app = app(&db, &[("FLAKERADAR_API_TOKEN", TOKEN)]);

    let linked = put(
        &app,
        &format!("/api/jobs/{}/issue", job.id),
        &json!({"number": 20}),
    )
    .await;
    assert_eq!(linked.status, StatusCode::OK);
    assert_eq!(
        linked.json()["github_issue_url"],
        json!("https://github.com/acme/app/issues/20")
    );

    let refused = put(
        &app,
        &format!("/api/jobs/{}/issue", other.id),
        &json!({"number": 22}),
    )
    .await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        refused.json()["detail"],
        json!("Only Jobs of GitHub Pipelines can link a GitHub issue")
    );
    assert_eq!(
        job_link(&db, other.id).await.0,
        Some(3),
        "the refused link leaves the Job alone"
    );

    let unlinked = delete(&app, &format!("/api/jobs/{}/issue", other.id)).await;
    assert_eq!(unlinked.status, StatusCode::OK);
    assert_eq!(unlinked.json()["github_issue_number"], Value::Null);
}

#[tokio::test]
async fn a_linked_test_is_what_the_leaderboard_and_its_filter_list() {
    let db = TestDb::new().await.expect("a test database");
    let test = one_test(&db).await;
    let app = app(&db, &[("FLAKERADAR_API_TOKEN", TOKEN)]);

    put(
        &app,
        &format!("/api/tests/{}/issue", test.id),
        &json!({"number": 12}),
    )
    .await;

    let listed = get(
        &app,
        "/api/tests?repo=acme/app&has_issue=true&include_stable=true",
    )
    .await;
    assert_eq!(listed.status, StatusCode::OK);
    assert_eq!(listed.json()["total"], json!(1));
    assert_eq!(listed.json()["items"][0]["github_issue_number"], json!(12));

    delete(&app, &format!("/api/tests/{}/issue", test.id)).await;
    let after = get(
        &app,
        "/api/tests?repo=acme/app&has_issue=true&include_stable=true",
    )
    .await;
    assert_eq!(after.json()["total"], json!(0));
    assert_eq!(after.json()["items"], json!([]));
}

#[tokio::test]
async fn the_link_routes_need_the_token() {
    let db = TestDb::new().await.expect("a test database");
    let test = one_test(&db).await;
    let app = app(&db, &[("FLAKERADAR_API_TOKEN", TOKEN)]);

    for (method, uri) in [
        (Method::PUT, format!("/api/tests/{}/issue", test.id)),
        (Method::DELETE, format!("/api/tests/{}/issue", test.id)),
        (Method::PUT, format!("/api/jobs/{}/issue", test.id)),
        (Method::DELETE, format!("/api/jobs/{}/issue", test.id)),
    ] {
        let reply = request(&app, method, &uri).await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{uri}");
        assert_eq!(
            reply.json()["detail"],
            json!("Invalid or missing X-API-Key")
        );
    }

    let wrong = request_with_headers(
        &app,
        Method::PUT,
        &format!("/api/tests/{}/issue", test.id),
        &[("X-API-Key", "wrong")],
    )
    .await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
    assert_eq!(test_link(&db, test.id).await.0, None);
}

#[tokio::test]
async fn a_path_parameter_that_is_not_an_id_is_the_path_validation_error() {
    let db = TestDb::new().await.expect("a test database");
    let app = app(&db, &[("FLAKERADAR_API_TOKEN", TOKEN)]);

    let reply = put(&app, "/api/tests/abc/issue", &json!({"number": 12})).await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    let detail = &reply.json()["detail"][0];
    assert_eq!(detail["type"], json!("int_parsing"));
    assert_eq!(detail["loc"], json!(["path", "test_id"]));
    assert!(
        detail["input"].is_string(),
        "the refused path value is reported"
    );
}

#[tokio::test]
async fn replacing_a_link_puts_it_back_at_the_front_of_the_sync_rotation() {
    let db = TestDb::new().await.expect("a test database");
    let test = one_test(&db).await;
    let app = app(&db, &[("FLAKERADAR_API_TOKEN", TOKEN)]);

    put(
        &app,
        &format!("/api/tests/{}/issue", test.id),
        &json!({"number": 12}),
    )
    .await;
    let rows = flakeradar_db::issue_links::linked_for_check(db.connection(), EntityKind::Test, 10)
        .await
        .expect("the rotation list reads");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].number, 12);
    assert_eq!(rows[0].repo, "acme/app");

    delete(&app, &format!("/api/tests/{}/issue", test.id)).await;
    let empty = flakeradar_db::issue_links::linked_for_check(db.connection(), EntityKind::Test, 10)
        .await
        .expect("the rotation list reads");
    assert!(empty.is_empty(), "an unlinked row has nothing to look up");
}

#[tokio::test]
async fn posting_to_a_link_route_is_a_method_error_with_the_allow_header() {
    let db = TestDb::new().await.expect("a test database");
    let app = app(&db, &[("FLAKERADAR_API_TOKEN", TOKEN)]);

    let reply = post(&app, "/api/tests/1/issue", &[], "").await;
    assert_eq!(reply.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(reply.json()["detail"], json!("Method Not Allowed"));
}

#[tokio::test]
async fn linking_a_job_issue_stores_the_number_and_url() {
    let db = TestDb::new().await.expect("a test database");
    let job = one_job(&db).await;
    let app = app(&db, &[("FLAKERADAR_API_TOKEN", TOKEN)]);

    let reply = put(
        &app,
        &format!("/api/jobs/{}/issue", job.id),
        &json!({"number": 42}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    let body = reply.json();
    assert_eq!(body["id"], json!(job.id));
    assert_eq!(body["github_issue_number"], json!(42));
    assert_eq!(
        body["github_issue_url"],
        json!("https://github.com/acme/app/issues/42")
    );

    let (number, filed, checked, closed, noted) = job_link(&db, job.id).await;
    assert_eq!(number, Some(42));
    assert_eq!(filed, Some(false), "a human linked it, not FlakeRadar");
    assert!(checked.is_none(), "a fresh link is checked first");
    assert!(!closed);
    assert!(!noted);

    let history = get(&app, &format!("/api/jobs/{}/history", job.id)).await;
    assert_eq!(history.json()["job"]["github_issue_number"], json!(42));
}

#[tokio::test]
async fn a_job_link_conflict_needs_replace() {
    let db = TestDb::new().await.expect("a test database");
    let job = one_job(&db).await;
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE jobs SET github_issue_number = 4 WHERE id = $1",
            [job.id.into()],
        ))
        .await
        .expect("the seeded link writes");
    let app = app(&db, &[("FLAKERADAR_API_TOKEN", TOKEN)]);

    let conflict = put(
        &app,
        &format!("/api/jobs/{}/issue", job.id),
        &json!({"number": 5}),
    )
    .await;
    assert_eq!(conflict.status, StatusCode::CONFLICT);
    assert_eq!(
        conflict.json()["detail"],
        json!("Job already links issue #4; pass replace=true to replace it")
    );

    let replaced = put(
        &app,
        &format!("/api/jobs/{}/issue?replace=true", job.id),
        &json!({"number": 5}),
    )
    .await;
    assert_eq!(replaced.status, StatusCode::OK);
    assert_eq!(replaced.json()["github_issue_number"], json!(5));
    assert_eq!(job_link(&db, job.id).await.0, Some(5));
}

#[tokio::test]
async fn the_same_job_number_keeps_the_check_stamp() {
    let db = TestDb::new().await.expect("a test database");
    let job = one_job(&db).await;
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE jobs SET github_issue_number = 5, github_issue_checked_at = now() WHERE id = $1",
            [job.id.into()],
        ))
        .await
        .expect("the seeded link writes");
    let app = app(&db, &[("FLAKERADAR_API_TOKEN", TOKEN)]);
    let before = job_link(&db, job.id).await;
    assert!(before.2.is_some());

    let reply = put(
        &app,
        &format!("/api/jobs/{}/issue", job.id),
        &json!({"number": 5}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.json()["github_issue_number"], json!(5));
    let after = job_link(&db, job.id).await;
    assert_eq!(after.0, Some(5));
    assert_eq!(
        after.2, before.2,
        "a same-number link keeps the stamp it just checked"
    );
}

#[tokio::test]
async fn a_job_url_must_be_the_issue_in_its_own_repo() {
    let db = TestDb::new().await.expect("a test database");
    let job = one_job(&db).await;
    let app = app(&db, &[("FLAKERADAR_API_TOKEN", TOKEN)]);

    let refused = put(
        &app,
        &format!("/api/jobs/{}/issue", job.id),
        &json!({"number": 6, "url": "https://github.com/other/repo/issues/6"}),
    )
    .await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        refused.json()["detail"],
        json!(
            "url must be https://github.com/acme/app/issues/6: the issue must be in the Job's own Repo"
        )
    );
    assert_eq!(job_link(&db, job.id).await.0, None);

    let accepted = put(
        &app,
        &format!("/api/jobs/{}/issue", job.id),
        &json!({"number": 6, "url": "https://github.com/acme/app/issues/6"}),
    )
    .await;
    assert_eq!(accepted.status, StatusCode::OK);
}

#[tokio::test]
async fn unlinking_a_job_clears_both_halves_and_its_provenance() {
    let db = TestDb::new().await.expect("a test database");
    let job = one_job(&db).await;
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE jobs SET github_issue_number = 8, github_issue_filed = FALSE, \
             github_issue_checked_at = now(), github_issue_closed = TRUE WHERE id = $1",
            [job.id.into()],
        ))
        .await
        .expect("the seeded link writes");
    let app = app(&db, &[("FLAKERADAR_API_TOKEN", TOKEN)]);

    let reply = delete(&app, &format!("/api/jobs/{}/issue", job.id)).await;
    assert_eq!(reply.status, StatusCode::OK);
    let body = reply.json();
    assert_eq!(body["github_issue_number"], Value::Null);
    assert_eq!(body["github_issue_url"], Value::Null);
    let (number, filed, checked, closed, noted) = job_link(&db, job.id).await;
    assert_eq!(number, None);
    assert_eq!(filed, None, "an unlinked row carries no provenance");
    assert!(checked.is_none());
    assert!(!closed);
    assert!(!noted);

    let again = delete(&app, &format!("/api/jobs/{}/issue", job.id)).await;
    assert_eq!(again.status, StatusCode::OK);
    assert_eq!(again.json()["github_issue_number"], Value::Null);
}

#[tokio::test]
#[expect(
    clippy::panic,
    reason = "the test names the arm that must not happen when the engine answers"
)]
async fn the_link_engine_refuses_with_its_status_and_links() {
    use flakeradar_core::EntityKind;
    use flakeradar_db::LinkError;
    use flakeradar_db::issue_links::{IssueLinkError, link_issue};

    let db = TestDb::new().await.expect("a test database");
    let test = one_test(&db).await;
    let reads = settings(&[]).reads(system_clock());

    let refused = link_issue(
        db.connection(),
        EntityKind::Test,
        i64::from(test.id),
        0,
        None,
        false,
        &reads,
    )
    .await;
    let LinkError::Refused(error) = refused.expect_err("a zero number is refused") else {
        panic!("the engine refuses, not fails");
    };
    assert_eq!(
        error,
        IssueLinkError::NotLinkable("number must be between 1 and 2147483647".to_owned())
    );
    // The refusal's kind decides the status at the front door, not in the engine.
    assert_eq!(
        flakeradar::RouteError::Refused(error).status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        flakeradar::RouteError::Refused(IssueLinkError::NotFound(String::new())).status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        flakeradar::RouteError::Refused(IssueLinkError::Conflict(String::new())).status(),
        StatusCode::CONFLICT
    );

    let linked = link_issue(
        db.connection(),
        EntityKind::Test,
        i64::from(test.id),
        11,
        None,
        false,
        &reads,
    )
    .await
    .expect("a valid number links");
    let flakeradar_db::LinkedOut::Test(out) = linked else {
        panic!("a Test target answers a Test");
    };
    assert_eq!(out.github_issue_number, Some(11));
}

#[tokio::test]
async fn a_failed_link_query_answers_the_plain_text_500() {
    let db = TestDb::new().await.expect("a test database");
    db.connection()
        .execute_unprepared("DROP TABLE test_cases CASCADE")
        .await
        .expect("the table is dropped");
    let reply = put(
        &app(&db, &[("FLAKERADAR_API_TOKEN", TOKEN)]),
        "/api/tests/1/issue",
        &json!({"number": 3}),
    )
    .await;
    assert_eq!(reply.status, 500);
    assert_eq!(reply.text(), "Internal Server Error");
    assert_eq!(
        reply.header("content-type").as_deref(),
        Some("text/plain; charset=utf-8")
    );
}

#[tokio::test]
async fn a_link_body_that_fails_while_it_is_read_is_a_500() {
    let db = TestDb::new().await.expect("a test database");
    let stream = futures_util::stream::iter(vec![
        Ok(axum::body::Bytes::from_static(b"{\"number\":")),
        Err(std::io::Error::other("the client went away")),
    ]);
    let request = axum::http::Request::builder()
        .method(Method::PUT)
        .uri("/api/tests/1/issue")
        .header("X-API-Key", TOKEN)
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from_stream(stream))
        .expect("the fixture request builds");
    let reply = send(&app(&db, &[("FLAKERADAR_API_TOKEN", TOKEN)]), request).await;
    assert_eq!(reply.status, 500);
    assert!(
        reply.text().contains("Reading the request body failed"),
        "{}",
        reply.text()
    );
}
