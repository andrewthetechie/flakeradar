//! The Issue link routes: link and unlink a GitHub issue on a Test or a Job.
//!
//! Ported from `backend/tests/test_issue_links_api.py` and
//! `backend/tests/test_issue_links_lifecycle.py`: the response shapes, the rules
//! that refuse a request, and the state the pair leaves behind.

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when a fixture or an answer does not behave"
)]

mod common;

use axum::http::{Method, StatusCode};
use common::{app, get, post, request, request_with_headers, send};
use flakeradar_test_support::{TestDb, make_job, make_pipeline, make_project, make_test_case};
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
async fn put(app: &axum::Router, uri: &str, body: &Value) -> common::Reply {
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
async fn delete(app: &axum::Router, uri: &str) -> common::Reply {
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
    let (_, _, checked, _, _) = test_link(&db, test.id).await;
    assert!(
        checked,
        "the same number was just checked, so its stamp stays"
    );
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

    let missing_job = delete(&app, "/api/jobs/999/issue").await;
    assert_eq!(missing_job.status, StatusCode::NOT_FOUND);
    assert_eq!(missing_job.json()["detail"], json!("Job not found"));
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
    let app = app(&db, &[("FLAKERADAR_API_TOKEN", TOKEN)]);

    for (method, uri) in [
        (Method::PUT, "/api/tests/1/issue"),
        (Method::DELETE, "/api/tests/1/issue"),
        (Method::PUT, "/api/jobs/1/issue"),
        (Method::DELETE, "/api/jobs/1/issue"),
    ] {
        let reply = request(&app, method, uri).await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{uri}");
        assert_eq!(
            reply.json()["detail"],
            json!("Invalid or missing X-API-Key")
        );
    }
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
    let rows = flakeradar_db::issue_links::linked_for_check(
        db.connection(),
        flakeradar_db::LinkTarget::Test,
        10,
    )
    .await
    .expect("the rotation list reads");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].number, 12);
    assert_eq!(rows[0].repo, "acme/app");

    delete(&app, &format!("/api/tests/{}/issue", test.id)).await;
    let empty = flakeradar_db::issue_links::linked_for_check(
        db.connection(),
        flakeradar_db::LinkTarget::Test,
        10,
    )
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
