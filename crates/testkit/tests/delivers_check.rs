//! Plan 209, F65: `pij-delivers-check` passes `none` and full ddocs addresses,
//! and refuses anything `pij delivered` would refuse after the merge,
//! including the PR template's own placeholder.
use std::io::Write;
use std::process::{Command, Stdio};

fn check(body: &str) -> (bool, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_pij-delivers-check"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run pij-delivers-check");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(body.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned()
            + &String::from_utf8_lossy(&output.stderr),
    )
}

#[test]
fn a_pr_body_passes_only_with_none_or_full_addresses() {
    let address = "docs/project/prds/001-pij-community.dd.json#requirements/req-07";
    let step = "docs/project/roadmaps/001-pij-community.dd.json#steps/s05";
    let (ok, out) = check(&format!("## What changed\n\nDelivers: {address}, {step}\n"));
    assert!(ok, "{out}");
    assert!(out.contains("2 address(es)"), "{out}");
    let (ok, out) = check("Delivers: none\n");
    assert!(ok, "{out}");

    // #138's line, and #132's prose one.
    for bad in [
        "Delivers: no roadmap step or state change",
        "Delivers: roadmap 001 s07 (role memory); req-09 shipped.",
        "## What changed\n\nnothing says what it delivers\n",
    ] {
        let (ok, out) = check(bad);
        assert!(!ok, "{bad:?} passed: {out}");
    }

    // F122: a Fixes: line must parse too; with none, nothing changes.
    let (ok, out) = check("Delivers: none\nFixes: F119, F117\n");
    assert!(ok && out.contains("Fixes: F119, F117"), "{out}");
    let (ok, out) = check("Delivers: none\nFixes: F119 and F117\n");
    assert!(!ok && out.contains("not an F-item"), "{out}");

    // The template's placeholder fails until someone fills it in.
    let template = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.github/pull_request_template.md"),
    )
    .unwrap();
    let (ok, out) = check(&template);
    assert!(!ok, "the template's placeholder passed: {out}");
}
