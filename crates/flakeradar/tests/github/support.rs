//! The fixtures the GitHub tests share: settings, a client, a stub and its answers.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use flakeradar::github::{self, Api, BatchReport};
use flakeradar::settings::Secret;
use flakeradar::settings::Settings;
use flakeradar_core::{model::EntityKind, model::ReportStatus};
use flakeradar_db::processing::ProcessOutcome;
#[expect(
    unused_imports,
    reason = "the module's own users name these helpers through it"
)]
pub(crate) use flakeradar_test_support::{
    CapturedLog, IssueLink as Link, link_of, link_table, stamp, stamp_link, stamp_note,
};
use flakeradar_test_support::{
    TestDb, make_execution, make_job, make_job_execution, make_pipeline, make_project, make_run,
    make_test_case, system_clock,
};
use sea_orm::ConnectionTrait as _;
use sea_orm::{DbBackend, Statement, Value};
use serde_json::Value as Json;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// The GitHub token every request presents, which no log line may carry.
pub(crate) const TOKEN: &str = "ghs_testtoken";

/// The settings a filing test runs with.
pub(crate) fn gh_settings(pairs: &[(&str, &str)]) -> Settings {
    let mut values = vec![
        ("FLAKERADAR_GITHUB_TOKEN", TOKEN),
        ("FLAKERADAR_GITHUB_ISSUE_MIN_SCORE", "0.3"),
    ];
    values.extend_from_slice(pairs);
    let map = values
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect();
    Settings::from_map(&map).expect("the test's settings are valid")
}

/// A stop signal that never fires, so a batch runs to its end.
pub(crate) fn unstopped() -> CancellationToken {
    CancellationToken::new()
}

/// A client pointed at the stub server.
pub(crate) fn api(server: &MockServer) -> Api {
    Api::new(Secret::new(TOKEN), &server.uri()).expect("the HTTP client builds")
}

/// The settings a fixed-note test runs with.
pub(crate) fn fixed_settings(pairs: &[(&str, &str)]) -> Settings {
    let mut values = vec![
        ("FLAKERADAR_GITHUB_TOKEN", TOKEN),
        ("FLAKERADAR_GITHUB_FIXED_STREAK", "3"),
    ];
    values.extend_from_slice(pairs);
    let map = values
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect();
    Settings::from_map(&map).expect("the test's settings are valid")
}

/// The identity of one seeded flaky Test.
///
/// The filing body carries every part of it — the `root` as the Location's
/// directory, and the `suite`, `classname` and `name` as the Test and the
/// fingerprint line a reader copies — so a fixture that changes one changes what
/// an issue body says. The fingerprint is never part of the fixture: it is the
/// digest of the identity, which is what the builder stores and the read layer
/// serves.
#[derive(Debug)]
pub(crate) struct FlakyTest<'a> {
    /// The `owner/name` the Test belongs to.
    pub repo: &'a str,
    /// The Project the Test lives in.
    pub project: &'a str,
    /// The Project's directory, or `None` for the Repo root.
    pub root: Option<&'a str>,
    /// The suite half of the Test's identity.
    pub suite: &'a str,
    /// The classname half of the Test's identity.
    pub classname: &'a str,
    /// The Test's name.
    pub name: &'a str,
}

/// One flaky Test with `identity`, and one failing Execution it can show.
async fn seed_flaky_test(db: &TestDb, identity: FlakyTest<'_>) -> i32 {
    let mut project = make_project().repo(identity.repo).project(identity.project);
    if let Some(root) = identity.root {
        project = project.root(root);
    }
    let project = project
        .build(db.connection())
        .await
        .expect("the Project inserts");
    let test = make_test_case(&project)
        .name(identity.name)
        .classname(identity.classname)
        .suite(identity.suite)
        .flakiness_score(0.5)
        .confirmed_flake_count(2)
        .file("tests/test_cart.py")
        .line(12)
        .build(db.connection())
        .await
        .expect("the Test inserts");
    let run = make_run(&project)
        .commit_sha("abcdef1234567890")
        .branch("main")
        .build(db.connection())
        .await
        .expect("the Run inserts");
    make_execution(&test, &run)
        .status("failed")
        .message("AssertionError: expected 3")
        .details("Traceback (most recent call last):\n  ...")
        .build(db.connection())
        .await
        .expect("the Execution inserts");
    test.id
}

