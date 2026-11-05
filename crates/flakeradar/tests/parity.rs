//! The parity replay: every recorded scenario step, against the Rust app.
//!
//! The contract is `tests/parity/README.md`. `tools/parity-record/record.py`
//! recorded what the frozen Python server answered for each step of each
//! scenario; this harness sends the same requests to
//! [`flakeradar::build_app`] on a database of its own and compares the answers.
//!
//! Every scenario in `tests/parity/scenarios/` runs every step: there is no list
//! of enabled steps to fall behind and no way to switch a scenario off. A new
//! scenario file is replayed by [`every_scenario_replays_identically`] without
//! being registered anywhere, and a step without a golden fails the test that
//! checks the fixtures before any server is started.

#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "a replay that cannot read its fixtures or send its steps has failed"
)]

mod common;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use axum::routing::get;
use chrono::DateTime;
use flakeradar::{AdvisoryLock, DatabaseProcessor, ReportProcessor, ReportWorker, WorkerSettings};
use flakeradar::{AppState, Settings, build_app};
use flakeradar_db::entities::repos;
use flakeradar_db::{DbSettings, ProcessingSettings};
use flakeradar_test_support::TestDb;
use sea_orm::{ConnectionTrait, DbBackend, EntityTrait, JsonValue, Statement};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tower::ServiceExt as _;

/// The token every scenario's requests present, as the recorder used it.
const TOKEN: &str = "parity-token";

/// The `FLAKERADAR_*` settings the recorder pinned for every scenario.
///
/// Copied from `FIXED_SETTINGS` in `tools/parity-record/record.py`: a scenario
/// recorded against these values must be replayed against them, or the server
/// under test answers a different instance's question.
const FIXED_SETTINGS: &[(&str, &str)] = &[
    ("FLAKERADAR_API_TOKEN", TOKEN),
    ("FLAKERADAR_WORKER_POLL_SECONDS", "0.05"),
    ("FLAKERADAR_PRUNE_INTERVAL_SECONDS", "3600"),
    ("FLAKERADAR_WEBHOOK_URLS", ""),
    ("FLAKERADAR_WEBHOOK_FORMAT", "json"),
    ("FLAKERADAR_GITHUB_TOKEN", ""),
    ("FLAKERADAR_GITHUB_SYNC_BATCH", "2000"),
    ("FLAKERADAR_PUBLIC_URL", ""),
    ("FLAKERADAR_SCORE_WINDOW", "50"),
    ("FLAKERADAR_SCORE_DECAY", "0.85"),
    ("FLAKERADAR_BROKEN_STREAK", "5"),
    ("FLAKERADAR_FLAKE_THRESHOLD", "0.3"),
    ("FLAKERADAR_GITHUB_ISSUE_MIN_SCORE", "0.3"),
    ("FLAKERADAR_GITHUB_ISSUE_MIN_PROVEN_FLAKES", "0"),
    ("FLAKERADAR_GITHUB_ISSUE_MIN_FAILURES", "0"),
    ("FLAKERADAR_GITHUB_FIXED_STREAK", "0"),
    ("FLAKERADAR_GITHUB_CLOSE_FIXED", "false"),
    ("FLAKERADAR_CORS_ORIGINS", "http://localhost:5173"),
    ("FLAKERADAR_DB_COMMAND_TIMEOUT_SECONDS", "30"),
    ("FLAKERADAR_MAX_REPORT_RETRIES", "3"),
    ("FLAKERADAR_REPORT_RETENTION_DAYS", "7"),
    ("FLAKERADAR_FAILED_REPORT_RETENTION_DAYS", "0"),
    ("FLAKERADAR_EXECUTION_RETENTION_DAYS", "90"),
    ("FLAKERADAR_SCORE_HISTORY_RETENTION_DAYS", "365"),
    ("FLAKERADAR_PRUNE_BATCH", "5000"),
    ("FLAKERADAR_PRUNE_TIME_BUDGET_SECONDS", "60"),
];

/// How long one `wait_processed` step waits for the queue to drain.
const WAIT_PROCESSED_DEADLINE: Duration = Duration::from_secs(60);

/// How often `wait_processed` asks whether the queue is drained.
const WAIT_PROCESSED_INTERVAL: Duration = Duration::from_millis(50);

/// How long a replay waits for its worker to take the processor lock.
///
/// The recorder's server held the lock before it recorded, so the `/ready` and
/// `/metrics` goldens report a leader. A worker elects itself on its first loop
/// iteration, after `start` has returned, so an instrumented build can answer a
/// probe before the election; this is the bound on waiting for it.
const LEADERSHIP_DEADLINE: Duration = Duration::from_secs(30);

/// How often a replay asks its worker whether it leads.
const LEADERSHIP_INTERVAL: Duration = Duration::from_millis(20);

/// One scenario file.
#[derive(Debug, Deserialize)]
struct ScenarioFile {
    /// The scenario's name.
    name: String,
    /// Per-scenario settings, overriding [`FIXED_SETTINGS`].
    #[serde(default)]
    settings: BTreeMap<String, String>,
    /// The steps, whose shape depends on their type.
    steps: Vec<Value>,
}

/// A failure a replay reports, naming the scenario and the step.
type Mismatches = Vec<String>;

/// The request path with the characters an HTTP client percent-encodes encoded.
///
/// The recorder's client encoded a space in a path the same way, so a scenario
/// step that names a Repo or Project the identity rules refuse still reaches the
/// route the recording reached. Everything in the path a URI may carry unescaped
/// is left as it was.
fn encoded_path(path: &str) -> String {
    let mut encoded = String::with_capacity(path.len());
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'~'
            | b'!'
            | b'$'
            | b'&'
            | b'\''
            | b'('
            | b')'
            | b'*'
            | b'+'
            | b','
            | b';'
            | b'='
            | b':'
            | b'@'
            | b'/' => encoded.push(char::from(byte)),
            other => {
                write!(encoded, "%{other:02X}").expect("a String takes every write");
            }
        }
    }
    encoded
}

/// The root of the recorded fixtures.
fn parity_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/parity")
}

/// Loads a scenario by name.
fn scenario(name: &str) -> ScenarioFile {
    let path = parity_root().join("scenarios").join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("parsing {}: {error}", path.display()))
}

/// Loads the golden for one step of a scenario.
fn golden(scenario: &str, step: usize) -> Value {
    let path = parity_root()
        .join("golden")
        .join(scenario)
        .join(format!("{step:02}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("parsing {}: {error}", path.display()))
}

/// Holds the worker's first claim until a scenario's `wait_processed` step.
///
/// The recorder's server ran its worker from start-up, so a golden reports a
/// Report as still `pending` only because the recorder reached that step before
/// the worker's first claim. A replay worker that claims eagerly would race the
/// steps it is compared against, so the replay's worker leads from start-up —
/// the readiness and metrics goldens need a leader — but claims nothing until
/// the scenario asks the queue to drain. The claim itself is the real one.
#[derive(Debug)]
struct GatedProcessor {
    open: Arc<AtomicBool>,
}

impl GatedProcessor {
    fn new() -> Self {
        Self {
            open: Arc::new(AtomicBool::new(false)),
        }
    }

    fn open(&self) {
        self.open.store(true, Ordering::SeqCst);
    }
}

/// Waits until the replay's worker holds the processor lock.
///
/// `ReportWorker::start` returns before the election, which the worker runs on
/// its first loop iteration, so a replay that asked `/api/health/ready` straight
/// away could see `processor_leader: false` where every golden that records it
/// says `true`. The wait is bounded, and a worker that never takes the lock is
/// named here rather than surfacing as a mismatch in the probe's body.
async fn wait_for_leadership(worker: &ReportWorker) {
    let deadline = tokio::time::Instant::now() + LEADERSHIP_DEADLINE;
    while !worker.is_leader() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(LEADERSHIP_INTERVAL).await;
    }
    assert!(
        worker.is_leader(),
        "the replay's worker never took the processor lock in {LEADERSHIP_DEADLINE:?}"
    );
}

impl ReportProcessor for GatedProcessor {
    fn process_next(
        &self,
        pool: sea_orm::DatabaseConnection,
        settings: ProcessingSettings,
    ) -> flakeradar::worker::ProcessFuture<'static> {
        let open = Arc::clone(&self.open);
        Box::pin(async move {
            if open.load(Ordering::SeqCst) {
                DatabaseProcessor.process_next(pool, settings).await
            } else {
                Ok(None)
            }
        })
    }
}

