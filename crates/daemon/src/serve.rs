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
use std::sync::Arc;

use tokio::net::{TcpListener, UnixListener};

use crate::http::provenance::{ProcReader, socket_peer};

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
        spawn_connection(&graceful, stream, service, idle);
    }
    drop(listener);
    graceful.shutdown().await;
}

/// Serve `router` on the daemon's Unix socket (plan 223 AC3) until
/// `shutdown`. Each connection's peer is read ONCE, at accept, from
/// `peer_cred` and an in-process start read; every request on it carries
/// that [`SocketPeer`] (a failure included, so it counts as neither and never
/// falls back to body identifiers). One read per connection, no launch.
pub(crate) async fn serve_unix(
    listener: UnixListener,
    router: Router,
    idle: Duration,
    daemon_uid: u32,
    reader: Arc<dyn ProcReader>,
    shutdown: impl Future<Output = ()> + Send,
) {
    let graceful = GracefulShutdown::new();
    let mut shutdown = std::pin::pin!(shutdown);
    loop {
        let stream = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => stream,
                Err(error) => {
                    if !is_connection_error(&error) {
                        eprintln!("pij-rs: socket accept failed: {error}");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                    continue;
                }
            },
            () = &mut shutdown => break,
        };
        let peer = socket_peer(&stream, daemon_uid, reader.as_ref());
        let service = router.clone().layer(Extension(peer));
        spawn_connection(&graceful, stream, service, idle);
    }
    drop(listener);
    graceful.shutdown().await;
}

fn spawn_connection<S>(graceful: &GracefulShutdown, stream: S, service: Router, idle: Duration)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
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

/// The daemon's socket path in its state directory.
pub(crate) const SOCKET_FILE: &str = "daemon.sock";

/// Bind `<state-dir>/daemon.sock`: the directory 0700, the socket 0600. A
/// socket file already there is removed ONLY after a connect to it fails: a
/// live daemon answers, so its socket is never taken (the TCP bind, the
/// single-instance guard, ran first anyway). Returns the listener and the
/// daemon's uid, read from the socket it just created.
pub(crate) fn bind_socket(state_dir: &std::path::Path) -> std::io::Result<(UnixListener, u32)> {
    use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _, PermissionsExt as _};
    let path = state_dir.join(SOCKET_FILE);
    std::fs::set_permissions(state_dir, std::fs::Permissions::from_mode(0o700))?;
    match std::fs::symlink_metadata(&path) {
        Ok(existing) if existing.file_type().is_socket() => {
            if std::os::unix::net::UnixStream::connect(&path).is_ok() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AddrInUse,
                    format!("{} answers: another daemon serves it", path.display()),
                ));
            }
            std::fs::remove_file(&path)?;
        }
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("{} exists and is not a socket", path.display()),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    let uid = std::fs::metadata(&path)?.uid();
    Ok((listener, uid))
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

    /// Plan 223 AC3: a socket request carries the peer read at accept (this
    /// process: its pid and in-process start), the socket is 0600 in a 0700
    /// directory, and a stale socket is replaced only once nothing answers it.
    #[tokio::test]
    async fn socket_requests_carry_the_peer_read_at_accept() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::path::PathBuf::from(format!("/tmp/pij-sk{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let stale = std::os::unix::net::UnixListener::bind(dir.join(super::SOCKET_FILE))
            .expect("stale socket");
        assert!(
            super::bind_socket(&dir).is_err(),
            "a socket that answers is never removed"
        );
        drop(stale);
        let (listener, uid) = super::bind_socket(&dir).expect("bind over a dead socket");
        let mode =
            |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join(super::SOCKET_FILE)), 0o600);
        let router =
            Router::new().route(
                "/peer",
                get(
                    |axum::Extension(peer): axum::Extension<
                        crate::http::provenance::SocketPeer,
                    >| async move { format!("{peer:?}") },
                ),
            );
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(super::serve_unix(
            listener,
            router,
            IDLE,
            uid,
            std::sync::Arc::new(crate::http::provenance::KernelReader),
            async {
                let _ = stopped.await;
            },
        ));
        let mut stream = tokio::net::UnixStream::connect(dir.join(super::SOCKET_FILE))
            .await
            .expect("connect");
        stream
            .write_all(b"GET /peer HTTP/1.1\r\nHost: t\r\n\r\n")
            .await
            .expect("write");
        let mut buffer = vec![0; 4096];
        let read = stream.read(&mut buffer).await.expect("read");
        let body = String::from_utf8_lossy(&buffer[..read]).into_owned();
        let me = std::process::id();
        let start = pij_harnesses::proc::observed_start(me)
            .expect("own start")
            .local;
        assert!(
            body.ends_with(&format!(
                "Proven(ProcIdentity {{ pid: {me}, proc_start: {start} }})"
            )),
            "{body}"
        );
        drop(stop);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn requests_still_carry_the_peer_address() {
        let (addr, _stop) = start().await;
        let mut stream = TcpStream::connect(addr).await.expect("connect");
        let local = stream.local_addr().expect("local").to_string();
        assert!(request(&mut stream, "/peer").await.ends_with(&local));
    }
}
