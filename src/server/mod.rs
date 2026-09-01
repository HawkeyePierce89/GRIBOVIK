//! The local HTTP server the reviewer's browser talks to.
//!
//! The graph is computed once, before the server starts, and never changes
//! while it runs — only the review state does. So the snapshot is shared
//! immutably and the state sits behind a mutex, written through to disk on
//! every mutation. A reviewer who kills the process mid-session loses nothing.
//!
//! The API is deliberately tiny: read the graph, read the state, replace the
//! state. Replacing the whole object rather than patching single nodes keeps
//! the client free of merge logic, and the payload is a few kilobytes at worst.

pub mod assets;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};

use crate::core::GraphSnapshot;
use crate::review::{self, ReviewState};
use crate::server::assets::Assets;

/// The address the server binds to.
///
/// Loopback only: the graph carries a diff of unpushed work, and nothing about
/// this tool wants that reachable from the rest of the network.
const HOST: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// Everything the handlers share.
#[derive(Debug)]
pub struct AppState {
    /// The analysis result, fixed for the lifetime of the process.
    snapshot: GraphSnapshot,
    /// The reviewer's verdicts, mutated by `POST /api/state`.
    state: Mutex<ReviewState>,
    /// Where those verdicts are persisted.
    state_path: PathBuf,
    /// Where the SPA's files come from.
    assets: Assets,
}

impl AppState {
    /// The review state, whatever a previous panic did to the mutex.
    ///
    /// The guarded value is a plain map with no invariant a half-finished write
    /// could break, so recovering from poisoning is strictly better than
    /// turning one panicking request into a permanently broken endpoint.
    fn lock(&self) -> std::sync::MutexGuard<'_, ReviewState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Assemble the shared state from an analysis result and a place to keep
    /// the review in.
    pub fn new(
        snapshot: GraphSnapshot,
        state: ReviewState,
        state_path: PathBuf,
        assets: Assets,
    ) -> Self {
        Self {
            snapshot,
            state: Mutex::new(state),
            state_path,
            assets,
        }
    }
}

/// Build the router. Split out from [`serve`] so tests can drive it in-process
/// without binding a port.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/graph", get(get_graph))
        .route("/api/state", get(get_state).post(post_state))
        .route("/", get(index))
        .route("/{*path}", get(asset))
        .layer(axum::middleware::from_fn(reject_foreign_host))
        .layer(axum::middleware::from_fn(deny_framing))
        .with_state(state)
}

/// Forbid every response from being framed.
///
/// The `Host` check turns away an attacker's domain that resolves to loopback,
/// but a page on any origin may still put `http://127.0.0.1:<port>/` in an
/// iframe — the browser sends a loopback `Host` for that, so it passes. The
/// framed page cannot be read cross-origin, but its Approve and Reject buttons
/// can be clicked through, which is enough to falsify a review.
async fn deny_framing(request: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        axum::http::header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("frame-ancestors 'none'"),
    );
    response
        .headers_mut()
        .insert("x-frame-options", HeaderValue::from_static("DENY"));
    response
}

/// Hostnames a browser may legitimately have used to reach a loopback server.
const LOCAL_HOSTS: [&str; 3] = ["127.0.0.1", "localhost", "[::1]"];

/// Refuse requests that did not address this server by a loopback name.
///
/// Binding loopback keeps the network out, but not a page on an attacker's
/// domain whose DNS resolves to 127.0.0.1: to the browser that page is
/// same-origin with this server and can read `/api/graph`, which is the diff of
/// work that has not been pushed anywhere. Checking `Host` is what closes that.
async fn reject_foreign_host(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let host = request
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    // HTTP/2 and HTTP/3 omit `Host` in favour of `:authority`, which axum puts
    // back in the URI; an empty host there means neither was sent.
    let host = if host.is_empty() {
        request.uri().host().unwrap_or_default()
    } else {
        host
    };
    // Split the port off, minding that an IPv6 literal is bracketed.
    let name = match host.rfind(':') {
        Some(colon) if !host.ends_with(']') => &host[..colon],
        _ => host,
    };

    if LOCAL_HOSTS.contains(&name) {
        next.run(request).await
    } else {
        (
            StatusCode::FORBIDDEN,
            "gribovik only answers requests addressed to localhost\n",
        )
            .into_response()
    }
}

