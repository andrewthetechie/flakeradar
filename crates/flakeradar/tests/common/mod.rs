//! Helpers shared by the server's integration tests.
//!
//! Every test drives [`flakeradar::build_app`] with `tower`'s `oneshot`, so no
//! test binds a port or depends on real timing. A test that needs rows gets them
//! from [`flakeradar_test_support`], which gives it a database of its own.

#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "a test helper fails loudly when a fixture cannot be built"
)]
// Each test binary uses a few of these helpers, so the rest look unused to it.
#![allow(
    dead_code,
    reason = "one shared module is compiled into every test binary, and each uses a subset"
)]

pub(crate) mod allowances;

use std::borrow::Cow;
use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, Method, Request, StatusCode};
use flakeradar::{AppState, Asset, AssetSource, Settings, build_app};
use flakeradar_test_support::TestDb;
use serde_json::Value;
use tower::ServiceExt as _;

/// The settings a test runs with: the defaults, plus `pairs`.
#[must_use]
pub(crate) fn settings(pairs: &[(&str, &str)]) -> Settings {
    let values = pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    Settings::from_map(&values).expect("the test's settings are valid")
}

/// The app over `db`, with the defaults plus `pairs` as settings.
pub(crate) fn app(db: &TestDb, pairs: &[(&str, &str)]) -> Router {
    app_with(db, settings(pairs))
}

/// The app over `db`, with `settings`.
pub(crate) fn app_with(db: &TestDb, settings: Settings) -> Router {
    build_app(
        AppState::new(db.connection().clone(), Arc::new(settings)).expect("the app state builds"),
    )
}

