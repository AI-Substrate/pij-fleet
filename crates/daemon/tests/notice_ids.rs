//! F141: every daemon notice id is minted through `pij_core::notice_id`, whose
//! prefixes external admission refuses. This scan fails the build on a daemon
//! id built any other way, so a new notice kind cannot ship without a reserved
//! prefix (encoded, not left to review).
//!
//! Scope: non-test source of the daemon and the sidecars (which also enqueue
//! turns). It reads the shapes ids are built in here: a reserved prefix in a
//! format string, `msg_id`/`notice_id` assigned from `format!`, and `*_id()`
//! helpers that `format!` a bare literal.

use std::path::{Path, PathBuf};

use pij_core::notice_id::NoticeKind;

fn sources() -> Vec<(PathBuf, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for crate_src in ["daemon/src", "sidecars/src"] {
        walk(&root.join(crate_src), &mut files);
    }
    files
        .into_iter()
        .filter(|path| {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            name != "tests.rs" && !name.ends_with("_tests.rs")
        })
        .map(|path| {
            let text = std::fs::read_to_string(&path).expect("read source");
            // Test modules keep their literal ids: they are fixtures, not mints.
            let live = text
                .find("#[cfg(test)]\nmod ")
                .map_or(text.as_str(), |at| &text[..at])
                .to_string();
            (path, live)
        })
        .collect()
}

fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read source dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            walk(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
}

fn line_of(text: &str, at: usize) -> usize {
    text[..at].matches('\n').count() + 1
}

/// The string literal opening at byte `at` (just after its `"`), up to its
/// closing quote.
fn literal_at(text: &str, at: usize) -> &str {
    let bytes = text.as_bytes();
    let mut end = at;
    while end < bytes.len() && bytes[end] != b'"' {
        end += if bytes[end] == b'\\' { 2 } else { 1 };
    }
    &text[at..end.min(bytes.len())]
}

/// After `at`, skip whitespace, then expect each token in turn; returns the
/// position after the last one.
fn expect_tokens(text: &str, mut at: usize, tokens: &[&str]) -> Option<usize> {
    for token in tokens {
        at += text[at..].len() - text[at..].trim_start().len();
        if !text[at..].starts_with(token) {
            return None;
        }
        at += token.len();
    }
    Some(at)
}

/// Every `format!` literal that builds a new id: assigned to `msg_id` or
/// `notice_id`, or returned from a `fn *_id(..) -> String` helper.
fn built_ids(text: &str) -> Vec<(usize, &str)> {
    let mut built = Vec::new();
    for name in ["msg_id", "notice_id"] {
        for (at, _) in text.match_indices(name) {
            let before = text[..at].chars().next_back();
            if before.is_some_and(|ch| ch.is_alphanumeric() || ch == '_') {
                continue;
            }
            let after = at + name.len();
            let open = [":", "="]
                .iter()
                .find_map(|sep| expect_tokens(text, after, &[sep, "format!(", "\""]));
            if let Some(open) = open {
                built.push((open, literal_at(text, open)));
            }
        }
    }
    for (at, _) in text.match_indices("fn ") {
        let line_end = text[at..].find('{').map_or(text.len(), |end| at + end);
        let signature = &text[at..line_end];
        let is_id_helper = signature
            .split('(')
            .next()
            .is_some_and(|name| name.ends_with("_id"))
            && signature.trim_end().ends_with("-> String");
        if is_id_helper && let Some(open) = expect_tokens(text, line_end, &["{", "format!(", "\""])
        {
            built.push((open, literal_at(text, open)));
        }
    }
    built
}

#[test]
fn every_daemon_notice_id_is_minted_through_the_reserved_table() {
    let files = sources();
    assert!(files.len() > 50, "the scan must see the daemon's source");
    let mut found = Vec::new();

    for (path, text) in &files {
        // (A) A reserved prefix spelled out in a format string: a mint that
        // bypasses `NoticeKind::id`.
        for kind in NoticeKind::ALL {
            let quoted = format!("\"{}", kind.prefix());
            for (at, _) in text.match_indices(&quoted) {
                if literal_at(text, at + 1).contains('{') {
                    found.push(format!(
                        "{}:{}: {:?} spelled out; mint it with NoticeKind::{kind:?}.id(..)",
                        path.display(),
                        line_of(text, at),
                        kind.prefix()
                    ));
                }
            }
        }
        // (B) A new kind: an id built from a bare literal. Ids derived from an
        // existing id (`"{msg_id}:held"`) start with an interpolation and pass.
        for (at, literal) in built_ids(text) {
            if !literal.starts_with('{') {
                found.push(format!(
                    "{}:{}: id built from {literal:?}; add a NoticeKind and mint it there",
                    path.display(),
                    line_of(text, at)
                ));
            }
        }
    }

    // (C) Every kind is minted somewhere, so the table is the inventory.
    for kind in NoticeKind::ALL {
        let used = format!("NoticeKind::{kind:?}");
        if !files.iter().any(|(_, text)| text.contains(&used)) {
            found.push(format!("{used} is in the table but minted nowhere"));
        }
    }

    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// The scan's own control: each shape it must catch, it catches.
#[test]
fn the_scan_catches_each_shape_of_a_hand_built_id() {
    for (source, caught) in [
        ("let msg_id = format!(\"new-kind:{}\", seat);", true),
        ("Msg { msg_id: format!(\n \"new-kind-{seat}\"), }", true),
        (
            "fn thing_id(job: &Job) -> String {\n    format!(\"thing-{}\", job.id)\n}",
            true,
        ),
        ("let msg_id = format!(\"{msg_id}:held\");", false),
        ("let msg_id = NoticeKind::PijBg.id(&rest);", false),
    ] {
        let built = built_ids(source);
        let bare = built.iter().any(|(_, literal)| !literal.starts_with('{'));
        assert_eq!(bare, caught, "{source}: {built:?}");
    }
}