#[tokio::test]
async fn every_scenario_replays_identically() {
    let mut failures: Mismatches = Vec::new();
    for scenario in scenario_files() {
        let name = scenario.name.clone();
        let db = TestDb::new().await.expect("a test database is available");
        let settings = settings_for(&scenario, &db);
        let app = build_app(AppState::new(
            db.connection().clone(),
            Arc::new(settings.clone()),
        ));
        // The recorded goldens came from a process that ran its Report
        // processor, so the replay runs the real worker over the same database.
        let gate = Arc::new(GatedProcessor::new());
        let gate_held: Arc<GatedProcessor> = Arc::clone(&gate);
        let processor: Arc<dyn ReportProcessor> = gate_held;
        let worker = Arc::new(
            ReportWorker::new(
                db.connection().clone(),
                Arc::new(AdvisoryLock::new(db.url(), DbSettings::default())),
                processing_of(&settings),
                WorkerSettings::from_settings(&settings),
            )
            .with_processor(Arc::clone(&processor)),
        );
        Arc::clone(&worker).start().await;
        wait_for_leadership(&worker).await;
        for (index, spec) in scenario.steps.iter().enumerate() {
            let step = index + 1;
            let recorded = golden(&name, step);
            if spec.get("wait_processed").is_some() {
                gate.open();
            }
            failures.extend(replay_step(&app, &db, &settings, &name, step, spec, &recorded).await);
        }
        worker.stop().await;
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// The settings a scenario's server runs with.
///
/// The recorder's `FIXED_SETTINGS` name no database, because its server read the
/// one in its own environment; the replay's worker needs the URL of the database
/// this scenario owns.
fn settings_for(scenario: &ScenarioFile, db: &TestDb) -> Settings {
    let mut values: BTreeMap<String, String> = FIXED_SETTINGS
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    values.extend(scenario.settings.clone());
    let _ = values.insert("FLAKERADAR_DATABASE_URL".to_owned(), db.url().to_owned());
    Settings::from_map(&values).expect("the recorded settings are valid")
}

/// The processor settings the worker claims and rescores with.
fn processing_of(settings: &Settings) -> ProcessingSettings {
    settings.processing()
}

/// Replays one step and reports every difference from its golden.
async fn replay_step(
    app: &Router,
    db: &TestDb,
    settings: &Settings,
    scenario: &str,
    step: usize,
    spec: &Value,
    recorded: &Value,
) -> Mismatches {
    if spec.get("http").is_some() {
        replay_http(app, scenario, step, spec, recorded).await
    } else if spec.get("dump_db").is_some() {
        let tables = spec
            .get("dump_db")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let dump = dump_db(db, &tables).await;
        compare_recorded(scenario, step, recorded, &dump)
    } else if spec.get("wait_processed").is_some() {
        let drained = wait_processed(app, db, settings).await;
        compare_recorded(scenario, step, recorded, &drained)
    } else if spec.get("mcp").is_some() {
        replay_mcp(app, scenario, step, spec, recorded).await
    } else {
        vec![format!(
            "{scenario} step {step}: the step is neither http, dump_db, wait_processed nor mcp"
        )]
    }
}

/// The protocol version every replayed MCP request declares.
///
/// The recorded server answered a client of this revision, so the replay
/// declares it too: rmcp drops the `resultType` discriminator for an older peer
/// and adds the caching fields for a newer one, and the golden carries the
/// first and not the second.
const MCP_VERSION: &str = "2026-07-28";

/// Replays one `mcp` step: one JSON-RPC request to the transport.
///
/// The golden holds the JSON-RPC `result` the frozen server's client decoded, so
/// the comparison is against the `result` member of the answer. A `tools/list`
/// golden holds the result object itself, because the recorder dumped the
/// client's tool models rather than the envelope.
async fn replay_mcp(
    app: &Router,
    scenario: &str,
    step: usize,
    spec: &Value,
    recorded: &Value,
) -> Mismatches {
    let call = spec
        .get("mcp")
        .and_then(|mcp| mcp.get("call"))
        .and_then(Value::as_str)
        .expect("the step names an MCP call");
    let (method, tool, params) = match call {
        "initialize" => (
            "initialize",
            None,
            json!({
                "protocolVersion": MCP_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "parity-replay", "version": "0"},
            }),
        ),
        "tools/list" => ("tools/list", None, json!({})),
        "tools/call" => {
            let tool = spec
                .get("mcp")
                .and_then(|mcp| mcp.get("tool"))
                .and_then(Value::as_str)
                .expect("a tools/call step names its tool");
            let arguments = spec
                .get("mcp")
                .and_then(|mcp| mcp.get("arguments"))
                .cloned()
                .unwrap_or_else(|| json!({}));
            (
                "tools/call",
                Some(tool),
                json!({"name": tool, "arguments": arguments}),
            )
        }
        other => panic!("{scenario} step {step}: unknown MCP call {other}"),
    };
    let mut params = params;
    if let Some(fields) = params.as_object_mut() {
        // A request that declares 2026-07-28 carries its lifecycle in `_meta`,
        // which rmcp requires before it dispatches one.
        let _ = fields.insert(
            "_meta".to_owned(),
            json!({
                "io.modelcontextprotocol/protocolVersion": MCP_VERSION,
                "io.modelcontextprotocol/clientCapabilities": {},
            }),
        );
    }
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri("/mcp/")
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("mcp-protocol-version", MCP_VERSION)
        .header("mcp-method", method);
    if let Some(tool) = tool {
        builder = builder.header("mcp-name", tool);
    }
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let request = builder
        .body(Body::from(body.to_string()))
        .unwrap_or_else(|error| panic!("{scenario} step {step}: the request builds: {error}"));
    let response = app
        .clone()
        .oneshot(request)
        .await
        .expect("the app answers every request");
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("the body reads to the end");
    let Ok(answer) = serde_json::from_slice::<Value>(&bytes) else {
        return vec![format!(
            "{scenario} step {step}: the answer is not JSON-RPC: {}",
            String::from_utf8_lossy(&bytes)
        )];
    };
    let actual = if let Some(result) = answer.get("result") {
        if recorded.get("result").is_some() {
            json!({"result": result})
        } else {
            result.clone()
        }
    } else if let Some(error) = answer.get("error") {
        json!({"error": error})
    } else {
        return vec![format!(
            "{scenario} step {step}: no result or error: {answer}"
        )];
    };
    compare_mcp(scenario, step, recorded, &actual)
}

/// Replays one `http` step.
async fn replay_http(
    app: &Router,
    scenario: &str,
    step: usize,
    spec: &Value,
    recorded: &Value,
) -> Mismatches {
    let request = spec.get("http").expect("the step is an http step");
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .expect("the step names a method");
    let path = request
        .get("path")
        .and_then(Value::as_str)
        .expect("the step names a path");

    let mut uri = encoded_path(path);
    let query = request
        .get("query")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if !query.is_empty() {
        let encoded: Vec<String> = query
            .iter()
            .map(|(key, value)| {
                let value = substitute(value.as_str().unwrap_or_default());
                format!(
                    "{}={}",
                    form_urlencoded::byte_serialize(key.as_bytes()).collect::<String>(),
                    form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>()
                )
            })
            .collect();
        uri.push('?');
        uri.push_str(&encoded.join("&"));
    }

    let mut builder = Request::builder()
        .method(Method::from_bytes(method.as_bytes()).expect("the method is valid"))
        .uri(&uri);
    if let Some(headers) = request.get("headers").and_then(Value::as_object) {
        for (name, value) in headers {
            builder = builder.header(name, substitute(value.as_str().unwrap_or_default()));
        }
    }
    let (body, content_type) = body_of(request);
    if let Some(content_type) = content_type {
        builder = builder.header("content-type", content_type);
    }
    let request = builder
        .body(body)
        .unwrap_or_else(|error| panic!("{scenario} step {step}: the request builds: {error}"));

    let response = app
        .clone()
        .oneshot(request)
        .await
        .expect("the app answers every request");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("the body reads to the end");

    let mut failures = header_differences(
        scenario,
        step,
        recorded,
        status,
        &headers,
        &allowances(scenario, step),
    );
    let content_type = media_type(&headers);
    let actual = decode_body(&bytes, content_type.as_deref());
    failures.extend(compare_body(
        scenario,
        step,
        recorded,
        &actual,
        &content_type.unwrap_or_default(),
    ));
    failures
}

/// Substitutes the token placeholder a scenario may carry.
fn substitute(value: &str) -> String {
    value.replace("$TOKEN", TOKEN)
}

/// The body a step asks for, and the content type it implies.
fn body_of(request: &Value) -> (Body, Option<String>) {
    if let Some(file) = request.get("multipart_file") {
        let field = file
            .get("field")
            .and_then(Value::as_str)
            .unwrap_or("report");
        let filename = file
            .get("filename")
            .and_then(Value::as_str)
            .unwrap_or("report.xml");
        let name = file
            .get("body_file")
            .and_then(Value::as_str)
            .expect("the multipart step names a file");
        let payload =
            std::fs::read(parity_root().join(name)).expect("the fixture body is readable");
        return multipart(field, filename, &payload);
    }
    if let Some(name) = request.get("body_file").and_then(Value::as_str) {
        let payload =
            std::fs::read(parity_root().join(name)).expect("the fixture body is readable");
        return (Body::from(payload), None);
    }
    if let Some(value) = request.get("body_json") {
        // The recorder sent these bytes with no content type, and only a step
        // that names one sends it: `json.dumps` text is the body, not a typed
        // document, which is why Python read it as raw text.
        let payload = serde_json::to_vec(value).expect("the fixture body serializes");
        return (Body::from(payload), None);
    }
    if let Some(text) = request.get("body_text").and_then(Value::as_str) {
        return (Body::from(text.to_owned()), None);
    }
    if let Some(size) = request.get("body_size").and_then(Value::as_u64) {
        return (
            Body::from(vec![b'a'; usize::try_from(size).expect("a body size")]),
            None,
        );
    }
    (Body::empty(), None)
}

/// One file field, in the multipart framing httpx wrote.
fn multipart(field: &str, filename: &str, payload: &[u8]) -> (Body, Option<String>) {
    const BOUNDARY: &str = "flakeradar-parity-boundary";
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"{field}\"; filename=\"{filename}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(b"Content-Type: application/xml\r\n\r\n");
    body.extend_from_slice(payload);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    (
        Body::from(body),
        Some(format!("multipart/form-data; boundary={BOUNDARY}")),
    )
}

/// The response's media type, without its parameters.
fn media_type(headers: &HeaderMap) -> Option<String> {
    headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_lowercase()
        })
}