/// Serve until Ctrl+C, returning the address that was actually bound.
///
/// `port` 0 asks the OS for a free one, which is why the bound address is
/// reported through `on_bind` rather than assumed by the caller.
pub async fn serve(
    state: Arc<AppState>,
    port: u16,
    on_bind: impl FnOnce(SocketAddr),
) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(SocketAddr::new(HOST, port))
        .await
        .with_context(|| match port {
            0 => "could not bind a local port".to_string(),
            port => format!("could not bind port {port} — is something already using it?"),
        })?;
    let addr = listener
        .local_addr()
        .context("could not read the bound address")?;
    on_bind(addr);

    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")
}

/// Resolve on Ctrl+C. A failure to install the handler is not worth aborting a
/// review over, so it degrades into "never shut down on a signal".
async fn shutdown_signal() {
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// `GET /api/graph` — the precomputed snapshot.
async fn get_graph(State(state): State<Arc<AppState>>) -> Json<GraphSnapshot> {
    Json(state.snapshot.clone())
}

/// `GET /api/state` — every verdict recorded so far.
async fn get_state(State(state): State<Arc<AppState>>) -> Json<ReviewState> {
    Json(state.lock().clone())
}

/// `POST /api/state` — replace the whole state and write it to disk.
///
/// The response only reports whether the write succeeded; the client already
/// knows what it sent, so there is nothing to echo back.
async fn post_state(
    State(state): State<Arc<AppState>>,
    Json(incoming): Json<ReviewState>,
) -> Response {
    // The write happens under the lock so that two overlapping posts land on
    // disk in the order they took it. Dropping the guard first would let an
    // older state win the race and outlive the newer one the browser is showing.
    let mut guard = state.lock();
    let mut incoming = incoming;
    // Stamped here, against the snapshot this process is serving: a verdict
    // arriving now is a verdict on the code the browser is showing, and
    // recording which code that was is what stops the next run from replaying
    // it over a rewritten symbol. The client never computes or reads the
    // field — it only carries it back and forth.
    review::stamp(&mut incoming, &state.snapshot);

    // Disk first, memory second. A 500 tells the browser the verdict is not
    // durable, and `GET /api/state` has to say the same thing: committing the
    // change here anyway would serve the unsaved state back on the next reload,
    // clearing the banner and hiding a loss that only surfaces after exit.
    match review::save(&state.state_path, &incoming) {
        Ok(()) => {
            *guard = incoming;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("could not save review state: {err:#}"),
        )
            .into_response(),
    }
}

/// `GET /` — the SPA shell.
async fn index(State(state): State<Arc<AppState>>) -> Response {
    state.assets.respond("/")
}

/// `GET /*path` — an asset, or the SPA shell for a client-side route.
async fn asset(State(state): State<Arc<AppState>>, AxumPath(path): AxumPath<String>) -> Response {
    state.assets.respond(&path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::snapshot::{ChangeKind, Confidence, DiffLine, DiffTag, Edge, Meta, Node};
    use crate::review::{Comment, NodeReview, Status};
    use axum::body::Body;
    use axum::http::{header, Request};
    use http_body_util::BodyExt;
    use tempfile::TempDir;
    use tower::ServiceExt;

    fn snapshot() -> GraphSnapshot {
        GraphSnapshot {
            meta: Meta {
                repo: "/tmp/repo".to_string(),
                base: "abc123".to_string(),
                head: "HEAD".to_string(),
                files_changed: 1,
                warnings: vec!["could not parse src/broken.rs".to_string()],
            },
            nodes: vec![Node {
                id: "src/a.rs::alpha".to_string(),
                file: "src/a.rs".to_string(),
                name: "alpha".to_string(),
                kind: "function".to_string(),
                change: ChangeKind::Modified,
                diff: vec![DiffLine {
                    tag: DiffTag::Add,
                    old_line: None,
                    new_line: Some(2),
                    text: "    beta();".to_string(),
                }],
            }],
            edges: vec![Edge {
                from: "src/a.rs::alpha".to_string(),
                to: "src/b.rs::beta".to_string(),
                confidence: Confidence::Certain,
            }],
        }
    }

    fn review() -> ReviewState {
        let mut state = ReviewState::new();
        state.insert(
            "src/a.rs::alpha".to_string(),
            NodeReview {
                status: Status::Approved,
                comments: vec![Comment {
                    text: "fine".to_string(),
                    created_at: "2026-09-01T10:00:00.000Z".to_string(),
                }],
                fingerprint: None,
            },
        );
        state
    }

    /// `review()` as the server keeps it: `POST /api/state` stamps every entry
    /// with the fingerprint of the node it describes, so what comes back out
    /// carries one even though the client never sent it.
    fn stamped() -> ReviewState {
        let mut state = review();
        review::stamp(&mut state, &snapshot());
        state
    }

    /// A router over a temp state file, plus the directory keeping it alive.
    fn app(state: ReviewState) -> (TempDir, PathBuf, Router) {
        let dir = TempDir::new().unwrap();
        let path = review::state_path(dir.path(), "abc123", "HEAD");
        let app_state = Arc::new(AppState::new(
            snapshot(),
            state,
            path.clone(),
            Assets::Embedded,
        ));
        (dir, path, router(app_state))
    }

    async fn get(app: &Router, uri: &str) -> Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header(header::HOST, "127.0.0.1:7391")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn post_json(app: &Router, uri: &str, body: String) -> Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header(header::HOST, "127.0.0.1:7391")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn body_bytes(response: Response) -> Vec<u8> {
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec()
    }

    async fn body_json<T: serde::de::DeserializeOwned>(response: Response) -> T {
        serde_json::from_slice(&body_bytes(response).await).unwrap()
    }

    /// The request a DNS-rebinding page would make: same-origin as far as the
    /// browser is concerned, but addressed to the attacker's hostname.
    #[tokio::test]
    async fn a_request_for_a_foreign_host_is_refused() {
        let (_dir, _path, app) = app(ReviewState::new());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/graph")
                    .header(header::HOST, "evil.example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn every_loopback_spelling_is_accepted() {
        for host in [
            "127.0.0.1:7391",
            "localhost:7391",
            "[::1]:7391",
            "localhost",
        ] {
            let (_dir, _path, app) = app(ReviewState::new());

            let response = app
                .oneshot(
                    Request::builder()
                        .uri("/api/graph")
                        .header(header::HOST, host)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::OK, "rejected host {host}");
        }
    }

    /// A loopback `Host` is what a browser sends for an iframe pointed at this
    /// server, so the `Host` check cannot see clickjacking; the headers can.
    #[tokio::test]
    async fn responses_refuse_to_be_framed() {
        let (_dir, _path, app) = app(ReviewState::new());

        let response = get(&app, "/").await;

        assert_eq!(
            response.headers().get(header::CONTENT_SECURITY_POLICY),
            Some(&HeaderValue::from_static("frame-ancestors 'none'"))
        );
        assert_eq!(
            response.headers().get("x-frame-options"),
            Some(&HeaderValue::from_static("DENY"))
        );
    }

    #[tokio::test]
    async fn api_graph_returns_the_snapshot() {
        let (_dir, _path, app) = app(ReviewState::new());

        let response = get(&app, "/api/graph").await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_json::<GraphSnapshot>(response).await, snapshot());
    }

    #[tokio::test]
    async fn api_state_starts_from_the_loaded_state() {
        let (_dir, _path, app) = app(review());

        let response = get(&app, "/api/state").await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_json::<ReviewState>(response).await, review());
    }

    #[tokio::test]
    async fn posting_state_then_getting_it_round_trips() {
        let (_dir, _path, app) = app(ReviewState::new());
        let payload = serde_json::to_string(&review()).unwrap();

        let posted = post_json(&app, "/api/state", payload).await;
        assert_eq!(posted.status(), StatusCode::NO_CONTENT);

        let fetched = get(&app, "/api/state").await;
        assert_eq!(body_json::<ReviewState>(fetched).await, stamped());
    }

    #[tokio::test]
    async fn posting_state_writes_the_file() {
        let (_dir, path, app) = app(ReviewState::new());
        assert!(!path.exists());

        post_json(
            &app,
            "/api/state",
            serde_json::to_string(&review()).unwrap(),
        )
        .await;

        assert_eq!(review::load(&path).0, stamped());
    }

    /// A 500 says the verdict is not on disk, and `GET /api/state` has to keep
    /// saying so. Committing the post in memory anyway would serve it back on
    /// the next reload, clear the browser's banner, and turn a loss the reviewer
    /// was told about into one they only discover after the process exits.
    #[tokio::test]
    async fn a_state_that_could_not_be_written_is_not_kept_in_memory() {
        let dir = TempDir::new().unwrap();
        // A directory where the state file belongs: the rename cannot land.
        let path = review::state_path(dir.path(), "abc123", "HEAD");
        std::fs::create_dir_all(&path).unwrap();
        let app = router(Arc::new(AppState::new(
            snapshot(),
            ReviewState::new(),
            path,
            Assets::Embedded,
        )));

        let posted = post_json(
            &app,
            "/api/state",
            serde_json::to_string(&review()).unwrap(),
        )
        .await;
        assert_eq!(posted.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let fetched = get(&app, "/api/state").await;
        assert_eq!(body_json::<ReviewState>(fetched).await, ReviewState::new());
    }

    /// The fingerprint is what stops the next run from replaying an approval
    /// over a symbol that changed since, and the client never computes it.
    #[tokio::test]
    async fn posting_state_stamps_each_entry_with_its_nodes_fingerprint() {
        let (_dir, _path, app) = app(ReviewState::new());

        post_json(
            &app,
            "/api/state",
            serde_json::to_string(&review()).unwrap(),
        )
        .await;

        let stored = body_json::<ReviewState>(get(&app, "/api/state").await).await;
        assert_eq!(
            stored["src/a.rs::alpha"].fingerprint.as_deref(),
            Some(review::fingerprint(&snapshot().nodes[0]).as_str())
        );
    }

    #[tokio::test]
    async fn posting_state_replaces_rather_than_merges() {
        let (_dir, path, app) = app(review());

        post_json(&app, "/api/state", "{}".to_string()).await;

        assert_eq!(
            body_json::<ReviewState>(get(&app, "/api/state").await).await,
            ReviewState::new()
        );
        assert_eq!(review::load(&path).0, ReviewState::new());
    }

    #[tokio::test]
    async fn malformed_state_is_rejected_without_touching_the_stored_state() {
        let (_dir, path, app) = app(review());

        let response = post_json(&app, "/api/state", "{\"a\": 5}".to_string()).await;

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(!path.exists(), "a rejected post must not write the file");
        assert_eq!(
            body_json::<ReviewState>(get(&app, "/api/state").await).await,
            review()
        );
    }

    #[tokio::test]
    async fn the_root_serves_the_spa_shell() {
        let (_dir, _path, app) = app(ReviewState::new());

        let response = get(&app, "/").await;

        assert_eq!(response.status(), StatusCode::OK);
        let html = String::from_utf8(body_bytes(response).await).unwrap();
        assert!(
            html.contains("<div id=\"root\">"),
            "unexpected body: {html}"
        );
    }

    #[tokio::test]
    async fn an_unknown_path_falls_back_to_index_html() {
        let (_dir, _path, app) = app(ReviewState::new());

        let index = body_bytes(get(&app, "/").await).await;
        let response = get(&app, "/no/such/route").await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_bytes(response).await, index);
    }

    #[tokio::test]
    async fn a_real_asset_is_served_with_its_own_content_type() {
        let (_dir, _path, app) = app(ReviewState::new());
        let index = String::from_utf8(body_bytes(get(&app, "/").await).await).unwrap();
        let script = index
            .split("src=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .expect("index.html should reference a script");

        let response = get(&app, script).await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/javascript"
        );
        assert!(!body_bytes(response).await.is_empty());
    }
}