/// One flaky Test in `acme/app`'s `backend` Project, with one failing Execution.
pub(crate) async fn flaky_test(db: &TestDb) -> i32 {
    seed_flaky_test(
        db,
        FlakyTest {
            repo: "acme/app",
            project: "backend",
            root: Some("backend"),
            suite: "unit",
            classname: "tests.test_cart",
            name: "test_total",
        },
    )
    .await
}

/// One flaky Test named `name` in `repo`/`project`, with one failing Execution.
pub(crate) async fn flaky_test_in(db: &TestDb, repo: &str, project: &str, name: &str) -> i32 {
    seed_flaky_test(
        db,
        FlakyTest {
            repo,
            project,
            root: None,
            suite: "unit",
            classname: &format!("tests.test_{name}"),
            name,
        },
    )
    .await
}

/// One flaky github-provider Job in `acme/app`.
pub(crate) async fn flaky_job(db: &TestDb) -> i32 {
    let pipeline = make_pipeline()
        .repo("acme/app")
        .provider("github")
        .build(db.connection())
        .await
        .expect("the Pipeline inserts");
    let job = make_job(&pipeline)
        .name("unit")
        .flakiness_score(0.5)
        .confirmed_flake_count(1)
        .build(db.connection())
        .await
        .expect("the Job inserts");
    make_job_execution(&job)
        .status("failed")
        .build(db.connection())
        .await
        .expect("the Job execution inserts");
    job.id
}

/// Mounts the create-issue endpoint with `answer`.
pub(crate) async fn mount_create(server: &MockServer, answer: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path("/repos/acme/app/issues"))
        .respond_with(answer)
        .mount(server)
        .await;
}

/// The `github_issue_*` columns of one Test.
/// Reads one Test's or Job's Issue link.
/// Stamps one Test's link columns, as an earlier pass would have.
/// A processed Report outcome for `tests` and `jobs`.
pub(crate) fn processed(tests: &[i32], jobs: &[i32]) -> ProcessOutcome {
    ProcessOutcome {
        report_id: 1,
        status: ReportStatus::Processed,
        run_id: Some(1),
        counts: None,
        touched_test_ids: tests.to_vec(),
        error: None,
        touched_job_ids: jobs.to_vec(),
        tier_changes: Vec::new(),
        retry_count: 0,
    }
}

/// Mounts the issue-state read of `acme/app` issue 12 with `answer`.
pub(crate) async fn mount_state(server: &MockServer, answer: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path("/repos/acme/app/issues/12"))
        .respond_with(answer)
        .mount(server)
        .await;
}

/// The issue body the fixture input must produce, line for line.
///
/// The input is a `<failure message="boom older">` with details, then a bare
/// `<failure/>` on the next commit, then a pass on that same commit. The newest
/// failing row carries no text, so the sample comes from the older one, and
/// `wire-notes.md` records where the body's text comes from.
pub(crate) const RECORDED_BODY: &str = "FlakeRadar detected a flaky test: `tests.test_cart::test_total`

- **Repo / project:** `acme/app` / `backend`
- **Location:** `backend/tests/test_cart.py` line 12
- **Permalink (last failing commit):** https://github.com/acme/app/blob/s2/backend/tests/test_cart.py#L12
- **Flakiness score:** 0.50 (filed at ≥ 0.30)
- **Proven flakes (same commit failed *and* passed):** 2
- **Suite / classname:** `unit` / `tests.test_cart`
- **Last failing commit:** `s2` on branch `main`

FlakeRadar flags this test because it reports nondeterministic results (a failure gives way to a
pass on identical code). To reproduce it locally:

```
pytest backend/tests/test_cart.py::test_total
```

Then look in the traceback below for a shared resource the test depends on: timing, execution
order, shared/global state, or the network. Re-run the test several times to confirm.

