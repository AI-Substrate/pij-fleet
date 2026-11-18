//! The listener: routes, the loopback guard, and the accept loop.
//!
//! The accept loop is the daemon's (`crates/daemon/src/serve.rs`): hyper's
//! header read timeout closes an idle keep-alive connection, and is armed only
//! between requests, so a minutes-long stream is never cut by it.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State as AxumState};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto::Builder;
use hyper_util::server::graceful::GracefulShutdown;
use hyper_util::service::TowerToHyperService;
use tokio::net::TcpListener;

use crate::proxy::{MAX_BODY, Proxy, Settings, error_response, log};

/// The status route (blueprint 003 c-status-route).
pub const STATUS_PATH: &str = "/__pij/status";

/// The reload route.
pub const RELOAD_PATH: &str = "/__pij/reload";

/// How long a connection may sit between requests.
const IDLE_CONNECTION_TIMEOUT: Duration = Duration::from_secs(60);

/// How long a shutdown waits for streams in flight.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(15);

/// The sidecar's routes, behind the loopback guard.
pub fn router(proxy: Arc<Proxy>) -> Router {
    let port = proxy.settings().port;
    Router::new()
        .route(STATUS_PATH, get(status))
        .route(RELOAD_PATH, post(reload))
        .fallback(forward)
        .layer(middleware::from_fn_with_state(port, loopback_only))
        .with_state(proxy)
}

/// Refuse anything a browser or a rebound DNS name could send: any `Origin`
/// header, or a `Host` other than this port on loopback. A seat's client sends
/// neither, so this costs it nothing and keeps a web page from spending the
/// account through the held token.
async fn loopback_only(AxumState(port): AxumState<u16>, request: Request, next: Next) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|host| host.to_str().ok())
        .unwrap_or_default();
    let host_ok = host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}");
    if request.headers().contains_key(header::ORIGIN) || !host_ok {
        log(&format!(
            "refused {} {}: {}",
            request.method(),
            request.uri().path(),
            if host_ok {
                "Origin header present"
            } else {
                "Host is not loopback"
            }
        ));
        return error_response(
            StatusCode::FORBIDDEN,
            "the pij cache proxy serves only loopback clients without an Origin header",
        );
    }
    next.run(request).await
}

async fn status(AxumState(proxy): AxumState<Arc<Proxy>>) -> impl IntoResponse {
    Json(proxy.status().await)
}

/// Takes no body (blueprint 003): any body is ignored.
async fn reload(AxumState(proxy): AxumState<Arc<Proxy>>) -> impl IntoResponse {
    log("reload requested");
    Json(proxy.reload().await)
}

async fn forward(AxumState(proxy): AxumState<Arc<Proxy>>, request: Request<Body>) -> Response {
    let (parts, body) = request.into_parts();
    let path = parts
        .uri
        .path_and_query()
        .map_or("/", |path| path.as_str())
        .to_string();
    let Ok(raw) = axum::body::to_bytes(body, MAX_BODY).await else {
        return error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "request body too large or unreadable",
        );
    };
    proxy
        .forward(parts.method, &path, &parts.headers, raw)
        .await
}

/// Serve `router` on `listener` until `shutdown` resolves, then drain streams
/// in flight for up to 15 s.
pub async fn serve(
    listener: TcpListener,
    router: Router,
    shutdown: impl Future<Output = ()> + Send,
) {
    let graceful = GracefulShutdown::new();
    let mut shutdown = std::pin::pin!(shutdown);
    loop {
        let (stream, _remote): (_, SocketAddr) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(accepted) => accepted,
                Err(error) => {
                    if !matches!(
                        error.kind(),
                        std::io::ErrorKind::ConnectionRefused
                            | std::io::ErrorKind::ConnectionAborted
                            | std::io::ErrorKind::ConnectionReset
                    ) {
                        log(&format!("accept failed: {error}"));
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                    continue;
                }
            },
            () = &mut shutdown => break,
        };
        let mut builder = Builder::new(TokioExecutor::new());
        builder
            .http1()
            .timer(TokioTimer::new())
            .header_read_timeout(IDLE_CONNECTION_TIMEOUT);
        let connection = builder
            .serve_connection_with_upgrades(
                TokioIo::new(stream),
                TowerToHyperService::new(router.clone()),
            )
            .into_owned();
        let connection = graceful.watch(connection);
        tokio::spawn(async move {
            let _ = connection.await;
        });
    }
    drop(listener);
    if tokio::time::timeout(DRAIN_TIMEOUT, graceful.shutdown())
        .await
        .is_err()
    {
        log("streams still open after 15 s; exiting");
    }
}

/// SIGTERM (launchd) or SIGINT.
async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    match signal(SignalKind::terminate()) {
        Ok(mut terminate) => {
            tokio::select! {
                _ = terminate.recv() => {}
                _ = tokio::signal::ctrl_c() => {}
            }
        }
        Err(_) => {
            let _ = tokio::signal::ctrl_c().await;
        }
    }
}

/// Run the sidecar: bind 127.0.0.1:<port> first (so a second copy fails
/// before it launches gh), load and check, then serve until signalled.
///
/// # Errors
/// The port could not be bound.
pub async fn run(settings: Settings) -> Result<(), String> {
    let addr = SocketAddr::from(([127, 0, 0, 1], settings.port));
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|error| format!("cannot listen on {addr}: {error}"))?;
    log(&format!(
        "listening on http://{addr} (pid {})",
        std::process::id()
    ));
    let proxy = Arc::new(Proxy::start(settings).await);
    serve(listener, router(proxy), shutdown_signal()).await;
    log("stopped");
    Ok(())
}