/// Every difference in the status line and the recorded headers.
///
/// A status or content type an allowance covers is not a difference: the
/// allowances name those fields where a pre-approved change touches them.
fn header_differences(
    scenario: &str,
    step: usize,
    recorded: &Value,
    status: StatusCode,
    headers: &HeaderMap,
    allowances: &[AllowedField],
) -> Mismatches {
    let mut failures = Vec::new();
    let expected_status = recorded
        .get("status")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let actual_status = u64::from(status.as_u16());
    if actual_status != expected_status
        && !allowed(
            "status",
            &Value::from(expected_status),
            &Value::from(actual_status),
            allowances,
        )
    {
        failures.push(format!(
            "{scenario} step {step}: status {status} does not match {expected_status}"
        ));
    }
    for name in ["content-type", "location"] {
        let actual = headers.get(name).and_then(|value| value.to_str().ok());
        let recorded_value = recorded.get(name).and_then(Value::as_str);
        let differs = actual != recorded_value;
        let covered = allowed(
            name,
            &recorded_value.map_or(Value::Null, Value::from),
            &actual.map_or(Value::Null, Value::from),
            allowances,
        );
        if differs && !covered {
            match recorded_value {
                Some(expected) => failures.push(format!(
                    "{scenario} step {step}: {name} {actual:?} does not match {expected:?}"
                )),
                None => failures.push(format!(
                    "{scenario} step {step}: {name} {actual:?} was not recorded"
                )),
            }
        }
    }
    failures
}

/// The body of a recorded response, decoded by its content type.
fn decode_body(bytes: &[u8], content_type: Option<&str>) -> Value {
    let is_json = content_type.is_some_and(|value| value.contains("json"));
    if is_json {
        match serde_json::from_slice(bytes) {
            Ok(value) => value,
            Err(_) => Value::String(String::from_utf8_lossy(bytes).into_owned()),
        }
    } else {
        Value::String(String::from_utf8_lossy(bytes).into_owned())
    }
}

/// Every difference between a recorded payload and the answer.
///
/// The `dump_db` and `wait_processed` goldens hold their payload at the top
/// level beside `step`, not inside an HTTP `body` with a `status`, so the
/// comparison is the body comparison with `step` removed.
fn compare_recorded(scenario: &str, step: usize, recorded: &Value, actual: &Value) -> Mismatches {
    let mut expected = recorded.clone();
    if let Some(object) = expected.as_object_mut() {
        object.remove("step");
    }
    let allowances = allowances(scenario, step);
    let context = Comparison {
        scenario,
        step,
        content_type: "application/json",
        allowances: &allowances,
        in_table_body: false,
    };
    let mut failures = Vec::new();
    compare_value(&context, "body", &expected, actual, &mut failures);
    failures
}

/// Every difference between an MCP golden and the answer, from the document root.
///
/// An MCP golden holds the JSON-RPC result itself, so its fields are named from
/// the root (`result._meta`, `tools.0.name`) rather than under a `body` member,
/// which is how `allowed-differences.json` names them too.
fn compare_mcp(scenario: &str, step: usize, recorded: &Value, actual: &Value) -> Mismatches {
    let mut expected = recorded.clone();
    if let Some(object) = expected.as_object_mut() {
        object.remove("step");
    }
    let allowances = allowances(scenario, step);
    let context = Comparison {
        scenario,
        step,
        content_type: "application/json",
        allowances: &allowances,
        in_table_body: false,
    };
    let mut failures = Vec::new();
    compare_value(&context, "", &expected, actual, &mut failures);
    failures
}

/// Every difference between the recorded body and the answer.
fn compare_body(
    scenario: &str,
    step: usize,
    recorded: &Value,
    actual: &Value,
    content_type: &str,
) -> Mismatches {
    let expected = recorded.get("body").cloned().unwrap_or(Value::Null);
    let status = recorded
        .get("status")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let allowances = allowances(scenario, step);
    let context = Comparison {
        scenario,
        step,
        content_type,
        allowances: &allowances,
        in_table_body: false,
    };
    let mut failures = Vec::new();
    if status == 422 && is_framework_422(&expected) {
        compare_framework_422(scenario, step, &expected, actual, &mut failures);
        return failures;
    }
    compare_value(&context, "body", &expected, actual, &mut failures);
    failures
}

/// Whether a recorded body is FastAPI's own validation-error shape.
fn is_framework_422(body: &Value) -> bool {
    body.get("detail")
        .and_then(Value::as_array)
        .is_some_and(|issues| {
            !issues.is_empty()
                && issues
                    .iter()
                    .all(|issue| issue.get("type").is_some() && issue.get("loc").is_some())
        })
}

/// Compares a framework 422: the shape, each entry's `type` and `loc`.
fn compare_framework_422(
    scenario: &str,
    step: usize,
    expected: &Value,
    actual: &Value,
    failures: &mut Mismatches,
) {
    let expected_issues = expected
        .get("detail")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let Some(actual_issues) = actual.get("detail").and_then(Value::as_array) else {
        failures.push(format!(
            "{scenario} step {step}: the detail is not a list: {actual}"
        ));
        return;
    };
    if expected_issues.len() != actual_issues.len() {
        failures.push(format!(
            "{scenario} step {step}: {} issues recorded, {} answered",
            expected_issues.len(),
            actual_issues.len()
        ));
        return;
    }
    for (index, expected_issue) in expected_issues.iter().enumerate() {
        let Some(actual_issue) = actual_issues.get(index) else {
            continue;
        };
        for key in ["type", "loc"] {
            let wanted = expected_issue.get(key);
            let got = actual_issue.get(key);
            if wanted != got {
                failures.push(format!(
                    "{scenario} step {step}: issue {index} {key} {got:?} does not match {wanted:?}"
                ));
            }
        }
    }
}

/// One allowance: a golden file and a field of it that may differ.
#[derive(Debug, Deserialize)]
struct Allowance {
    /// The fields of one golden the entry covers.
    allows: Vec<AllowedField>,
}

/// One field an allowance covers.
#[derive(Debug, Deserialize)]
struct AllowedField {
    /// The golden the field belongs to, relative to `tests/parity`.
    file: String,
    /// The dotted path into the golden, such as `body.detail`.
    field: String,
    /// The value Python recorded, when the allowance is narrowed to it.
    #[serde(rename = "pythonValue")]
    python_value: Option<Value>,
    /// The value the Rust server answers, when the allowance is narrowed to it.
    #[serde(rename = "rustValue")]
    rust_value: Option<Value>,
    /// A pattern the Rust value must match, such as a prefix.
    #[serde(rename = "match")]
    matching: Option<MatchRule>,
    /// A contract the field carries, which is compared by its promises rather
    /// than by value.
    contract: Option<ContractRule>,
}

/// A contract a field is compared by instead of value for value.
///
/// The only kind is `openapi`: `/openapi.json` is generated by two different
/// frameworks, so the documents cannot agree byte for byte, and the promise both
/// sides make is the one this comparison checks — the same paths and methods,
/// the same parameters, and the same response field names. The reason is in
/// `differences.md`.
#[derive(Debug, Deserialize)]
struct ContractRule {
    /// Which contract: `openapi` is the only kind.
    kind: String,
}

/// A pattern an allowed Rust value must match.
#[derive(Debug, Deserialize)]
struct MatchRule {
    /// How to match: `prefix` is the only kind the recorder writes.
    kind: String,
    /// What the Rust value must start with.
    value: String,
}

/// The allowances that cover one step's golden.
fn allowances(scenario: &str, step: usize) -> Vec<AllowedField> {
    let path = parity_root().join("allowed-differences.json");
    let text = std::fs::read_to_string(&path).expect("the allowances are readable");
    let entries: Vec<Allowance> =
        serde_json::from_str(&text).expect("the allowances are valid JSON");
    let file = format!("golden/{scenario}/{step:02}.json");
    entries
        .into_iter()
        .flat_map(|entry| entry.allows)
        .filter(|allowed| allowed.file == file)
        .collect()
}

/// Whether an allowance covers `path` with exactly this mismatch.
fn allowed(path: &str, expected: &Value, actual: &Value, allowances: &[AllowedField]) -> bool {
    allowances.iter().any(|allowance| {
        if allowance.field != path {
            return false;
        }
        if let Some(python_value) = &allowance.python_value
            && python_value != expected
        {
            return false;
        }
        if let Some(rust_value) = &allowance.rust_value
            && rust_value != actual
        {
            return false;
        }
        match &allowance.matching {
            Some(rule) if rule.kind == "prefix" => actual
                .as_str()
                .is_some_and(|value| value.starts_with(&rule.value)),
            Some(_) => false,
            None => true,
        }
    })
}

/// The contract an allowance pins `path` to, if one does.
fn contract_for<'a>(path: &str, allowances: &'a [AllowedField]) -> Option<&'a str> {
    allowances
        .iter()
        .filter(|allowance| allowance.field == path)
        .find_map(|allowance| allowance.contract.as_ref())
        .map(|rule| rule.kind.as_str())
}

/// The component schemas whose names the two frameworks spell differently.
///
/// Every other schema must exist under the same name and declare the same field
/// names. FastAPI's two validation schemas are its framework's own: the reference
/// documents its 422 as `HTTPValidationError` wrapping `ValidationError`, while
/// this port documents the status with a description and keeps the same five
/// issue fields in `ValidationIssue`. The 422 replay steps compare the wire
/// bodies of both, and `differences.md` records the difference.
const OPENAPI_SCHEMA_ALIASES: &[(&str, &str)] = &[("ValidationError", "ValidationIssue")];

/// The schemas only the reference's document has.
const OPENAPI_SCHEMA_ONLY_ON_THE_REFERENCE: &[&str] = &["HTTPValidationError"];

/// Compares an OpenAPI document against the recorded one, by its promises.
///
/// The two documents come from different frameworks, so their descriptions,
/// summaries, operation ids, schema spellings and framework components differ by
/// construction. What both must agree on is what a client reads: the same paths,
/// the same methods, the same parameters, and the same field names in every
/// response schema. Every difference is reported by the path or schema it is in.
fn compare_openapi(scenario: &str, step: usize, expected: &Value, actual: &Value) -> Mismatches {
    let mut failures = Mismatches::new();
    let describe = |message: String| format!("{scenario} step {step}: {message}");
    let (Some(recorded), Some(answered)) = (
        expected.get("paths").and_then(Value::as_object),
        actual.get("paths").and_then(Value::as_object),
    ) else {
        return vec![format!(
            "{scenario} step {step}: the document has no paths object on one side"
        )];
    };
    compare_openapi_paths(recorded, answered, &describe, &mut failures);
    compare_openapi_schemas(expected, actual, &describe, &mut failures);
    compare_openapi_bodies(recorded, answered, &describe, &mut failures);
    failures
}