### Last 10 executions

| Status | Commit | Branch | When (UTC) |
|---|---|---|---|
| passed | `s2` | main | 2026-03-04 05:23 |
| failed | `s2` | main | 2026-03-04 05:22 |
| failed | `s1` | main | 2026-03-04 05:11 |

### Sample failure

```
older details
```

_Fingerprint: `25cdd150e8f4335ddfb17485d80e2defa6b56643`_";

/// One flaky github-provider Job named `name` in `repo`, under `pipeline`.
pub(crate) async fn flaky_job_in(db: &TestDb, repo: &str, pipeline: &str, name: &str) -> i32 {
    let pipeline = make_pipeline()
        .repo(repo)
        .provider("github")
        .name(pipeline)
        .build(db.connection())
        .await
        .expect("the Pipeline inserts");
    let job = make_job(&pipeline)
        .name(name)
        .flakiness_score(0.5)
        .confirmed_flake_count(1)
        .build(db.connection())
        .await
        .expect("the Job inserts");
    make_job_execution(&job)
        .status("failed")
        .ci_job_id(name)
        .build(db.connection())
        .await
        .expect("the Job execution inserts");
    job.id
}

/// The lines of `logged` that carry `needle`.
pub(crate) fn lines_with<'a>(logged: &'a str, needle: &str) -> Vec<&'a str> {
    logged
        .lines()
        .filter(|line| line.contains(needle))
        .collect()
}

/// The paths a stub server was asked for, in order.
pub(crate) async fn requested_paths(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .expect("the stub records requests")
        .iter()
        .map(|request| request.url.path().to_owned())
        .collect()
}

/// A failing Test uploaded by the same job attempt, so the Job's failure is explained.
pub(crate) async fn explain_job_execution(db: &TestDb, ci_job_id: &str) {
    let project = make_project()
        .repo("acme/app")
        .project(&format!("explained-{ci_job_id}"))
        .build(db.connection())
        .await
        .expect("the Project inserts");
    let test = make_test_case(&project)
        .name(&format!("t-{ci_job_id}"))
        .flakiness_score(0.0)
        .confirmed_flake_count(0)
        .build(db.connection())
        .await
        .expect("the Test inserts");
    let run = make_run(&project)
        .commit_sha("s")
        .ci_job_id(ci_job_id)
        .build(db.connection())
        .await
        .expect("the Run inserts");
    make_execution(&test, &run)
        .status("failed")
        .build(db.connection())
        .await
        .expect("the Execution inserts");
}

/// Stamps one Job's link columns, as an earlier pass would have.
/// Whether one Job's link carries a check stamp.
/// Mounts `GET /repos/{repo}/issues/{number}` answering `answer`.
pub(crate) async fn mount_issue(
    server: &MockServer,
    repo: &str,
    number: i32,
    answer: ResponseTemplate,
) {
    Mock::given(method("GET"))
        .and(path(format!("/repos/{repo}/issues/{number}")))
        .respond_with(answer)
        .mount(server)
        .await;
}

/// Mounts a comment `POST` and a close `PATCH` for one issue.
pub(crate) async fn mount_close(server: &MockServer, number: i32, status: u16, is_patch: bool) {
    let matcher = if is_patch {
        method("PATCH")
    } else {
        method("POST")
    };
    let route = if is_patch {
        format!("/repos/acme/app/issues/{number}")
    } else {
        format!("/repos/acme/app/issues/{number}/comments")
    };
    Mock::given(matcher)
        .and(path(route))
        .respond_with(ResponseTemplate::new(status))
        .mount(server)
        .await;
}

/// A complete `201 Created` answer with a nine-byte JSON body.
pub(crate) const CREATED: &str = "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\n\
                      Content-Length: 9\r\nConnection: close\r\n\r\n{\"id\": 1}";

/// The same answer whose body dies after three of the nine bytes it promised.
pub(crate) const TRUNCATED: &str = "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\n\
                         Content-Length: 9\r\nConnection: close\r\n\r\n{\"i";

