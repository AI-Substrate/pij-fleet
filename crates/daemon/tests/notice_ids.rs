//! F141: every daemon notice id is minted through `pij_core::notice_id`, whose
//! prefixes external admission refuses. This scan fails the build on a daemon
//! id built any other way, so a new notice kind cannot ship without a reserved
//! prefix (encoded, not left to review).
//!
//! Scope: non-test source of the daemon and the sidecars (which also enqueue
//! turns). It reads the shapes ids are built in here: a reserved prefix in a
//! format string; `msg_id`/`notice_id` assigned from `format!`; `*_id()`
//! helpers that `format!` a bare literal; and any argument passed where a
//! function takes `msg_id` or `dedupe_key` (`enqueue_turn`, `bg_msg`, ...).

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
            name != "tests.rs"
                && !name.ends_with("_tests.rs")
                && !path.components().any(|part| part.as_os_str() == "tests")
        })
        .map(|path| {
            let text = std::fs::read_to_string(&path).expect("read source");
            // Test items keep their literal ids: they are fixtures, not mints.
            (path, strip_test_items(&text))
        })
        .collect()
}

/// Where the token opening at `at` ends (exclusive), when it is one whose
/// contents must not be read as code: a string, raw string, char literal or
/// comment. A lifetime (`'a`) is not one.
fn skip_opaque(bytes: &[u8], at: usize) -> Option<usize> {
    let rest = &bytes[at..];
    if rest.starts_with(b"//") {
        return Some(at + rest.iter().position(|&b| b == b'\n').unwrap_or(rest.len()));
    }
    if rest.starts_with(b"/*") {
        return Some(
            at + 2
                + rest[2..]
                    .windows(2)
                    .position(|w| w == b"*/")
                    .map_or(rest.len() - 2, |end| end + 2),
        );
    }
    if rest.starts_with(b"r\"") || rest.starts_with(b"r#") {
        let hashes = rest[1..].iter().take_while(|&&b| b == b'#').count();
        if rest.get(1 + hashes) == Some(&b'"') {
            let mut close = vec![b'"'];
            close.extend(std::iter::repeat_n(b'#', hashes));
            let body = 2 + hashes;
            let end = rest[body..]
                .windows(close.len())
                .position(|w| w == close.as_slice())
                .map_or(rest.len(), |end| body + end + close.len());
            return Some(at + end);
        }
    }
    match rest.first() {
        Some(b'"') => {
            let mut end = 1;
            while end < rest.len() && rest[end] != b'"' {
                end += if rest[end] == b'\\' { 2 } else { 1 };
            }
            Some(at + (end + 1).min(rest.len()))
        }
        Some(b'\'') => {
            // A char literal: 'x' or an escape ('\n', '\u{1b}'); else a lifetime.
            if rest.get(1) == Some(&b'\\') {
                rest.iter()
                    .skip(2)
                    .position(|&b| b == b'\'')
                    .map(|end| at + end + 3)
            } else {
                let width = std::str::from_utf8(&rest[1..rest.len().min(5)])
                    .ok()
                    .or_else(|| std::str::from_utf8(&rest[1..rest.len().min(2)]).ok())
                    .and_then(|text| text.chars().next())
                    .map_or(1, char::len_utf8);
                (rest.get(1 + width) == Some(&b'\'')).then_some(at + 2 + width)
            }
        }
        _ => None,
    }
}

/// `text` with every `#[cfg(test)]` item blanked out (spaces, newlines kept,
/// so offsets and line numbers hold): an external `mod x;`, an inline module
/// or function and its body, a field, a statement. Production code after it is
/// still scanned. When unsure where an item ends it stops early, at the first
/// `;`, `,` or block at its own depth, so an error leaves test code in (a
/// false finding) and never blanks production code (a missed one).
fn strip_test_items(text: &str) -> String {
    const MARK: &str = "#[cfg(test)]";
    let mut bytes = text.as_bytes().to_vec();
    let source = text.as_bytes();
    let mut from = 0;
    while let Some(found) = text[from..].find(MARK) {
        let start = from + found;
        let mut at = start + MARK.len();
        let mut depth = 0_i32;
        let mut end = source.len();
        while at < source.len() {
            if let Some(next) = skip_opaque(source, at) {
                at = next;
                continue;
            }
            match source[at] {
                b'(' | b'[' => depth += 1,
                b')' | b']' => depth -= 1,
                b';' | b',' if depth == 0 => {
                    end = at + 1;
                    break;
                }
                b'{' if depth == 0 => {
                    end = closing(text, at).map_or(source.len(), |close| close + 1);
                    break;
                }
                b'}' if depth == 0 => {
                    end = at;
                    break;
                }
                _ => {}
            }
            at += 1;
        }
        for byte in &mut bytes[start..end] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
        from = end.max(start + MARK.len());
    }
    String::from_utf8(bytes).expect("only ASCII bytes were replaced, whole tokens at a time")
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

/// Literals that look like a mint but are not message ids, each with why.
const NOT_MESSAGE_IDS: &[(&str, &str)] = &[
    // A background job's own id (`bg-<started>-<random>`), never a msg_id.
    ("daemon/src/background.rs", "bg-{started_at}-{:032x}"),
];

fn exempt(path: &Path, literal: &str) -> bool {
    NOT_MESSAGE_IDS
        .iter()
        .any(|(file, exempt)| path.ends_with(file) && literal == *exempt)
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

/// The index of the bracket that closes the one opening at `open`, skipping
/// string literals.
fn closing(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0_i32;
    let mut at = open;
    while at < bytes.len() {
        if let Some(next) = skip_opaque(bytes, at) {
            at = next;
            continue;
        }
        match bytes[at] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(at);
                }
            }
            _ => {}
        }
        at += 1;
    }
    None
}