/// Compares the paths, the methods on each path, and each operation's parameters.
fn compare_openapi_paths(
    recorded: &Map<String, Value>,
    answered: &Map<String, Value>,
    describe: &dyn Fn(String) -> String,
    failures: &mut Mismatches,
) {
    for path in recorded.keys() {
        if !answered.contains_key(path) {
            failures.push(describe(format!("the openapi path {path} is missing")));
        }
    }
    for path in answered.keys() {
        if !recorded.contains_key(path) {
            failures.push(describe(format!(
                "the openapi path {path} was not recorded"
            )));
        }
    }
    for (path, recorded) in recorded {
        let (Some(answered), Some(recorded)) = (
            answered.get(path).and_then(Value::as_object),
            recorded.as_object(),
        ) else {
            continue;
        };
        for method in recorded.keys() {
            match answered.get(method) {
                Some(operation) => compare_operation(
                    &recorded[method],
                    operation,
                    &format!("{method} {path}"),
                    describe,
                    failures,
                ),
                None => failures.push(describe(format!(
                    "the openapi method {method} {path} is missing"
                ))),
            }
        }
        for method in answered.keys() {
            if !recorded.contains_key(method) {
                failures.push(describe(format!(
                    "the openapi method {method} {path} was not recorded"
                )));
            }
        }
    }
}

/// The field names a component schema declares, sorted.
fn schema_fields(schema: &Value) -> Vec<String> {
    let mut names: Vec<String> = schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| properties.keys().cloned().collect())
        .unwrap_or_default();
    names.sort();
    names
}

/// Compares the field names of every component schema the reference declares.
fn compare_openapi_schemas(
    expected: &Value,
    actual: &Value,
    describe: &dyn Fn(String) -> String,
    failures: &mut Mismatches,
) {
    let schemas = |document: &Value| {
        document
            .get("components")
            .and_then(|components| components.get("schemas"))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default()
    };
    let (recorded_schemas, answered_schemas) = (schemas(expected), schemas(actual));
    if recorded_schemas.is_empty() || answered_schemas.is_empty() {
        failures.push(describe(
            "the document has no component schemas on one side".to_owned(),
        ));
        return;
    }
    for (name, schema) in &recorded_schemas {
        if OPENAPI_SCHEMA_ONLY_ON_THE_REFERENCE.contains(&name.as_str()) {
            continue;
        }
        let answered_name = OPENAPI_SCHEMA_ALIASES
            .iter()
            .find_map(|(reference, port)| (*reference == name).then_some(*port))
            .unwrap_or(name.as_str());
        let Some(answered) = answered_schemas.get(answered_name) else {
            failures.push(describe(format!(
                "the openapi schema {answered_name} is missing"
            )));
            continue;
        };
        let (recorded_fields, answered_fields) = (schema_fields(schema), schema_fields(answered));
        if recorded_fields != answered_fields {
            failures.push(describe(format!(
                "the openapi schema {answered_name} declares {answered_fields:?} where {name} \
                 declares {recorded_fields:?}"
            )));
        } else if answered_name != name && answered_schemas.contains_key(name.as_str()) {
            // The alias must be a rename and nothing else: the fields are equal
            // by the comparison above, and the reference spelling must not also
            // be present, or a client reading it would see a second shape.
            failures.push(describe(format!(
                "the openapi schema {name} and its port {answered_name} both exist"
            )));
        }
    }
}

/// Compares the schema every 2xx answer's body references.
fn compare_openapi_bodies(
    recorded: &Map<String, Value>,
    answered: &Map<String, Value>,
    describe: &dyn Fn(String) -> String,
    failures: &mut Mismatches,
) {
    for (path, recorded) in recorded {
        let (Some(recorded), Some(answered)) = (
            recorded.as_object(),
            answered.get(path).and_then(Value::as_object),
        ) else {
            continue;
        };
        for (method, operation) in recorded {
            let Some(answered_operation) = answered.get(method) else {
                continue;
            };
            for (status, response) in operation
                .get("responses")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                if !status.starts_with('2') {
                    continue;
                }
                let Some(recorded_ref) = response_ref(response) else {
                    continue;
                };
                let answered_ref = answered_operation
                    .get("responses")
                    .and_then(|responses| responses.get(status))
                    .and_then(response_ref);
                if answered_ref.as_deref() != Some(recorded_ref.as_str()) {
                    failures.push(describe(format!(
                        "the {status} body of {method} {path} references {answered_ref:?} where it \
                         references {recorded_ref}"
                    )));
                }
            }
        }
    }
}

/// Compares one operation's parameters and reports every difference.
///
/// A parameter is its place and its name: the reference's `x-api-key` header is
/// documented beside the query and path parameters, and either side naming an
/// extra or a missing one is a difference a client would trip over.
fn compare_operation(
    recorded: &Value,
    answered: &Value,
    operation: &str,
    describe: &dyn Fn(String) -> String,
    failures: &mut Mismatches,
) {
    let parameters = |operation: &Value| -> Vec<String> {
        let mut names: Vec<String> = operation
            .get("parameters")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|parameter| {
                        let place = parameter.get("in").and_then(Value::as_str)?;
                        let name = parameter.get("name").and_then(Value::as_str)?;
                        Some(format!("{place} {name}"))
                    })
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names.dedup();
        names
    };
    let (recorded, answered) = (parameters(recorded), parameters(answered));
    if recorded != answered {
        failures.push(describe(format!(
            "{operation} takes {answered:?} where it takes {recorded:?}"
        )));
    }
}

/// The component schema a response's JSON body references, if it references one.
fn response_ref(response: &Value) -> Option<String> {
    let schema = response
        .get("content")?
        .get("application/json")?
        .get("schema")?;
    let reference = schema.get("$ref")?.as_str()?;
    reference.rsplit('/').next().map(str::to_owned)
}

/// What one comparison carries besides its two values.
struct Comparison<'a> {
    /// The scenario being replayed.
    scenario: &'a str,
    /// The step being replayed.
    step: usize,
    /// The response's media type, which decides whether a placeholder applies.
    content_type: &'a str,
    /// The allowances that cover this step's golden.
    allowances: &'a [AllowedField],
    /// Whether these values were decoded from a hex-encoded table `body` column.
    in_table_body: bool,
}

impl Comparison<'_> {
    /// The same comparison, one level inside a decoded table `body`.
    fn inside_table_body(&self) -> Comparison<'_> {
        Comparison {
            scenario: self.scenario,
            step: self.step,
            content_type: self.content_type,
            allowances: self.allowances,
            in_table_body: true,
        }
    }
}

/// Compares one value, reporting every difference by its dotted path.
///
/// JSON compares by value with numbers compared numerically, `"<timestamp>"` and
/// `"<date>"` match anything in the timestamp and date rules, and an allowance
/// covers exactly the field it names. A dumped table's `body` column holds its
/// document in hex, so that column's document is compared field by field rather
/// than as one string.
fn compare_value(
    context: &Comparison<'_>,
    path: &str,
    expected: &Value,
    actual: &Value,
    failures: &mut Mismatches,
) {
    if let Some(contract) = contract_for(path, context.allowances) {
        match contract {
            "openapi" => failures.extend(compare_openapi(
                context.scenario,
                context.step,
                expected,
                actual,
            )),
            other => failures.push(format!(
                "{} step {}: {path} is pinned to the unknown contract {other}",
                context.scenario, context.step
            )),
        }
        return;
    }
    if names_a_dumped_column(path)
        && let (Some(recorded), Some(answered)) = (table_body(expected), table_body(actual))
    {
        let inside = context.inside_table_body();
        compare_value(&inside, path, &recorded, &answered, failures);
        return;
    }
    match (expected, actual) {
        (Value::Object(expected), Value::Object(actual)) => {
            for (key, value) in expected {
                let child = join(path, key);
                match actual.get(key) {
                    Some(other) => compare_value(context, &child, value, other, failures),
                    None => {
                        if !allowed(&child, value, &Value::Null, context.allowances) {
                            failures.push(format!(
                                "{} step {}: {child} is missing",
                                context.scenario, context.step
                            ));
                        }
                    }
                }
            }
            for key in actual.keys() {
                if expected.contains_key(key) {
                    continue;
                }
                let child = join(path, key);
                if !allowed(&child, &Value::Null, &actual[key], context.allowances) {
                    failures.push(format!(
                        "{} step {}: {child} was not recorded",
                        context.scenario, context.step
                    ));
                }
            }
        }
        (Value::Array(expected), Value::Array(actual)) if expected.len() == actual.len() => {
            for (index, (value, other)) in expected.iter().zip(actual).enumerate() {
                compare_value(context, &format!("{path}.{index}"), value, other, failures);
            }
        }
        _ => {
            if values_match(expected, actual, context.content_type)
                || (context.in_table_body && same_instant(path, expected, actual))
                || allowed(path, expected, actual, context.allowances)
            {
                return;
            }
            failures.push(format!(
                "{} step {}: {path} {actual} does not match {expected}",
                context.scenario, context.step
            ));
        }
    }
}

/// The dotted path of a value inside another.
///
/// The root itself has no path, so a comparison that starts at a document's root
/// names its first level without a leading dot, which is how a golden file names
/// its own fields.
fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_owned()
    } else {
        format!("{path}.{key}")
    }
}