/// A stand-in for the embedded frontend: the files, bodies and `ETag`s a test hands it.
#[derive(Debug, Default)]
pub(crate) struct MemoryAssets {
    files: Vec<(&'static str, &'static [u8], &'static str)>,
}

impl MemoryAssets {
    /// Serves exactly `files`, each as its path, its body and its `ETag`.
    pub(crate) fn new(files: &[(&'static str, &'static [u8], &'static str)]) -> Self {
        Self {
            files: files.to_vec(),
        }
    }
}

impl AssetSource for MemoryAssets {
    /// Looks `path` up the way a directory on disk would answer it.
    ///
    /// A `.` or `..` segment is resolved before the lookup, so a request that
    /// climbs out of the root reaches the file it names. The real source is a
    /// table of exact names and could never be climbed out of; making the
    /// double answer as a filesystem does is what lets a test prove the route
    /// refuses the path rather than relying on the table's shape.
    fn get(&self, path: &str) -> Option<Asset> {
        let path = normalize(path);
        self.files
            .iter()
            .find(|(name, _, _)| *name == path)
            .map(|(_, body, etag)| Asset::new(Cow::Borrowed(*body), *etag))
    }
}

/// `path` with its `.` and `..` segments resolved.
fn normalize(path: &str) -> String {
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            name => segments.push(name),
        }
    }
    segments.join("/")
}

/// The app over `db`, serving `files` from memory.
pub(crate) fn app_serving(
    db: &TestDb,
    files: &[(&'static str, &'static [u8], &'static str)],
) -> Router {
    let state = AppState::new(db.connection().clone(), Arc::new(settings(&[])))
        .expect("the app state builds")
        .with_assets(Arc::new(MemoryAssets::new(files)));
    build_app(state)
}

/// One answered request, with its body already read.
#[derive(Debug)]
pub(crate) struct Reply {
    /// The status line's code.
    pub(crate) status: StatusCode,
    /// Every response header.
    pub(crate) headers: HeaderMap,
    /// The whole body.
    pub(crate) body: Vec<u8>,
}

impl Reply {
    /// The body parsed as JSON.
    #[must_use]
    pub(crate) fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "the body is JSON ({error}): {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The body as text.
    #[must_use]
    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// One response header, as text.
    #[must_use]
    pub(crate) fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    }
}

/// Sends `request` to `app`.
pub(crate) async fn send(app: &Router, request: Request<Body>) -> Reply {
    let response = app
        .clone()
        .oneshot(request)
        .await
        .expect("the app answers every request");
    let status = response.status();
    let headers = response.headers().clone();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("the body reads to the end");
    Reply {
        status,
        headers,
        body: body.to_vec(),
    }
}

/// Sends one request with no body.
pub(crate) async fn request(app: &Router, method: Method, uri: &str) -> Reply {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .expect("the fixture request builds");
    send(app, request).await
}

/// Sends one request with no body and `headers`.
pub(crate) async fn request_with_headers(
    app: &Router,
    method: Method,
    uri: &str,
    headers: &[(&str, &str)],
) -> Reply {
    let mut builder = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    send(
        app,
        builder
            .body(Body::empty())
            .expect("the fixture request builds"),
    )
    .await
}

/// Sends one `GET`.
pub(crate) async fn get(app: &Router, uri: &str) -> Reply {
    request(app, Method::GET, uri).await
}

/// Sends one POST with `body` and `headers`.
pub(crate) async fn post(
    app: &Router,
    uri: &str,
    headers: &[(&str, &str)],
    body: impl Into<Body>,
) -> Reply {
    let mut builder = Request::builder().method(Method::POST).uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    send(
        app,
        builder
            .body(body.into())
            .expect("the fixture request builds"),
    )
    .await
}

/// A loopback endpoint that reads a request and then never answers it.
///
/// `wiremock` always finishes a response, so "a call held open with no answer at
/// all" needs a socket of its own. The request is read first, so the call really
/// is in flight, and its socket is then kept open until the test ends: nothing
/// can complete the call except the code under test giving up on it.
pub(crate) struct Unanswered {
    /// The URL the endpoint answers nothing on.
    pub(crate) base: String,
    /// How many requests arrived and were held.
    requests: Arc<std::sync::atomic::AtomicUsize>,
    /// Signalled once per request that arrived and is being held.
    arrived: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<()>>,
    /// The task serving the socket, aborted when the test ends.
    task: tokio::task::JoinHandle<()>,
}

impl Unanswered {
    /// Binds a loopback port and starts holding every request that arrives on it.
    pub(crate) async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port");
        let base = format!(
            "http://{}",
            listener.local_addr().expect("the bound address")
        );
        let (arrived, receiver) = tokio::sync::mpsc::unbounded_channel();
        let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = Arc::clone(&requests);
        let task = tokio::spawn(async move {
            // The sockets are held open rather than read: dropping one would end
            // the request it carries, which is the whole point of this endpoint.
            #[expect(
                clippy::collection_is_never_read,
                reason = "the held sockets keep their requests open and are never read"
            )]
            let mut held = Vec::new();
            while let Ok((mut socket, _)) = listener.accept().await {
                read_request(&mut socket).await;
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let _ = arrived.send(());
                // Held open, unanswered, until this task is aborted.
                held.push(socket);
            }
        });
        Self {
            base,
            requests,
            arrived: tokio::sync::Mutex::new(receiver),
            task,
        }
    }

    /// How many requests the endpoint has been asked for.
    pub(crate) fn requests(&self) -> usize {
        self.requests.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Waits until one request has arrived and is being held.
    pub(crate) async fn arrived(&self) {
        let mut arrived = self.arrived.lock().await;
        arrived
            .recv()
            .await
            .expect("the endpoint accepted a request");
    }
}

impl Drop for Unanswered {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Reads one request's head, and as much of the body as it announced.
async fn read_request(socket: &mut tokio::net::TcpStream) {
    use tokio::io::AsyncReadExt as _;

    let mut buffer = [0_u8; 4096];
    let mut received = Vec::new();
    loop {
        let read = match socket.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };
        received.extend_from_slice(buffer.get(..read).unwrap_or_default());
        let text = String::from_utf8_lossy(&received).to_lowercase();
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let announced = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if body.len() >= announced {
                return;
            }
        }
    }
}
