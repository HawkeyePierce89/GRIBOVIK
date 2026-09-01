//! The local HTTP server the reviewer's browser talks to.
//!
//! The graph is computed once, before the server starts, and never changes
//! while it runs, so the server is read-only: the snapshot goes out, nothing
//! comes back in.
//!
//! The API is deliberately tiny: read the graph. Everything else is the SPA's
//! static assets.

pub mod assets;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};

use crate::core::GraphSnapshot;
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
    /// Where the SPA's files come from.
    assets: Assets,
}

impl AppState {
    /// Assemble the shared state from an analysis result and an asset source.
    pub fn new(snapshot: GraphSnapshot, assets: Assets) -> Self {
        Self { snapshot, assets }
    }
}

/// Build the router. Split out from [`serve`] so tests can drive it in-process
/// without binding a port.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/graph", get(get_graph))
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
/// framed page cannot be read cross-origin, but keeping it unframeable costs
/// nothing and closes the door for good.
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
    use axum::body::Body;
    use axum::http::{header, Request};
    use http_body_util::BodyExt;
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

    fn app() -> Router {
        router(Arc::new(AppState::new(snapshot(), Assets::Embedded)))
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
        let response = app()
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
            let response = app()
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
    /// server, so the `Host` check cannot see framing; the headers can.
    #[tokio::test]
    async fn responses_refuse_to_be_framed() {
        let app = app();

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
        let app = app();

        let response = get(&app, "/api/graph").await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_json::<GraphSnapshot>(response).await, snapshot());
    }

    #[tokio::test]
    async fn the_root_serves_the_spa_shell() {
        let app = app();

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
        let app = app();

        let index = body_bytes(get(&app, "/").await).await;
        let response = get(&app, "/no/such/route").await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_bytes(response).await, index);
    }

    #[tokio::test]
    async fn a_real_asset_is_served_with_its_own_content_type() {
        let app = app();
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