/// Whether a recorded value and an answered one are the same value.
fn values_match(expected: &Value, actual: &Value, content_type: &str) -> bool {
    match (expected, actual) {
        (Value::Number(left), Value::Number(right)) => left.as_f64() == right.as_f64(),
        (Value::String(placeholder), Value::String(value)) => {
            match (placeholder.as_str(), content_type) {
                (TIMESTAMP_PLACEHOLDER, _) => is_timestamp(value),
                (DATE_PLACEHOLDER, _) => is_date(value),
                _ => placeholder == value || placeholder_text_matches(placeholder, value),
            }
        }
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(one, other)| values_match(one, other, content_type))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, one)| {
                    right
                        .get(key)
                        .is_some_and(|other| values_match(one, other, content_type))
                })
        }
        _ => expected == actual,
    }
}

/// One piece of a recorded text: a literal run of characters, or a placeholder.
#[derive(Debug, PartialEq, Eq)]
enum Piece<'a> {
    /// Characters the answer must carry unchanged.
    Literal(&'a str),
    /// A timestamp, in the rule Fabro accepts.
    Timestamp,
    /// A `YYYY-MM-DD` date.
    Date,
}

/// Splits recorded text into its literal runs and its placeholders.
fn pieces(template: &str) -> Vec<Piece<'_>> {
    let mut pieces = Vec::new();
    let mut rest = template;
    while !rest.is_empty() {
        let next = [TIMESTAMP_PLACEHOLDER, DATE_PLACEHOLDER]
            .iter()
            .filter_map(|placeholder| {
                rest.find(placeholder)
                    .map(|index| (index, *placeholder, placeholder.len()))
            })
            .min_by_key(|(index, _, _)| *index);
        let Some((index, placeholder, length)) = next else {
            pieces.push(Piece::Literal(rest));
            return pieces;
        };
        if let Some(literal) = rest.get(..index) {
            pieces.push(Piece::Literal(literal));
        }
        pieces.push(if placeholder == TIMESTAMP_PLACEHOLDER {
            Piece::Timestamp
        } else {
            Piece::Date
        });
        rest = rest.get(index + length..).unwrap_or_default();
    }
    pieces
}

/// Whether an answered text is the recorded one with every placeholder filled in.
///
/// The recorder replaces each timestamp and date with a placeholder, and it does
/// so inside a text field too: an MCP content block holds its JSON as a string,
/// where the timestamps the answer writes live inside that string. So a recorded
/// text with a placeholder in it matches when every other character is identical
/// and each placeholder holds a value in its rule. A recorded text with no
/// placeholder is compared as it is.
fn placeholder_text_matches(template: &str, value: &str) -> bool {
    let pieces = pieces(template);
    if !pieces
        .iter()
        .any(|piece| matches!(piece, Piece::Timestamp | Piece::Date))
    {
        return false;
    }
    let mut rest = value;
    for (index, piece) in pieces.iter().enumerate() {
        match piece {
            Piece::Literal(literal) => match rest.strip_prefix(literal) {
                Some(after) => rest = after,
                None => return false,
            },
            Piece::Timestamp | Piece::Date => {
                // A placeholder runs until the next literal piece, which in a
                // JSON text is the closing quote of the string it fills.
                let end = pieces.get(index + 1).and_then(|next| match next {
                    Piece::Literal(literal) => rest.find(literal),
                    Piece::Timestamp | Piece::Date => None,
                });
                let Some(end) = end else {
                    return false;
                };
                let Some(candidate) = rest.get(..end) else {
                    return false;
                };
                let matches = match piece {
                    Piece::Timestamp => is_timestamp(candidate),
                    _ => is_date(candidate),
                };
                if !matches {
                    return false;
                }
                rest = rest.get(end..).unwrap_or_default();
            }
        }
    }
    rest.is_empty()
}

/// Whether two texts name the same instant on a timestamp-named field.
///
/// `differences.md` lets a stored timestamp omit a zero fraction and carry either
/// `Z` or `+00:00`, so a `*_at` field inside a table `body` compares by the
/// instant it names. Every other field, and every other difference in that
/// instant, is a mismatch.
fn same_instant(path: &str, expected: &Value, actual: &Value) -> bool {
    let key = path.rsplit_once('.').map_or(path, |(_, key)| key);
    if !key.ends_with("_at") {
        return false;
    }
    let (Some(expected), Some(actual)) = (expected.as_str(), actual.as_str()) else {
        return false;
    };
    let (Ok(expected), Ok(actual)) = (
        DateTime::parse_from_rfc3339(expected),
        DateTime::parse_from_rfc3339(actual),
    ) else {
        return false;
    };
    expected == actual
}

/// Whether `path` names a column of a dumped table: `body.tables.<table>.<row>.<column>`.
///
/// Only a `dump_db` step has that shape, so the hex decoding below reaches the
/// dump's table rows and nothing else: a `\x`-prefixed string anywhere in an HTTP
/// response, or nested deeper inside a dumped document, keeps comparing as text.
fn names_a_dumped_column(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("body.tables.") else {
        return false;
    };
    let mut parts = rest.split('.');
    let (Some(_table), Some(row), Some(_column), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    !row.is_empty() && row.chars().all(|digit| digit.is_ascii_digit())
}

/// The JSON document a hex-encoded table `body` column holds, when it holds one.
///
/// PostgreSQL renders a `bytea` column as `\x` and its bytes in hex, so a dump of
/// `reports` carries the queued upload inside a string. A body that is not JSON,
/// such as the JUnit upload, decodes to none and compares as the text it is.
fn table_body(value: &Value) -> Option<Value> {
    let text = value.as_str()?.strip_prefix("\\x")?;
    let bytes = unhex(text)?;
    serde_json::from_slice(&bytes).ok()
}

/// The bytes a string of hex digit pairs names, or none when it is not one.
fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let mut bytes = Vec::with_capacity(text.len() / 2);
    let mut digits = text.bytes();
    while let (Some(high), Some(low)) = (digits.next(), digits.next()) {
        let high = char::from(high).to_digit(16)?;
        let low = char::from(low).to_digit(16)?;
        bytes.push(u8::try_from(high * 16 + low).ok()?);
    }
    Some(bytes)
}

/// The placeholder a recorded timestamp is replaced with.
const TIMESTAMP_PLACEHOLDER: &str = "<timestamp>";

/// The placeholder a recorded date is replaced with.
const DATE_PLACEHOLDER: &str = "<date>";

/// Whether `value` is a timestamp in the rule Fabro accepts.
fn is_timestamp(value: &str) -> bool {
    let Some((date, rest)) = value.split_once('T') else {
        return false;
    };
    if !is_date(date) {
        return false;
    }
    let time = rest
        .strip_suffix('Z')
        .or_else(|| rest.strip_suffix("+00:00"))
        .unwrap_or(rest);
    let (clock, fraction) = match time.split_once('.') {
        Some((clock, fraction)) => (clock, Some(fraction)),
        None => (time, None),
    };
    let parts: Vec<&str> = clock.split(':').collect();
    if parts.len() != 3 || parts.iter().any(|part| !is_digits(part, 2)) {
        return false;
    }
    match fraction {
        Some(fraction) => !fraction.is_empty() && fraction.chars().all(|c| c.is_ascii_digit()),
        None => rest.ends_with('Z') || rest.ends_with("+00:00"),
    }
}

/// Whether `value` is a `YYYY-MM-DD` date.
fn is_date(value: &str) -> bool {
    let widths = [4, 2, 2];
    let mut parts = value.split('-');
    let matches = widths
        .iter()
        .all(|width| parts.next().is_some_and(|part| is_digits(part, *width)));
    matches && parts.next().is_none()
}

/// Whether `text` is exactly `width` ASCII digits.
fn is_digits(text: &str, width: usize) -> bool {
    text.len() == width && text.chars().all(|c| c.is_ascii_digit())
}

/// Settings at the frozen server's defaults, for a harness test with no scenario.
fn test_settings() -> Settings {
    Settings::from_map(&BTreeMap::new()).expect("the default settings are valid")
}

/// Polls `GET /api/reports/summary` until the running worker has drained the queue.
async fn wait_processed(app: &Router, _db: &TestDb, _settings: &Settings) -> Value {
    wait_for_drain(app).await
}

/// Polls `GET /api/reports/summary` until nothing is pending.
///
/// This is the only waiting a scenario does; the interval and the deadline are
/// constants so a replay fails rather than hangs.
async fn wait_for_drain(app: &Router) -> Value {
    let deadline = tokio::time::Instant::now() + WAIT_PROCESSED_DEADLINE;
    loop {
        let request = Request::builder()
            .uri("/api/reports/summary")
            .body(Body::empty())
            .expect("the fixture request builds");
        let response = app
            .clone()
            .oneshot(request)
            .await
            .expect("the app answers every request");
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the body reads to the end");
        let summary = decode_body(&bytes, Some("application/json"));
        if summary.get("pending").and_then(Value::as_u64) == Some(0) {
            return serde_json::json!({"processed": true, "reports": summary});
        }
        if tokio::time::Instant::now() >= deadline {
            return serde_json::json!({"processed": false, "reports": summary});
        }
        tokio::time::sleep(WAIT_PROCESSED_INTERVAL).await;
    }
}

/// Every row of the listed tables, as JSON.
///
/// An entry is a table name (ordered by `id`) or `{"table", "order_by"}` for a
/// table with no `id`, such as the Score history tables.
async fn dump_db(db: &TestDb, tables: &[Value]) -> Value {
    let mut dump = Map::new();
    for entry in tables {
        let (table, order) = match entry {
            Value::String(table) => (table.as_str(), "id"),
            other => (
                other
                    .get("table")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                other
                    .get("order_by")
                    .and_then(Value::as_str)
                    .unwrap_or("id"),
            ),
        };
        let sql = format!(
            "SELECT coalesce(json_agg(t), '[]'::json) AS rows \
             FROM (SELECT * FROM {table} ORDER BY {order}) t"
        );
        let row = db
            .connection()
            .query_one_raw(Statement::from_string(DbBackend::Postgres, sql))
            .await
            .expect("the dump query runs")
            .expect("the dump query returns a row");
        let rows: JsonValue = row.try_get("", "rows").expect("the dump is JSON");
        let _ = dump.insert(table.to_owned(), rows);
    }
    serde_json::json!({"tables": dump})
}