/// The top-level, comma-separated items between `open` and its closing bracket,
/// each with its byte offset.
fn items(text: &str, open: usize) -> Vec<(usize, &str)> {
    let Some(close) = closing(text, open) else {
        return Vec::new();
    };
    let mut parts = Vec::new();
    let mut start = open + 1;
    let mut at = start;
    while at < close {
        match text.as_bytes()[at] {
            b'(' | b'[' | b'{' | b'"' => {
                if text.as_bytes()[at] == b'"' {
                    at = at + 1 + literal_at(text, at + 1).len();
                } else {
                    at = closing(text, at).unwrap_or(close);
                }
            }
            b',' => {
                parts.push((start, text[start..at].trim()));
                start = at + 1;
            }
            _ => {}
        }
        at += 1;
    }
    if !text[start..close].trim().is_empty() {
        parts.push((start, text[start..close].trim()));
    }
    parts
}

fn is_ident(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// For each function the scan sees, the positions (after any `self`) of its
/// `msg_id` and `dedupe_key` parameters.
fn id_parameters(files: &[(PathBuf, String)]) -> Vec<(String, usize)> {
    let mut found = Vec::new();
    for (_, text) in files {
        for (at, _) in text.match_indices("fn ") {
            if text[..at].chars().next_back().is_some_and(is_ident) {
                continue;
            }
            let name: String = text[at + 3..]
                .chars()
                .take_while(|&ch| is_ident(ch))
                .collect();
            let Some(open) = text[at..].find('(').map(|offset| at + offset) else {
                continue;
            };
            if text[at + 3 + name.len()..open].contains(['{', ';']) {
                continue;
            }
            let params: Vec<String> = items(text, open)
                .into_iter()
                .map(|(_, param)| param.split(':').next().unwrap_or("").trim().to_string())
                .filter(|param| !param.ends_with("self"))
                .collect();
            for (index, param) in params.iter().enumerate() {
                if matches!(
                    param.trim_start_matches("mut ").trim(),
                    "msg_id" | "dedupe_key"
                ) {
                    found.push((name.clone(), index));
                }
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

/// Every argument passed where a function takes an id: a fresh id built from a
/// bare literal there is a new, unreserved kind.
fn id_arguments<'a>(text: &'a str, functions: &[(String, usize)]) -> Vec<(usize, &'a str)> {
    let mut arguments = Vec::new();
    for (name, index) in functions {
        let call = format!("{name}(");
        for (at, _) in text.match_indices(&call) {
            let before = text[..at].trim_end();
            if text[..at].chars().next_back().is_some_and(is_ident) || before.ends_with("fn") {
                continue;
            }
            if let Some(&(offset, argument)) = items(text, at + name.len()).get(*index) {
                arguments.push((offset, argument));
            }
        }
    }
    arguments
}

/// A fresh id built from a bare literal: `format!("kind-{}")` or `"kind"`.
fn bare_literal(argument: &str) -> Option<&str> {
    let argument = argument.trim();
    let rest = argument
        .strip_prefix("format!(")
        .map(str::trim_start)
        .unwrap_or(argument);
    let literal = rest.strip_prefix('"').map(|rest| literal_at(rest, 0))?;
    (!literal.starts_with('{')).then_some(literal)
}

#[test]
fn every_daemon_notice_id_is_minted_through_the_reserved_table() {
    let files = sources();
    assert!(files.len() > 50, "the scan must see the daemon's source");
    let mut found = Vec::new();

    for (path, text) in &files {
        // (A) A reserved prefix spelled out in a format string: a mint that
        // bypasses `NoticeKind::id`.
        for &kind in NoticeKind::ALL {
            let quoted = format!("\"{}", kind.prefix());
            for (at, _) in text.match_indices(&quoted) {
                let literal = literal_at(text, at + 1);
                if literal.contains('{') && !exempt(path, literal) {
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

    // (D) Any id argument: `enqueue_turn(.., format!("bg-{id}-cancelled"))`
    // hid a whole kind until review found it.
    let functions = id_parameters(&files);
    assert!(
        functions.iter().any(|(name, _)| name == "enqueue_turn"),
        "the scan must find the id parameters: {functions:?}"
    );
    for (path, text) in &files {
        for (at, argument) in id_arguments(text, &functions) {
            if let Some(literal) = bare_literal(argument) {
                found.push(format!(
                    "{}:{}: id argument built from {literal:?}; add a NoticeKind and mint it there",
                    path.display(),
                    line_of(text, at)
                ));
            }
        }
    }

    // (C) Every kind is minted somewhere, so the table is the inventory.
    for &kind in NoticeKind::ALL {
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
    let functions = vec![("enqueue_turn".to_string(), 4)];
    for (source, caught) in [
        (
            "enqueue_turn(&q, BG, t, body, format!(\"bg-{id}-cancelled\")).await",
            true,
        ),
        (
            "enqueue_turn(\n &q, BG, t,\n format!(\"x {y}\"),\n \"fixed-id\".to_string())",
            true,
        ),
        (
            "enqueue_turn(&q, BG, t, body, NoticeKind::SidecarBg.id(&rest))",
            false,
        ),
        (
            "enqueue_turn(&q, BG, t, body, format!(\"{msg_id}:held\"))",
            false,
        ),
    ] {
        let bare: Vec<_> = id_arguments(source, &functions)
            .into_iter()
            .filter_map(|(_, argument)| bare_literal(argument))
            .collect();
        assert_eq!(!bare.is_empty(), caught, "{source}: {bare:?}");
    }
}

/// A second origin for the table: the `notice_kinds!` invocation as written
/// declares exactly the kinds, in order, with exactly the prefixes that the
/// compiled `ALL` and `prefix()` carry. A macro that dropped a kind from `ALL`
/// fails here.
#[test]
fn the_compiled_table_is_the_declared_one() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/src/notice_id.rs");
    let text = std::fs::read_to_string(path).expect("read notice_id.rs");
    let start = text.find("\nnotice_kinds! {").expect("the invocation");
    let body = &text[start..start + text[start..].find("\n}").expect("its end")];
    let declared: Vec<(String, String)> = body
        .lines()
        .filter_map(|line| {
            let (kind, prefix) = line.trim().split_once(" => ")?;
            let prefix = prefix.strip_suffix(',')?.trim_matches('"');
            Some((kind.to_string(), prefix.to_string()))
        })
        .collect();
    let compiled: Vec<(String, String)> = NoticeKind::ALL
        .iter()
        .map(|kind| (format!("{kind:?}"), kind.prefix().to_string()))
        .collect();
    assert!(declared.len() > 20, "{declared:?}");
    assert_eq!(compiled, declared);
}

/// Herring's blocker at 52cb364: the scan cut each file at its first
/// `#[cfg(test)] mod`, and `http/mod.rs` declares external test modules
/// before production code (the sidecar guard, Telegram staging). Only test
/// items are blanked now; what follows them is still read.
#[test]
fn production_after_a_test_module_declaration_is_still_scanned() {
    let synthetic = "fn a() {}\n#[cfg(test)]\nmod tests;\n#[cfg(test)]\nmod more {\n    fn f() { let s = r#\"}{\"#; let c = '{'; }\n}\nfn live() { let msg_id = format!(\"pa-missing:{}\", 1); }\n";
    let stripped = strip_test_items(synthetic);
    assert_eq!(stripped.len(), synthetic.len(), "offsets hold");
    assert_eq!(
        stripped.lines().count(),
        synthetic.lines().count(),
        "lines hold"
    );
    assert!(
        !stripped.contains("mod tests") && !stripped.contains("mod more"),
        "{stripped}"
    );
    assert!(
        stripped.contains("fn live() { let msg_id = format!(\"pa-missing:{}\", 1); }"),
        "{stripped}"
    );
    assert_eq!(built_ids(&stripped).len(), 1, "the live mint is read");

    // The real file, as the scan reads it (through `sources()`).
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/http/mod.rs");
    let text = std::fs::read_to_string(&path).expect("read http/mod.rs");
    let first_test = text.find("#[cfg(test)]\nmod ").expect("its test modules");
    let guard = text
        .rfind("reserved_msg_id(\"pij sidecar\"")
        .expect("the sidecar guard");
    assert!(guard > first_test, "the guard follows the test modules");
    let scanned = sources()
        .into_iter()
        .find(|(file, _)| file.ends_with("daemon/src/http/mod.rs"))
        .map(|(_, scanned)| scanned)
        .expect("the scan reads http/mod.rs");
    assert_eq!(
        scanned.get(guard..guard + 30),
        text.get(guard..guard + 30),
        "the sidecar guard after the test modules is scanned"
    );
}
