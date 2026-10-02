//! Plan 158 — the Claude and Copilot status lines show `✉N` beside the seat
//! while the seat holds N > 0 FYIs, read from the same `whoami` answer.
use std::path::Path;
use std::process::{Command, Stdio};

fn render(root: &Path, shell: &str, script: &Path, whoami: &str) -> String {
    let fake = root.join("bin").join("pij-rs");
    std::fs::write(&fake, format!("#!/bin/sh\nprintf '%s\\n' '{whoami}'\n")).unwrap();
    let mut permissions = std::fs::metadata(&fake).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&fake, permissions).unwrap();
    let mut child = Command::new(shell)
        .arg(script)
        .current_dir(root)
        .env(
            "PATH",
            format!(
                "{}:{}",
                root.join("bin").display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .env("HOME", root)
        .env("PIJ_RS_STATE_DIR", root.join("state"))
        .env("TMUX_PANE", "%21")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("render");
    use std::io::Write as _;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!(r#"{{"workspace":{{"current_dir":"{}"}}}}"#, root.display()).as_bytes())
        .unwrap();
    String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap()
}

#[test]
fn status_lines_show_held_fyis_only_while_some_are_pending() {
    let root = pij_testkit::fresh_dir("pij-fyi-status-lines");
    std::fs::create_dir_all(root.join("bin")).unwrap();
    let state = root.join("state");
    for (shell, script) in [
        (
            "sh",
            pij_harnesses::install_claude_statusline_script(&state).unwrap(),
        ),
        (
            "bash",
            pij_harnesses::install_copilot_statusline_script(&state).unwrap(),
        ),
    ] {
        let held = render(
            &root,
            shell,
            &script,
            r#"{"ok":true,"data":{"id":"pij-fyi-seat","pending_fyis":3}}"#,
        );
        assert!(
            held.contains("⛓ pij-fyi-seat\u{1b}[0m \u{1b}[33m✉3"),
            "{held}"
        );
        for quiet in [
            r#"{"ok":true,"data":{"id":"pij-fyi-seat","pending_fyis":0}}"#,
            r#"{"ok":true,"data":{"id":"pij-fyi-seat"}}"#,
        ] {
            let rendered = render(&root, shell, &script, quiet);
            assert!(rendered.contains("⛓ pij-fyi-seat"), "{rendered}");
            assert!(!rendered.contains('✉'), "{shell}: {rendered}");
        }
    }
    let _ = std::fs::remove_dir_all(root);
}