#[cfg(test)]
mod tests {
    //! The comparison rules the contract states, on their own.

    use serde_json::json;

    use super::{
        AllowedField, MatchRule, compare_body, is_framework_422, is_timestamp, values_match,
    };

    /// Whether two values are the same under the contract's rules.
    fn matches(expected: &serde_json::Value, actual: &serde_json::Value) -> bool {
        values_match(expected, actual, "application/json")
    }

    #[test]
    fn numbers_compare_numerically() {
        assert!(matches(&json!(1), &json!(1.0)));
        assert!(matches(&json!(0.5), &json!(0.50)));
        assert!(!matches(&json!(1), &json!(2)));
        assert!(matches(&json!(-0.0), &json!(0.0)));
    }

    #[test]
    fn a_placeholder_matches_any_value_in_its_rule() {
        assert!(matches(
            &json!("<timestamp>"),
            &json!("2026-11-04T09:00:00.000000Z")
        ));
        assert!(matches(
            &json!("<timestamp>"),
            &json!("2026-11-04T09:00:00Z")
        ));
        assert!(matches(
            &json!("<timestamp>"),
            &json!("2026-11-04T09:00:00+00:00")
        ));
        assert!(!matches(&json!("<timestamp>"), &json!("2026-11-04")));
        assert!(!matches(
            &json!("<timestamp>"),
            &json!("2026-11-04T09:00:00")
        ));
        assert!(matches(&json!("<date>"), &json!("2026-11-04")));
        assert!(!matches(&json!("<date>"), &json!("2026-11-4")));
    }

    #[test]
    fn a_timestamp_is_read_by_the_rule_fabro_accepts() {
        assert!(is_timestamp("2026-11-04T09:00:00.123456Z"));
        assert!(is_timestamp("2026-11-04T09:00:00Z"));
        assert!(is_timestamp("2026-11-04T09:00:00+00:00"));
        assert!(!is_timestamp("2026-11-04T09:00:00.123+02:00"));
        assert!(!is_timestamp("2026-11-04 09:00:00Z"));
        assert!(!is_timestamp("2026-11-04T09:00Z"));
    }

    #[test]
    fn objects_and_arrays_compare_field_by_field() {
        assert!(matches(
            &json!({"a": 1, "b": [1, "<date>"]}),
            &json!({"a": 1.0, "b": [1, "2026-01-01"]})
        ));
        assert!(!matches(&json!({"a": 1}), &json!({"a": 1, "b": 2})));
        assert!(!matches(&json!({"a": 1, "b": 2}), &json!({"a": 1})));
        assert!(!matches(&json!([1, 2]), &json!([1, 2, 3])));
    }

    #[test]
    fn a_framework_422_compares_its_shape_and_each_type_and_loc() {
        let recorded = json!({"detail": [{"type": "int_parsing", "loc": ["query", "limit"], "msg": "Input should be a valid integer"}]});
        assert!(is_framework_422(&recorded));
        assert!(!is_framework_422(&json!({"detail": "Report not found"})));
        let same = json!({"detail": [{"type": "int_parsing", "loc": ["query", "limit"], "msg": "another wording"}]});
        assert_eq!(
            compare_body(
                "s",
                1,
                &json!({"status": 422, "body": recorded}),
                &same,
                "application/json"
            ),
            Vec::<String>::new()
        );
        let different = json!({"detail": [{"type": "value_error", "loc": ["query", "limit"]}]});
        let failures = compare_body(
            "s",
            1,
            &json!({"status": 422, "body": recorded}),
            &different,
            "application/json",
        );
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(failures[0].contains("type"), "{failures:?}");
    }

    #[test]
    fn an_allowance_covers_only_the_field_it_names() {
        let allowed_field = AllowedField {
            file: "golden/s/01.json".to_owned(),
            field: "body.detail".to_owned(),
            python_value: Some(json!("Internal Server Error")),
            rust_value: Some(json!("A typed error")),
            matching: None,
            contract: None,
        };
        let recorded = json!({"status": 400, "body": {"detail": "Internal Server Error"}});
        let actual = json!({"detail": "A typed error"});
        assert_eq!(
            compare_body("s", 1, &recorded, &actual, "application/json"),
            vec![
                "s step 1: body.detail \"A typed error\" does not match \"Internal Server Error\""
                    .to_owned()
            ]
        );
        // The same answer is accepted once the allowance is passed in, which the
        // path lookup does by file and field.
        assert!(super::allowed(
            "body.detail",
            &json!("Internal Server Error"),
            &json!("A typed error"),
            &[allowed_field]
        ));
    }

    #[test]
    fn a_prefix_allowance_accepts_a_longer_rust_value_only() {
        let allowed_field = AllowedField {
            file: "golden/s/01.json".to_owned(),
            field: "body.detail".to_owned(),
            python_value: None,
            rust_value: None,
            matching: Some(MatchRule {
                kind: "prefix".to_owned(),
                value: "Not a valid JUnit XML report: ".to_owned(),
            }),
            contract: None,
        };
        assert!(super::allowed(
            "body.detail",
            &json!("Not a valid JUnit XML report: syntax error"),
            &json!("Not a valid JUnit XML report: unexpected token"),
            &[allowed_field]
        ));
        assert!(!super::allowed(
            "body.detail",
            &json!("other"),
            &json!("other"),
            &[AllowedField {
                file: "golden/s/01.json".to_owned(),
                field: "body.detail".to_owned(),
                python_value: None,
                rust_value: None,
                matching: Some(MatchRule {
                    kind: "unknown".to_owned(),
                    value: String::new(),
                }),
                contract: None,
            }]
        ));
    }

    /// The `\x` hex PostgreSQL renders a `bytea` value as.
    fn hex(bytes: &[u8]) -> String {
        let mut text = String::from("\\x");
        for byte in bytes {
            for half in [byte >> 4, byte & 0x0f] {
                text.push(char::from_digit(u32::from(half), 16).unwrap_or('0'));
            }
        }
        text
    }

    /// The hex a table `body` column holds `document` as.
    fn stored(document: &serde_json::Value) -> serde_json::Value {
        serde_json::Value::String(hex(&serde_json::to_vec(document).expect("a document")))
    }

    /// The dump a `reports` table holding one Report with `document` answers.
    fn reports_table(document: &serde_json::Value) -> serde_json::Value {
        json!({"tables": {"reports": [{"id": 1, "status": "pending", "body": stored(document)}]}})
    }

    /// A recorded golden around the document a dump answers.
    fn recorded_dump(answered: &serde_json::Value) -> serde_json::Value {
        json!({"status": 200, "body": answered})
    }

    /// Replaces the recorder's placeholders with the values they stand for.
    fn resolve_placeholders(value: &mut serde_json::Value) {
        if let serde_json::Value::String(text) = value {
            let replacement = match text.as_str() {
                super::TIMESTAMP_PLACEHOLDER => Some(json!("2026-10-20T09:00:00.000000Z")),
                super::DATE_PLACEHOLDER => Some(json!("2026-10-20")),
                _ => None,
            };
            if let Some(replacement) = replacement {
                *value = replacement;
            }
            return;
        }
        if let Some(fields) = value.as_object_mut() {
            for field in fields.values_mut() {
                resolve_placeholders(field);
            }
            return;
        }
        if let Some(items) = value.as_array_mut() {
            for item in items {
                resolve_placeholders(item);
            }
        }
    }

    /// Rewrites every `*_at` text to six fractional digits and a `Z`, as Rust does.
    fn rust_timestamp_spelling(value: &mut serde_json::Value) {
        if let Some(fields) = value.as_object_mut() {
            for (key, field) in fields {
                let respelled = field
                    .as_str()
                    .filter(|_| key.ends_with("_at"))
                    .map(|text| text.replace("+00:00", ".000000Z"));
                match respelled {
                    Some(text) => *field = json!(text),
                    None => rust_timestamp_spelling(field),
                }
            }
            return;
        }
        if let Some(items) = value.as_array_mut() {
            for item in items {
                rust_timestamp_spelling(item);
            }
        }
    }

    #[test]
    fn a_table_body_compares_field_by_field_on_the_real_dump() {
        // The `reports` dump of a step this harness records. Rust writes the
        // fraction Python omits, which `differences.md` allows on a timestamp and on
        // nothing else, so the queued document is compared rather than its hex.
        let recorded = super::golden("pipeline-jobs", 37);
        let tables = recorded
            .get("tables")
            .cloned()
            .expect("the golden dumps the tables");
        let mut answered = json!({"tables": tables});
        resolve_placeholders(&mut answered);
        let golden = recorded_dump(&answered);
        for row in answered
            .get_mut("tables")
            .and_then(|tables| tables.get_mut("reports"))
            .and_then(serde_json::Value::as_array_mut)
            .expect("the dump lists reports")
        {
            // The second Report is the JUnit upload, which is not JSON and keeps
            // its text; every other one is a Pipeline payload, which Rust stores
            // with the timestamp spelling it writes.
            let Some(mut document) = super::table_body(&row["body"]) else {
                continue;
            };
            rust_timestamp_spelling(&mut document);
            row["body"] = stored(&document);
        }
        assert_eq!(
            compare_body("pipeline-jobs", 37, &golden, &answered, "application/json"),
            Vec::<String>::new()
        );

        // A materially different value inside that same body is still a failure:
        // the allowance covers the instant, not the document.
        let mut tampered = answered.clone();
        let mut document = super::table_body(&tampered["tables"]["reports"][0]["body"])
            .expect("a queued upload is JSON");
        document["commit_sha"] = json!("not-c1");
        tampered["tables"]["reports"][0]["body"] = stored(&document);
        let failures = compare_body("pipeline-jobs", 37, &golden, &tampered, "application/json");
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(failures[0].contains("commit_sha"), "{failures:?}");

        // The step's JUnit upload is not JSON: it decodes to none and compares as
        // the text it is, so one changed character is still a failure.
        let mut tampered = answered;
        let upload = tampered["tables"]["reports"][1]["body"]
            .as_str()
            .and_then(|text| text.strip_prefix("\\x"))
            .and_then(super::unhex)
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .expect("the second upload is JUnit XML in hex")
            .replacen("<testsuite", "<testsuitE", 1);
        tampered["tables"]["reports"][1]["body"] = json!(hex(upload.as_bytes()));
        let failures = compare_body("pipeline-jobs", 37, &golden, &tampered, "application/json");
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(failures[0].contains("reports.1.body"), "{failures:?}");
    }