/// A loopback endpoint that answers its first request and hangs up on the rest.
///
/// `wiremock` always finishes a response, so the two transport failures that
/// follow a status line — a body that dies mid-flight, and a connection that
/// dies between two calls — need a real socket.
pub(crate) struct Loopback {
    /// The base URL to hand [`Api::new`].
    pub(crate) base: String,
    /// How many connections the endpoint accepted.
    connections: Arc<AtomicUsize>,
    /// The task serving the socket, aborted when the test ends.
    task: tokio::task::JoinHandle<()>,
}

impl Loopback {
    /// Starts an endpoint that answers its first request with `first`.
    ///
    /// Every later connection is accepted and closed without an answer at all.
    pub(crate) async fn answering(first: String) -> Self {
        Self::start(Some(first)).await
    }

    /// Starts an endpoint that hangs up on every connection it accepts.
    pub(crate) async fn dropping() -> Self {
        Self::start(None).await
    }

    /// Starts an endpoint that answers its first request with `first`, if there is
    /// one, and hangs up on every later connection.
    pub(crate) async fn start(first: Option<String>) -> Self {
        use tokio::io::AsyncWriteExt as _;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port");
        let base = format!(
            "http://{}",
            listener.local_addr().expect("the bound address")
        );
        let connections = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&connections);
        let task = tokio::spawn(async move {
            let mut first = first;
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                counter.fetch_add(1, Ordering::SeqCst);
                read_request(&mut socket).await;
                if let Some(answer) = first.take() {
                    let _ = socket.write_all(answer.as_bytes()).await;
                }
                let _ = socket.shutdown().await;
            }
        });
        Self {
            base,
            connections,
            task,
        }
    }

    /// How many connections the endpoint accepted.
    pub(crate) fn accepted(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Reads one request off `socket`, head and body, so the answer cannot race it.
pub(crate) async fn read_request(socket: &mut tokio::net::TcpStream) {
    use tokio::io::AsyncReadExt as _;

    let mut buffer = [0_u8; 4096];
    let mut received = Vec::new();
    loop {
        let read = match socket.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };
        if let Some(chunk) = buffer.get(..read) {
            received.extend_from_slice(chunk);
        }
        let text = String::from_utf8_lossy(&received).to_lowercase();
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let expected = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if body.len() >= expected {
                return;
            }
        }
    }
}

/// A stub answer that reports its request as arrived and waits to be released.
///
/// Holding a GitHub call open this way keeps "while a call is in flight"
/// deterministic: no part of the test waits on elapsed time.
pub(crate) struct Held {
    arrived: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    released: std::sync::atomic::AtomicBool,
    answer: Json,
}

impl Held {
    /// A gate whose first request is reported on `arrived` and held until released.
    pub(crate) fn new(
        arrived: tokio::sync::oneshot::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
        answer: Json,
    ) -> Self {
        Self {
            arrived: std::sync::Mutex::new(Some(arrived)),
            release: std::sync::Mutex::new(release),
            released: std::sync::atomic::AtomicBool::new(false),
            answer,
        }
    }
}

impl wiremock::Respond for Held {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        let arrived = self
            .arrived
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(arrived) = arrived {
            let _ = arrived.send(());
        }
        if !self.released.load(Ordering::SeqCst) {
            let _ = self
                .release
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .recv_timeout(std::time::Duration::from_secs(30));
            self.released.store(true, Ordering::SeqCst);
        }
        ResponseTemplate::new(200).set_body_json(self.answer.clone())
    }
}

/// Sets one Test's or Job's Clean streak.
pub(crate) async fn set_clean_streak(db: &TestDb, table: &str, id: i32, streak: i32) {
    db.connection()
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!("UPDATE {table} SET clean_streak = $2 WHERE id = $1"),
            [Value::from(id), Value::from(streak)],
        ))
        .await
        .expect("the streak writes");
}

