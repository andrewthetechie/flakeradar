//! The OpenAPI document, the Redoc page and the browser bundle it loads.
//!
//! The document is generated from the routes at compile time, so it cannot fall
//! behind them: a route added without a `#[utoipa::path]` is missing here and
//! fails the test that compares the two. `/docs` renders it with Redoc, whose
//! browser bundle is vendored into the binary, so viewing the docs needs no
//! network beyond this instance.

// Rust guideline compliant 2026-07-21

use axum::Json;
use axum::Router;
use axum::http::header;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use flakeradar_core::{
    dto::CiRunFailedJobOut, dto::CiRunFailuresOut, dto::CiRunTestFailureOut, dto::DeletedCountsOut,
    dto::ExecutionOut, dto::ExplainingJobOut, dto::ExplainingTestOut, dto::HistoryOut,
    dto::IngestAccepted, dto::IssueLinkIn, dto::JobExecutionOut, dto::JobHistoryOut, dto::JobOut,
    dto::JobPage, dto::JobResultIn, dto::JobSummaryOut, dto::LocSegment, dto::LocationOut,
    dto::PipelineReportIn, dto::ProjectDeleteOut, dto::ProjectOut, dto::QuarantineIn,
    dto::QuarantineItem, dto::ReadinessOut, dto::RepoDeleteOut, dto::RepoOut, dto::ReportCountsOut,
    dto::ReportOut, dto::ReportSummaryOut, dto::RunnerStatOut, dto::ScorePointOut, dto::SummaryOut,
    dto::TestJobLinkOut, dto::TestOut, dto::TestPage, dto::ValidationIssue, model::JobStatus,
};
use utoipa::OpenApi;
use utoipa::openapi::OpenApi as OpenApiDocument;
use utoipa_redoc::Redoc;

use crate::app::AppState;
use crate::routes::ops::{self, HealthOut};
use crate::routes::{admin, issues, reads, reports};

/// Where the browser bundle is served from, under `/docs`.
pub const REDOC_SCRIPT_PATH: &str = "/docs/redoc.standalone.js";

/// The Redoc browser bundle, embedded in the binary.
///
/// A release binary carries it, so `/docs` works on an instance with no outbound
/// network; `assets/README.md` records the pinned version and its digest.
pub const REDOC_SCRIPT: &[u8] = include_bytes!("../assets/redoc.standalone.js");

/// The HTTP API, as served at `/openapi.json`.
#[derive(Debug, OpenApi)]
#[openapi(
    info(title = "FlakeRadar", version = env!("CARGO_PKG_VERSION")),
    paths(
        ops::health,
        ops::ready,
        ops::metrics,
        reports::ingest,
        reports::ingest_pipeline,
        reports::summary,
        reports::list_reports,
        reports::get_report,
        reports::retry_report,
        reports::delete_report,
        reads::repos,
        reads::tests,
        reads::test_summary,
        reads::test_history,
        reads::quarantine_test,
        reads::quarantine,
        reads::jobs,
        reads::jobs_summary,
        reads::job_history,
        reads::ci_run,
        issues::link_test,
        issues::unlink_test,
        issues::link_job,
        issues::unlink_job,
        admin::delete_repo,
        admin::delete_project
    ),
    components(schemas(
        HealthOut,
        ReadinessOut,
        LocSegment,
        ValidationIssue,
        IngestAccepted,
        ReportOut,
        ReportSummaryOut,
        ReportCountsOut,
        PipelineReportIn,
        JobResultIn,
        JobStatus,
        ProjectOut,
        RepoOut,
        DeletedCountsOut,
        RepoDeleteOut,
        ProjectDeleteOut,
        TestOut,
        TestPage,
        LocationOut,
        TestJobLinkOut,
        SummaryOut,
        ExecutionOut,
        ScorePointOut,
        HistoryOut,
        JobOut,
        JobPage,
        ExplainingTestOut,
        ExplainingJobOut,
        JobExecutionOut,
        RunnerStatOut,
        JobHistoryOut,
        JobSummaryOut,
        QuarantineIn,
        QuarantineItem,
        CiRunTestFailureOut,
        CiRunFailedJobOut,
        CiRunFailuresOut,
        IssueLinkIn
    ))
)]
pub struct ApiDoc;

/// The page `/docs` renders the API with.
///
/// `$spec` and `$config` are the two placeholders the renderer fills in: the
/// document to render and Redoc's own options. Every reference is a path on this
/// instance, so the browser requests nothing from a CDN.
const REDOC_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>FlakeRadar API</title>
    <style>
      body { margin: 0; padding: 0; }
    </style>
  </head>
  <body>
    <div id="redoc-container"></div>
    <script src="/docs/redoc.standalone.js"></script>
    <script>
      Redoc.init($spec, $config, document.getElementById("redoc-container"));
    </script>
  </body>
</html>
"#;

/// Adds the documentation routes to `router`.
pub fn router(router: Router<AppState>) -> Router<AppState> {
    router
        .route("/openapi.json", get(openapi))
        .route("/docs", get(redoc))
        .route("/docs/redoc.standalone.js", get(script))
}

/// The OpenAPI document.
pub async fn openapi() -> Json<OpenApiDocument> {
    Json(document())
}

/// The API document this build serves.
///
/// The title and the version come from the `OpenApi` derive, so the document
/// names the binary's own version without a second place to update.
#[must_use]
pub fn document() -> OpenApiDocument {
    ApiDoc::openapi()
}

/// The Redoc page.
pub async fn redoc() -> Response {
    Html(
        Redoc::new("/openapi.json")
            .custom_html(REDOC_HTML)
            .to_html(),
    )
    .into_response()
}

/// The Redoc browser bundle.
pub async fn script() -> Response {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        REDOC_SCRIPT,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    //! The document and the page are checked without a server.

    use super::{REDOC_HTML, REDOC_SCRIPT, REDOC_SCRIPT_PATH, document};

    #[test]
    fn the_document_names_the_binary_and_its_version() {
        let document = document();
        assert_eq!(document.info.title, "FlakeRadar");
        assert_eq!(document.info.version, env!("CARGO_PKG_VERSION"));
        let paths: Vec<&str> = document.paths.paths.keys().map(String::as_str).collect();
        assert!(paths.contains(&"/api/health"), "{paths:?}");
        assert!(paths.contains(&"/api/health/ready"), "{paths:?}");
        assert!(paths.contains(&"/metrics"), "{paths:?}");
        assert!(paths.contains(&"/api/ingest"), "{paths:?}");
        assert!(paths.contains(&"/api/reports/summary"), "{paths:?}");
    }

    #[test]
    fn the_page_loads_the_bundle_from_this_instance() {
        let html = super::Redoc::new("/openapi.json")
            .custom_html(REDOC_HTML)
            .to_html();
        assert!(html.contains(REDOC_SCRIPT_PATH), "{html}");
        assert!(html.contains("\"/openapi.json\""), "{html}");
        // No CDN: the docs page works on an instance with no outbound network.
        assert!(!html.contains("http://"), "{html}");
        assert!(!html.contains("https://"), "{html}");
    }

    #[test]
    fn the_browser_bundle_is_embedded() {
        assert!(REDOC_SCRIPT.len() > 100_000);
        assert!(String::from_utf8_lossy(REDOC_SCRIPT).contains("Redoc"));
    }
}