    #[test]
    fn hex_is_decoded_in_a_dumped_table_column_and_nowhere_else() {
        // One document with the recorded `+00:00` on each side would differ only in
        // its spelling, so inside a dumped table column the instant rule covers it.
        let recorded = reports_table(&json!({"created_at": "2026-10-20T09:00:00+00:00"}));
        let answered = reports_table(&json!({"created_at": "2026-10-20T09:00:00.000000Z"}));
        assert_eq!(
            compare_body(
                "s",
                1,
                &recorded_dump(&recorded),
                &answered,
                "application/json"
            ),
            Vec::<String>::new()
        );

        // The same two hex strings in an HTTP response body keep comparing as the
        // text they are: the decoding reaches dumped table columns only.
        let column = |dump: &serde_json::Value| dump["tables"]["reports"][0]["body"].clone();
        let golden = recorded_dump(&json!({"rows": [{"body": column(&recorded)}]}));
        let failures = compare_body(
            "s",
            1,
            &golden,
            &json!({"rows": [{"body": column(&answered)}]}),
            "application/json",
        );
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(failures[0].contains("rows.0.body"), "{failures:?}");

        // The shape that scopes the decoding: one dumped table's row, one column.
        assert!(super::names_a_dumped_column("body.tables.reports.0.body"));
        assert!(!super::names_a_dumped_column("body.detail.body"));
        assert!(!super::names_a_dumped_column("body.tables.reports"));
        assert!(!super::names_a_dumped_column("body.tables.reports..body"));
        assert!(!super::names_a_dumped_column(
            "body.tables.reports.first.body"
        ));
        assert!(!super::names_a_dumped_column(
            "body.tables.reports.0.body.extra"
        ));
    }

    #[test]
    fn a_changed_instant_in_a_table_body_is_still_a_failure() {
        let recorded =
            json!({"jobs": [{"started_at": "2026-10-20T09:00:00+00:00", "status": "failed"}]});
        let golden = recorded_dump(&reports_table(&recorded));
        let answered =
            json!({"jobs": [{"started_at": "2026-10-20T09:00:00.000000Z", "status": "failed"}]});
        assert_eq!(
            compare_body(
                "s",
                1,
                &golden,
                &reports_table(&answered),
                "application/json"
            ),
            Vec::<String>::new()
        );

        // The same field a second later is a different instant, not a difference in
        // spelling, so the rule does not cover it.
        let later =
            json!({"jobs": [{"started_at": "2026-10-20T09:00:01.000000Z", "status": "failed"}]});
        let failures = compare_body("s", 1, &golden, &reports_table(&later), "application/json");
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(failures[0].contains("started_at"), "{failures:?}");

        // A changed status in the same document is still a mismatch.
        let broken =
            json!({"jobs": [{"started_at": "2026-10-20T09:00:00.000000Z", "status": "passed"}]});
        let failures = compare_body("s", 1, &golden, &reports_table(&broken), "application/json");
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(failures[0].contains("status"), "{failures:?}");

        // The rule reaches `*_at` fields only: the same two spellings on a field
        // that is not a timestamp are a difference.
        let labelled = json!({"jobs": [{"started_at": "2026-10-20T09:00:00+00:00", "note": "2026-10-20T09:00:00+00:00"}]});
        let relabelled = json!({"jobs": [{"started_at": "2026-10-20T09:00:00.000000Z", "note": "2026-10-20T09:00:00.000000Z"}]});
        let failures = compare_body(
            "s",
            1,
            &recorded_dump(&reports_table(&labelled)),
            &reports_table(&relabelled),
            "application/json",
        );
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(failures[0].contains("note"), "{failures:?}");
    }
}