/// Whether one Test or Job carries the fixed-note stamp.
pub(crate) async fn noted(db: &TestDb, table: &str, id: i32) -> bool {
    let row = db
        .connection()
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!(
                "SELECT github_issue_fixed_noted_at IS NOT NULL AS noted FROM {table} WHERE id = $1"
            ),
            [Value::from(id)],
        ))
        .await
        .expect("the row reads")
        .expect("the row exists");
    row.try_get("", "noted").expect("noted")
}

/// Mounts the comment endpoint of issue `number`.
pub(crate) async fn mount_comment(
    server: &MockServer,
    repo: &str,
    number: i32,
    answer: ResponseTemplate,
) {
    Mock::given(method("POST"))
        .and(path(format!("/repos/{repo}/issues/{number}/comments")))
        .respond_with(answer)
        .mount(server)
        .await;
}

/// Mounts the close endpoint of issue `number`.
pub(crate) async fn mount_patch(
    server: &MockServer,
    repo: &str,
    number: i32,
    answer: ResponseTemplate,
) {
    Mock::given(method("PATCH"))
        .and(path(format!("/repos/{repo}/issues/{number}")))
        .respond_with(answer)
        .mount(server)
        .await;
}

/// Runs one statement on the test's own pool from inside a stub's synchronous answer.
///
/// The fixed-note step POSTs the comment and stamps the row afterwards, so the
/// only place a concurrent admin delete or rescore can commit is while the
/// request is in flight. `Respond::respond` is synchronous and sqlx drives its
/// own socket thread, so blocking on the statement here completes even though
/// the stub's worker thread holds the runtime.
pub(crate) fn stage_mid_request(db: &sea_orm::DatabaseConnection, sql: String) {
    block_on(db.execute_raw(Statement::from_string(DbBackend::Postgres, sql)))
        .expect("the staged statement commits");
}

/// Makes every write to `table` fail, the way a broken link store does.
///
/// A trigger that raises is a statement the server refuses, which no savepoint
/// can rescue: that is what separates a store failure from a row deleted between
/// the filing `POST` and its store.
pub(crate) async fn fail_link_stores(db: &TestDb, table: &str) {
    for statement in [
        "CREATE FUNCTION fail_link() RETURNS trigger AS $$ BEGIN \
         RAISE EXCEPTION 'the link store is broken'; END; $$ LANGUAGE plpgsql"
            .to_owned(),
        format!(
            "CREATE TRIGGER fail_link BEFORE UPDATE ON {table} \
             FOR EACH ROW EXECUTE FUNCTION fail_link()"
        ),
    ] {
        db.connection()
            .execute_raw(Statement::from_string(DbBackend::Postgres, statement))
            .await
            .expect("the failing trigger installs");
    }
}

/// Drives one future to completion on the current thread.
///
/// The stub's `Respond::respond` is synchronous, so a mid-request database change
/// has to drive its query here. sqlx runs each connection's socket on its own
/// thread, so this thread can block without the reactor it is holding.
pub(crate) fn block_on<F: Future>(future: F) -> F::Output {
    use std::pin::pin;
    use std::task::{Context, Poll, Wake, Waker};

    /// Wakes the thread that parked on one pending poll.
    struct Unpark(std::thread::Thread);

    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }

    let mut future = pin!(future);
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
        std::thread::park();
    }
}

/// A Test or Job named `name` in `repo`, linked to `number` when one is given,
/// and at `streak` passing runs.
pub(crate) async fn note_row(
    db: &TestDb,
    kind: EntityKind,
    repo: &str,
    name: &str,
    number: Option<i32>,
    streak: i32,
) -> i32 {
    let id = match kind {
        EntityKind::Test => flaky_test_in(db, repo, name, name).await,
        EntityKind::Job => {
            flaky_job_in(db, repo, &format!(".github/workflows/{name}.yml"), name).await
        }
    };
    if let Some(number) = number {
        stamp_link(db, kind, id, stamp().number(number)).await;
    }
    set_clean_streak(db, link_table(kind), id, streak).await;
    id
}

