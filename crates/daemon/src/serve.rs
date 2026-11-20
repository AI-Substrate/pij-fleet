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

/// Serve `router` until `shutdown` resolves, then drain open connections for
/// at most `drain`, and drop whatever is still open (F242).
///
/// Each request carries `ConnectInfo<SocketAddr>`, as
/// `into_make_service_with_connect_info` provided (plan 156 adopt reads it).
///
/// The drain lets a request in flight finish, but a stream never does (`pij
/// tail`, the extensions' event streams: 86 open on the live fleet), and an
/// unbounded wait kept the daemon from ever exiting on SIGINT. After `drain`
/// each remaining connection is dropped, which closes its socket; its client
/// sees the stream end and reconnects to the next daemon.
pub(crate) async fn serve(
    listener: TcpListener,
    router: Router,
    idle: Duration,
    drain: Duration,
    shutdown: impl Future<Output = ()> + Send,
) {
    let graceful = GracefulShutdown::new();
    // Closed when the drain is over (dropped): every connection still open
    // then ends, its socket closed.
    let (drained, closed) = tokio::sync::watch::channel(());
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
        let mut closed = closed.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = connection => {}
                _ = closed.changed() => {}
            }
        });
    }
    drop(listener);
    let _ = tokio::time::timeout(drain, graceful.shutdown()).await;
    drop(drained);
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
    use std::time::{Duration, Instant};

    use axum::Router;
    use axum::body::{Body, Bytes};
    use axum::extract::ConnectInfo;
    use axum::routing::get;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::task::JoinHandle;
    use tokio_stream::StreamExt;

    const IDLE: Duration = Duration::from_millis(300);
    const DRAIN: Duration = Duration::from_secs(2);

    struct Served {
        addr: SocketAddr,
        stop: tokio::sync::oneshot::Sender<()>,
        served: JoinHandle<()>,
    }

    async fn start() -> (SocketAddr, tokio::sync::oneshot::Sender<()>) {
        let served = serving().await;
        (served.addr, served.stop)
    }

    async fn serving() -> Served {
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
            )
            .route(
                "/stream",
                get(|| async {
                    // One line, then never another and never an end: a tail.
                    let open = tokio_stream::once(Ok::<_, std::io::Error>(Bytes::from("open\n")));
                    Body::from_stream(
                        open.chain(tokio_stream::pending::<Result<Bytes, std::io::Error>>()),
                    )
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let served = tokio::spawn(super::serve(listener, router, IDLE, DRAIN, async {
            let _ = stopped.await;
        }));
        Served { addr, stop, served }
    }

    /// F242: a stream that never ends (a `pij tail`) holds the stop for at most
    /// the drain, then is dropped: its client reads the end of the stream.
    #[tokio::test]
    async fn an_open_stream_does_not_hold_the_stop() {
        let Served { addr, stop, served } = serving().await;
        let mut stream = TcpStream::connect(addr).await.expect("connect");
        stream
            .write_all(b"GET /stream HTTP/1.1\r\nHost: t\r\n\r\n")
            .await
            .expect("write");
        let mut seen = Vec::new();
        while !String::from_utf8_lossy(&seen).contains("open") {
            let mut buffer = [0; 1024];
            let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buffer))
                .await
                .expect("the stream opens")
                .expect("read");
            assert_ne!(read, 0, "the stream is open");
            seen.extend_from_slice(&buffer[..read]);
        }
        let started = Instant::now();
        let _ = stop.send(());
        tokio::time::timeout(DRAIN + Duration::from_secs(5), served)
            .await
            .expect("the stop is not held by the stream")
            .expect("serve");
        assert!(started.elapsed() >= DRAIN, "the stream had the drain");
        let mut rest = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut rest))
            .await
            .expect("the stream was dropped")
            .expect("read");
    }

    /// F242: a request in flight when the stop comes still gets its answer.
    #[tokio::test]
    async fn a_request_in_flight_completes_inside_the_drain() {
        let Served { addr, stop, served } = serving().await;
        // A request whose answer (IDLE * 4) takes longer than the stop takes
        // to come, and less than the drain.
        let slow = tokio::spawn(async move {
            let mut stream = TcpStream::connect(addr).await.expect("connect");
            stream
                .write_all(b"GET /slow HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n")
                .await
                .expect("write");
            let mut answer = Vec::new();
            stream.read_to_end(&mut answer).await.expect("read");
            String::from_utf8_lossy(&answer).into_owned()
        });
        tokio::time::sleep(IDLE / 3).await;
        let _ = stop.send(());
        let answer = slow.await.expect("slow request");
        assert!(answer.ends_with("slow"), "{answer}");
        tokio::time::timeout(DRAIN + Duration::from_secs(5), served)
            .await
            .expect("the stop")
            .expect("serve");
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