#[tokio::test]
async fn a_dump_db_step_lists_the_named_tables() {
    let db = TestDb::new().await.expect("a test database is available");
    let empty = dump_db(&db, &[json!("repos")]).await;
    assert_eq!(empty, json!({"tables": {"repos": []}}));

    let repo = repos::ActiveModel {
        name: sea_orm::Set("acme/app".to_owned()),
        created_at: sea_orm::Set(chrono::Utc::now().fixed_offset()),
        ..Default::default()
    };
    repos::Entity::insert(repo)
        .exec_with_returning(db.connection())
        .await
        .expect("a Repo");
    let dump = dump_db(&db, &[json!("repos")]).await;
    let rows = dump
        .get("tables")
        .and_then(|tables| tables.get("repos"))
        .and_then(Value::as_array)
        .expect("the table holds rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("name"), Some(&json!("acme/app")));
}

/// A router whose `/api/reports/summary` answers `body`.
fn summary_app(body: Value) -> Router {
    Router::new().route(
        "/api/reports/summary",
        get(move || {
            let body = body.clone();
            async move { Json(body) }
        }),
    )
}

#[tokio::test]
async fn a_wait_processed_step_answers_a_drained_queue() {
    let db = TestDb::new().await.expect("a test database is available");
    let app = summary_app(json!({"pending": 0, "failed": 0}));
    assert_eq!(
        wait_processed(&app, &db, &test_settings()).await,
        json!({"processed": true, "reports": {"pending": 0, "failed": 0}})
    );
}

#[tokio::test(start_paused = true)]
async fn a_wait_processed_step_gives_up_on_a_queue_that_never_drains() {
    // The clock is paused, so the deadline passes without waiting for it: the
    // step must report the queue rather than hang the replay.
    let app = summary_app(json!({"pending": 2, "failed": 1}));
    assert_eq!(
        wait_for_drain(&app).await,
        json!({"processed": false, "reports": {"pending": 2, "failed": 1}})
    );
}

#[tokio::test]
async fn an_mcp_step_replays_the_recorded_protocol_result() {
    // The recorded `tools/list` answer of the frozen server, replayed against
    // the Rust transport: the tool names, descriptions, schemas and annotations
    // are the wire, so the golden must replay clean and one changed description
    // must fail.
    let db = TestDb::new().await.expect("a test database is available");
    let app = common::app(&db, &[("FLAKERADAR_API_TOKEN", TOKEN)]);
    let spec = scenario("mcp")
        .steps
        .get(5)
        .cloned()
        .expect("the mcp scenario has a sixth step");
    let recorded = golden("mcp", 6);
    let failures = replay_mcp(&app, "mcp", 6, &spec, &recorded).await;
    assert!(
        failures
            .iter()
            .all(|failure| failure.contains("annotations")
                || failure.contains("execution")
                || failure.contains("icons")
                || failure.contains("resultType")
                || failure.contains("ttlMs")
                || failure.contains("cacheScope")),
        "{failures:?}"
    );

    // A changed description is not one of the fields the differences record, so
    // it is still a mismatch.
    let mut tampered = recorded;
    let first = tampered
        .get_mut("tools")
        .and_then(Value::as_array_mut)
        .and_then(|tools| tools.first_mut())
        .expect("the golden lists its tools");
    first["description"] = json!("List every Repo.");
    let failures = replay_mcp(&app, "mcp", 6, &spec, &tampered).await;
    assert!(
        failures
            .iter()
            .any(|failure| failure.contains("description")),
        "{failures:?}"
    );
}

/// The app the `mcp` scenario's `tools/call` steps answer on, over a database
/// holding the rows the scenario's ingest steps queued.
///
/// A tool answer is data those rows produced, so a control that tampers one has
/// to replay the ingests and the drain first — the setup
/// [`every_enabled_scenario_replays_identically`] does inline for every scenario.
async fn seeded_mcp_app() -> (TestDb, Router) {
    let db = TestDb::new().await.expect("a test database is available");
    let file = scenario("mcp");
    let settings = settings_for(&file, &db);
    let app = build_app(AppState::new(
        db.connection().clone(),
        Arc::new(settings.clone()),
    ));
    // The worker claims nothing until the scenario drains the queue, so the
    // ingest goldens that record a pending Report still replay.
    let gate = Arc::new(GatedProcessor::new());
    let held: Arc<GatedProcessor> = Arc::clone(&gate);
    let processor: Arc<dyn ReportProcessor> = held;
    let worker = Arc::new(
        ReportWorker::new(
            db.connection().clone(),
            Arc::new(AdvisoryLock::new(db.url(), DbSettings::default())),
            processing_of(&settings),
            WorkerSettings::from_settings(&settings),
        )
        .with_processor(processor),
    );
    Arc::clone(&worker).start().await;
    for step in 1..=4 {
        let recorded = golden("mcp", step);
        let spec = file
            .steps
            .get(step - 1)
            .unwrap_or_else(|| panic!("mcp has no step {step}"));
        if spec.get("wait_processed").is_some() {
            gate.open();
        }
        let failures = replay_step(&app, &db, &settings, "mcp", step, spec, &recorded).await;
        assert!(failures.is_empty(), "mcp step {step}: {failures:?}");
    }
    worker.stop().await;
    (db, app)
}

/// The recorded `tools/call` answer with one substring of its content text
/// replaced.
fn with_tampered_text(recorded: &Value, from: &str, to: &str) -> Value {
    let mut tampered = recorded.clone();
    let block = tampered
        .get_mut("result")
        .and_then(|result| result.get_mut("content"))
        .and_then(|content| content.get_mut(0))
        .and_then(Value::as_object_mut)
        .expect("a tools/call golden carries its data as a content block");
    let text = block
        .get("text")
        .and_then(Value::as_str)
        .expect("the block's data is text")
        .to_owned();
    assert!(
        text.contains(from),
        "{from:?} is not in the recorded answer"
    );
    let _ = block.insert("text".to_owned(), json!(text.replace(from, to)));
    tampered
}

#[tokio::test]
async fn a_changed_mcp_tool_answer_fails_the_replay() {
    // A `tools/call` golden holds what an agent acts on, in the JSON text a
    // client reads and in the structured copy beside it. The replay must reject
    // a wrong Test name, score, permalink, Issue number and failure message, so
    // the recorded answer is proved non-vacuous field by field rather than only
    // by the `tools/list` control above.
    let (_db, app) = seeded_mcp_app().await;
    let file = scenario("mcp");
    let get_test = file
        .steps
        .get(18)
        .cloned()
        .expect("the mcp scenario has an eighteenth step");
    let recorded = golden("mcp", 19);
    assert_eq!(
        replay_mcp(&app, "mcp", 19, &get_test, &recorded).await,
        Vec::<String>::new(),
        "the recorded get_test answer must replay clean before a control alters it"
    );

    for (what, from, to) in [
        (
            "a Test name",
            "\"name\":\"test_total\"",
            "\"name\":\"test_totals\"",
        ),
        (
            "a score",
            "\"flakiness_score\":0.1667",
            "\"flakiness_score\":0.1668",
        ),
        (
            "a permalink",
            "blob/m1/tests/test_cart.py#L12",
            "blob/m1/tests/test_cart.py#L13",
        ),
        (
            "a failure message",
            "\"message\":\"assert 1 == 2\"",
            "\"message\":\"assert 2 == 2\"",
        ),
    ] {
        let tampered = with_tampered_text(&recorded, from, to);
        let failures = replay_mcp(&app, "mcp", 19, &get_test, &tampered).await;
        assert_eq!(failures.len(), 1, "{what}: {failures:?}");
        assert!(
            failures[0].contains("result.content.0.text"),
            "{what}: {failures:?}"
        );
    }

    // The structured copy of the same data is compared too, so a changed score
    // fails even when the text a client reads still matches.
    let mut rescored = recorded.clone();
    let _ = rescored
        .get_mut("result")
        .and_then(|result| result.get_mut("structuredContent"))
        .and_then(|structured| structured.get_mut("test"))
        .and_then(Value::as_object_mut)
        .expect("the recorded answer carries the Test")
        .insert("flakiness_score".to_owned(), json!(0.1668));
    let failures = replay_mcp(&app, "mcp", 19, &get_test, &rescored).await;
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(
        failures[0].contains("result.structuredContent.test.flakiness_score"),
        "{failures:?}"
    );

    // The Issue number a write recorded is part of the answer it returned.
    let link = file
        .steps
        .get(37)
        .cloned()
        .expect("the mcp scenario has a thirty-eighth step");
    let recorded = golden("mcp", 38);
    assert_eq!(
        replay_mcp(&app, "mcp", 38, &link, &recorded).await,
        Vec::<String>::new(),
        "the recorded link_issue answer must replay clean before a control alters it"
    );
    let tampered = with_tampered_text(
        &recorded,
        "\"github_issue_number\":31",
        "\"github_issue_number\":32",
    );
    let failures = replay_mcp(&app, "mcp", 38, &link, &tampered).await;
    assert_eq!(failures.len(), 1, "an Issue number: {failures:?}");
    assert!(
        failures[0].contains("result.content.0.text"),
        "an Issue number: {failures:?}"
    );
}

#[tokio::test]
async fn an_enabled_step_that_answers_something_else_fails_loudly() {
    // The replay loop asserts that every mismatch it collects is empty, so this
    // proves a wrong answer becomes one: the same step and spec replay clean
    // against the recorded golden and fail once the recorded status is changed.
    let db = TestDb::new().await.expect("a test database is available");
    let app = common::app(&db, &[]);
    let spec = scenario("health")
        .steps
        .first()
        .cloned()
        .expect("the health scenario has a first step");
    let recorded = golden("health", 1);
    assert_eq!(
        replay_http(&app, "health", 1, &spec, &recorded).await,
        Vec::<String>::new()
    );

    let mut tampered = recorded;
    tampered
        .as_object_mut()
        .expect("a golden is an object")
        .insert("status".to_owned(), json!(500));
    let failures = replay_http(&app, "health", 1, &spec, &tampered).await;
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(failures[0].contains("status"), "{failures:?}");
}

/// The golden directory that holds the JUnit parser's corpus, not a scenario.
const JUNIT_GOLDEN_DIRECTORY: &str = "junit";

/// Every scenario on disk, ordered by name.
fn scenario_files() -> Vec<ScenarioFile> {
    let directory = parity_root().join("scenarios");
    let mut scenarios: Vec<ScenarioFile> = Vec::new();
    for entry in std::fs::read_dir(&directory).expect("the scenarios are readable") {
        let path = entry.expect("a directory entry").path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
        scenarios.push(
            serde_json::from_str(&text)
                .unwrap_or_else(|error| panic!("parsing {}: {error}", path.display())),
        );
    }
    scenarios.sort_by(|left, right| left.name.cmp(&right.name));
    scenarios
}

#[test]
fn every_scenario_step_has_a_golden_and_no_golden_is_orphaned() {
    let scenarios = scenario_files();
    assert!(!scenarios.is_empty(), "the fixtures are committed");
    let mut named: Vec<&str> = Vec::new();
    for scenario in &scenarios {
        assert!(!scenario.steps.is_empty(), "{} has no steps", scenario.name);
        for step in 1..=scenario.steps.len() {
            let path = parity_root()
                .join("golden")
                .join(&scenario.name)
                .join(format!("{step:02}.json"));
            assert!(
                path.is_file(),
                "{} has no golden for step {step}",
                scenario.name
            );
        }
        named.push(&scenario.name);
    }
    // A golden directory no scenario names would never be replayed, which is
    // how a scenario silently stops being covered. `golden/junit` is the JUnit
    // parser's corpus, which has no scenario and is replayed by
    // `flakeradar-core`'s own test.
    for entry in std::fs::read_dir(parity_root().join("golden")).expect("the goldens are readable")
    {
        let path = entry.expect("a directory entry").path();
        if !path.is_dir() {
            continue;
        }
        let directory = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("a golden directory has a name");
        if directory == JUNIT_GOLDEN_DIRECTORY {
            continue;
        }
        assert!(
            named.contains(&directory),
            "the golden directory {directory} is not a scenario"
        );
    }
}

/// The reference's `/openapi.json`, as the replay compares it.
fn recorded_openapi() -> Value {
    golden("trailing-slash", 9)
        .get("body")
        .cloned()
        .expect("the openapi golden carries a body")
}

#[test]
fn the_served_openapi_document_keeps_the_reference_contract() {
    let actual =
        serde_json::to_value(flakeradar::docs::document()).expect("the document serializes");
    assert_eq!(
        compare_openapi("trailing-slash", 9, &recorded_openapi(), &actual),
        Vec::<String>::new()
    );
}

#[test]
fn a_missing_path_method_parameter_or_field_fails_the_openapi_contract() {
    let recorded = recorded_openapi();
    let actual =
        serde_json::to_value(flakeradar::docs::document()).expect("the document serializes");
    assert_eq!(
        compare_openapi("trailing-slash", 9, &recorded, &actual),
        Vec::<String>::new()
    );

    let mut missing_path = actual.clone();
    missing_path["paths"]
        .as_object_mut()
        .expect("the document has paths")
        .remove("/api/jobs");
    let failures = compare_openapi("trailing-slash", 9, &recorded, &missing_path);
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(failures[0].contains("/api/jobs"), "{failures:?}");

    let mut missing_method = actual.clone();
    missing_method["paths"]["/api/reports/{report_id}"]
        .as_object_mut()
        .expect("the path has methods")
        .remove("delete");
    let failures = compare_openapi("trailing-slash", 9, &recorded, &missing_method);
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(failures[0].contains("delete"), "{failures:?}");

    let mut missing_parameter = actual.clone();
    missing_parameter["paths"]["/api/tests"]["get"]["parameters"]
        .as_array_mut()
        .expect("the operation has parameters")
        .pop();
    let failures = compare_openapi("trailing-slash", 9, &recorded, &missing_parameter);
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(failures[0].contains("/api/tests"), "{failures:?}");

    let mut missing_field = actual.clone();
    missing_field["components"]["schemas"]["TestOut"]["properties"]
        .as_object_mut()
        .expect("the schema has properties")
        .remove("quarantined");
    let failures = compare_openapi("trailing-slash", 9, &recorded, &missing_field);
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(failures[0].contains("quarantined"), "{failures:?}");

    let mut renamed_body = actual;
    renamed_body["paths"]["/api/jobs"]["get"]["responses"]["200"]["content"]["application/json"]
        ["schema"]["$ref"] = json!("#/components/schemas/JobPages");
    let failures = compare_openapi("trailing-slash", 9, &recorded, &renamed_body);
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(failures[0].contains("JobPages"), "{failures:?}");
}