/// One flaky Test or Job in `repo` named `name`, with one failing Execution.
///
/// The filing scenarios need a row of one kind and nothing else about it, so the
/// kind is the only thing a per-entity test supplies.
pub(crate) async fn flaky_row(db: &TestDb, kind: EntityKind, repo: &str, name: &str) -> i32 {
    match kind {
        EntityKind::Test => flaky_test_in(db, repo, name, name).await,
        EntityKind::Job => {
            flaky_job_in(db, repo, &format!(".github/workflows/{name}.yml"), name).await
        }
    }
}

/// One filing batch for `kind`, over `ids`, against `server`.
///
/// The two entry points differ only in the clock the Job batch stamps with, so a
/// scenario runs one call and the entity is the test's whole difference.
pub(crate) async fn file_batch(
    kind: EntityKind,
    db: &TestDb,
    ids: &[i32],
    server: &MockServer,
) -> BatchReport {
    let settings = gh_settings(&[]);
    match kind {
        EntityKind::Test => {
            github::file_issues_for(db.connection(), ids, &settings, &api(server), &unstopped())
                .await
        }
        EntityKind::Job => {
            github::file_job_issues_for(
                db.connection(),
                ids,
                &settings,
                &api(server),
                &system_clock(),
                &unstopped(),
            )
            .await
        }
    }
}

/// [`file_batch`], with the lines it logged as well.
pub(crate) async fn file_batch_logged(
    kind: EntityKind,
    db: &TestDb,
    ids: &[i32],
    server: &MockServer,
) -> (BatchReport, String) {
    CapturedLog::during_call(file_batch(kind, db, ids, server)).await
}

/// One fixed-note batch for `kind`, over `ids`, through `api`.
pub(crate) async fn note_batch(
    kind: EntityKind,
    db: &TestDb,
    ids: &[i32],
    api: &Api,
) -> BatchReport {
    let settings = fixed_settings(&[]);
    match kind {
        EntityKind::Test => {
            github::note_fixed_issues_for(
                db.connection(),
                ids,
                &settings,
                api,
                &system_clock(),
                &unstopped(),
            )
            .await
        }
        EntityKind::Job => {
            github::note_fixed_job_issues_for(
                db.connection(),
                ids,
                &settings,
                api,
                &system_clock(),
                &unstopped(),
            )
            .await
        }
    }
}

/// One closed-issue sync over the rows `db` holds, against `server`.
///
/// Every sync scenario makes this call and reads the same report back; what
/// differs between them is the rows it seeded.
pub(crate) async fn sync_batch(db: &TestDb, server: &MockServer) -> BatchReport {
    github::sync_closed_issues(
        db.connection(),
        &gh_settings(&[]),
        &api(server),
        &system_clock(),
        &unstopped(),
    )
    .await
}

/// The method and path of every request a stub server was asked for, in order.
pub(crate) async fn requested_calls(server: &MockServer) -> Vec<(String, String)> {
    server
        .received_requests()
        .await
        .expect("the stub records requests")
        .iter()
        .map(|request| {
            (
                request.method.as_str().to_owned(),
                request.url.path().to_owned(),
            )
        })
        .collect()
}

/// The Report outcome that touches `ids` of `kind`, and nothing else.
pub(crate) fn processed_for(kind: EntityKind, ids: &[i32]) -> ProcessOutcome {
    match kind {
        EntityKind::Test => processed(ids, &[]),
        EntityKind::Job => processed(&[], ids),
    }
}

/// One Test's or Job's Clean streak.
pub(crate) async fn clean_streak_of(db: &TestDb, table: &str, id: i32) -> i32 {
    let row = db
        .connection()
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!("SELECT clean_streak FROM {table} WHERE id = $1"),
            [Value::from(id)],
        ))
        .await
        .expect("the row reads")
        .expect("the row exists");
    row.try_get("", "clean_streak").expect("a streak")
}

/// One Test's or Job's stored issue number.
pub(crate) async fn number_of(db: &TestDb, table: &str, id: i32) -> Option<i32> {
    let row = db
        .connection()
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!("SELECT github_issue_number FROM {table} WHERE id = $1"),
            [Value::from(id)],
        ))
        .await
        .expect("the row reads")
        .expect("the row exists");
    row.try_get("", "github_issue_number").expect("a number")
}
