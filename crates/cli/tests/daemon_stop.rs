//! F242: the daemon stops on its stop signal even while a client holds an
//! event stream open (a `pij tail`, the extensions' streams), and its tmux
//! control client leaves through its teardown first, never behind the HTTP
//! drain. Before F242 the drain waited for every connection to end, a stream
//! never does, and the stop hung until `bounce-rs` SIGKILLed the daemon: its
//! control client then went with no teardown at all (F238 undone).

use std::process::Command;
use std::time::{Duration, Instant};

use pij_cli::daemon_config;
use pij_daemon::SHUTDOWN_DRAIN_TIMEOUT;
use pij_testkit::time::scaled;
use pij_testkit::tmux::PrivateTmux;

fn tmux(server: &str, args: &[&str]) -> std::process::Output {
    Command::new("tmux")
        .args(["-L", server])
        .args(args)
        .env_remove("TMUX")
        .output()
        .expect("tmux is installed")
}

fn has_control_session(server: &str) -> bool {
    tmux(server, &["has-session", "-t", "=_pij"])
        .status
        .success()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_open_event_stream_does_not_hold_the_stop_and_the_control_client_leaves_first() {
    let state_dir = pij_testkit::fresh_dir("pij-rs-daemon-stop");
    let private = PrivateTmux::start();
    let server = private.name().to_string();
    let mut config = daemon_config("127.0.0.1:0", &state_dir.join("pij.sqlite"), false);
    config.tmux_server = Some(server.clone());
    let daemon = pij_daemon::boot(&config, state_dir.clone())
        .await
        .expect("the daemon boots");

    // An end state, so a long bound costs nothing when it comes (F263: 10 s
    // ran out under a loaded machine).
    let attached = Instant::now() + scaled(Duration::from_secs(30));
    while private.control_clients() == 0 {
        assert!(
            Instant::now() < attached,
            "the control client never attached"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(has_control_session(&server));

    // The stream a `pij tail` holds: open, and never ending on its own.
    let key = std::fs::read_to_string(&daemon.key.path).expect("key");
    let stream = reqwest::Client::new()
        .get(format!("http://{}/v1/events", daemon.addr))
        .bearer_auth(key.trim())
        .send()
        .await
        .expect("the event stream opens");
    assert!(stream.status().is_success(), "{}", stream.status());

    let stopping = tokio::spawn(daemon.shutdown());
    // `_pij` (alone: one client, the idle pane) is killed by the leaving
    // client's teardown while the stream still holds the HTTP drain: before
    // the stop ends, which the stream holds for SHUTDOWN_DRAIN_TIMEOUT. That
    // end state is the bound, not a wall-clock guess (F263: 2 s ran out under
    // a loaded machine).
    while has_control_session(&server) {
        assert!(
            !stopping.is_finished(),
            "_pij was not handled while the stream was open"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::timeout(
        SHUTDOWN_DRAIN_TIMEOUT + scaled(Duration::from_secs(10)),
        stopping,
    )
    .await
    .expect("the open stream does not hold the stop")
    .expect("the stop did not panic")
    .expect("the daemon stopped cleanly");
    assert!(
        tmux(&server, &["has-session", "-t", "=test"])
            .status
            .success(),
        "the server's other session survives"
    );

    drop(stream);
    drop(private);
    let _ = std::fs::remove_dir_all(&state_dir);
}
