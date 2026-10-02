//! The HTTP accept loop, with an idle-connection timeout.
//!
//! `axum::serve` keeps an idle keep-alive connection open forever. On
//! 2026-09-27 one client (a dev server that never finished its responses)
//! leaked about 150 connections a minute into the daemon, which first froze it
//! at launchd's 256 open-file limit and would have frozen it again at any
//! limit. hyper's header read timeout closes a connection that has waited that
//! long for its NEXT request head. It is armed only between requests, so a
//! request in flight (a long-poll, a spawn wait) is never cut by it.

use std::future::Future;
use std::net::SocketAddr;
use std::time::Duration;

use axum::Extension;
use axum::Router;
use axum::extract::ConnectInfo;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto::Builder;
use hyper_util::server::graceful::GracefulShutdown;
use hyper_util::service::TowerToHyperService;
use tokio::net::TcpListener;

/// How long a connection may sit between requests before the daemon closes it.
pub(crate) const IDLE_CONNECTION_TIMEOUT: Duration = Duration::from_secs(60);

/// Serve `router` until `shutdown` resolves, then drain open connections.
///
/// Each request carries `ConnectInfo<SocketAddr>`, as
/// `into_make_service_with_connect_info` provided (plan 156 adopt reads it).
pub(crate) async fn serve(
    listener: TcpListener,
    router: Router,
    idle: Duration,
    shutdown: impl Future<Output = ()> + Send,
) {
    let graceful = GracefulShutdown::new();
    let mut shutdown = std::pin::pin!(shutdown);
    loop {
        let (stream, remote) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(accepted) => accepted,
                Err(error) => {
                    // As axum does: a per-connection failure is retried at once;
                    // anything else (EMFILE) backs off instead of spinning.
                    if !is_connection_error(&error) {
                        eprintln!("pij-rs: accept failed: {error}");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                    continue;
                }
            },
            () = &mut shutdown => break,
        };
        let service = router
            .clone()
            .layer(Extension(ConnectInfo::<SocketAddr>(remote)));
        let mut builder = Builder::new(TokioExecutor::new());
        builder
            .http1()
            .timer(TokioTimer::new())
            .header_read_timeout(idle);
        let connection = builder
            .serve_connection_with_upgrades(TokioIo::new(stream), TowerToHyperService::new(service))
            .into_owned();
        let connection = graceful.watch(connection);
        tokio::spawn(async move {
            let _ = connection.await;
        });
    }
    drop(listener);
    graceful.shutdown().await;
}

fn is_connection_error(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionReset
    )
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::time::Duration;

    use axum::Router;
    use axum::extract::ConnectInfo;
    use axum::routing::get;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    const IDLE: Duration = Duration::from_millis(300);

    async fn start() -> (SocketAddr, tokio::sync::oneshot::Sender<()>) {
        let router = Router::new()
            .route("/fast", get(|| async { "fast" }))
            .route(
                "/slow",
                get(|| async {
                    tokio::time::sleep(IDLE * 4).await;
                    "slow"
                }),
            )
            .route(
                "/peer",
                get(|ConnectInfo(peer): ConnectInfo<SocketAddr>| async move { peer.to_string() }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (stop, stopped) = tokio::sync::oneshot::channel();
        tokio::spawn(super::serve(listener, router, IDLE, async {
            let _ = stopped.await;
        }));
        (addr, stop)
    }

    async fn request(stream: &mut TcpStream, path: &str) -> String {
        stream
            .write_all(format!("GET {path} HTTP/1.1\r\nHost: t\r\n\r\n").as_bytes())
            .await
            .expect("write");
        let mut buffer = vec![0; 4096];
        let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buffer))
            .await
            .expect("response within 5 s")
            .expect("read");
        String::from_utf8_lossy(&buffer[..read]).into_owned()
    }

    #[tokio::test]
    async fn an_idle_keep_alive_connection_is_closed() {
        let (addr, _stop) = start().await;
        let mut stream = TcpStream::connect(addr).await.expect("connect");
        assert!(request(&mut stream, "/fast").await.ends_with("fast"));
        // Keep-alive: the server now waits for the next request head. After the
        // idle timeout it must close, which the client reads as EOF.
        let mut buffer = [0; 64];
        let read = tokio::time::timeout(IDLE * 10, stream.read(&mut buffer))
            .await
            .expect("server closed the idle connection");
        assert_eq!(read.expect("read"), 0);
    }

    #[tokio::test]
    async fn a_request_in_flight_longer_than_the_timeout_is_not_cut() {
        let (addr, _stop) = start().await;
        let mut stream = TcpStream::connect(addr).await.expect("connect");
        assert!(request(&mut stream, "/slow").await.ends_with("slow"));
    }

    #[tokio::test]
    async fn requests_still_carry_the_peer_address() {
        let (addr, _stop) = start().await;
        let mut stream = TcpStream::connect(addr).await.expect("connect");
        let local = stream.local_addr().expect("local").to_string();
        assert!(request(&mut stream, "/peer").await.ends_with(&local));
    }
}
