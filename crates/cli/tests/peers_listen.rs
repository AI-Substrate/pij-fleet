//! F284: the peer-facing listener a daemon was started with by hand (`--bind
//! <lan>:7461 --insecure-bind`) is lost on a flagless restart (`just bounce-rs`,
//! the LaunchAgent) unless it lives in `peers.toml`. A flagless boot reads the
//! file's `listen`; `insecure_listen` opts in that address, and only it.
//!
//! The addresses are ones this host does not hold (TEST-NET-1, a Tailscale
//! address), so the remote bind fails or is refused by name and nothing listens
//! beyond loopback: the warning says which rule decided.

#![cfg(unix)]

use std::fs;
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef";

/// Boot `pij-rs daemon` with no `--bind` and no `PIJ_RS_BIND`, over a
/// peers.toml holding `extra` at its top, until it says what became of its
/// remote listener; then stop it. Its stdout and stderr.
fn flagless_boot(label: &str, extra: impl FnOnce(u16) -> String) -> (String, String, u16) {
    let root = pij_testkit::fresh_dir(label);
    let state = root.join("state");
    fs::create_dir_all(&state).expect("state dir");
    let port = {
        let reserved = TcpListener::bind("127.0.0.1:0").expect("reserve a port");
        reserved.local_addr().expect("reserved address").port()
    };
    let peers = state.join("peers.toml");
    fs::write(
        &peers,
        format!(
            "machine = \"studio\"\n{}[[peer]]\nalias = \"laptop\"\nurl = \"http://127.0.0.1:1\"\nkey = \"{KEY}\"\n",
            extra(port)
        ),
    )
    .expect("write peers.toml");
    fs::set_permissions(&peers, fs::Permissions::from_mode(0o600)).expect("chmod 600");
    let tmux = pij_testkit::tmux::PrivateTmux::start();
    let stdout = root.join("daemon.stdout");
    let stderr = root.join("daemon.stderr");
    let mut child = Command::new(env!("CARGO_BIN_EXE_pij-rs"))
        .args([
            "--state-dir",
            state.to_str().expect("UTF-8 state path"),
            "daemon",
        ])
        .env_remove("PIJ_RS_BIND")
        .env("CLAUDE_CONFIG_DIR", root.join("claude"))
        .env("HOME", &root)
        .env("PIJ_TMUX_SERVER", tmux.name())
        .stdin(Stdio::null())
        .stdout(fs::File::create(&stdout).expect("stdout file"))
        .stderr(fs::File::create(&stderr).expect("stderr file"))
        .spawn()
        .expect("start the daemon");
    let read = |path: &Path| fs::read_to_string(path).unwrap_or_default();
    let deadline = Instant::now() + pij_testkit::time::scaled(Duration::from_secs(30));
    while !read(&stderr).contains("remote listener") && !read(&stderr).contains("--insecure-bind:")
    {
        if child.try_wait().expect("daemon status").is_some() || Instant::now() > deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if child.try_wait().expect("daemon status").is_none() {
        let interrupted = Command::new("/bin/kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .expect("signal the daemon");
        assert!(interrupted.success());
    }
    child.wait().expect("join the daemon");
    let output = (read(&stdout), read(&stderr), port);
    let _ = fs::remove_dir_all(root);
    output
}

#[test]
fn a_flagless_boot_listens_where_peers_toml_says() {
    // Not Tailscale, not opted in: the file's address is the one check_bind
    // judges (and refuses), on the file's port.
    let (stdout, stderr, port) = flagless_boot("pij-f284-listen", |port| {
        format!("listen = \"192.0.2.1:{port}\"\n")
    });
    assert!(
        stdout.contains(&format!("listening on 127.0.0.1:{port}")),
        "loopback serves on the file's port: stdout={stdout} stderr={stderr}"
    );
    assert!(
        stderr.contains(&format!("refusing to bind 192.0.2.1:{port}")),
        "the file's address was the one judged: stderr={stderr}"
    );

    // Opted in by the file: check_bind lets it through, and only the OS (this
    // host does not hold the address) stops it, never the rule.
    let (stdout, stderr, port) = flagless_boot("pij-f284-insecure", |port| {
        format!("listen = \"192.0.2.1:{port}\"\ninsecure_listen = true\n")
    });
    assert!(
        stdout.contains(&format!("listening on 127.0.0.1:{port}")),
        "stdout={stdout} stderr={stderr}"
    );
    assert!(
        stderr.contains("remote listener NOT started") && !stderr.contains("refusing to bind"),
        "insecure_listen opts the file's address in: stderr={stderr}"
    );
}
