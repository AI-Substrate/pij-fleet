//! Role outputs (plan 200, req-10): one pij/output document per output under
//! `roles/<id>/outputs/<yyyy-mm>/`, written through the single writer.
//!
//! Builds the document and picks its path in-process. No git, no launch. A
//! verdict names the exact head it is for, its result, the notes it owes and
//! the evidence of what ran; a handover's body holds what is in flight now.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::directory::E_ROLE_MISSING;
use super::layout::{COMMUNITY, RolePaths};
use super::writer::{Existing, NewFile};
use super::{Refusal, role};

/// The kinds the pij/output schema allows.
pub const KINDS: [&str; 4] = ["verdict", "deliverable", "note", "handover"];
/// A verdict's results.
pub const VERDICT_RESULTS: [&str; 3] = ["pass", "pass-with-notes", "block"];
/// The longest subject part of a file name.
const SLUG_MAX: usize = 40;

/// One evidence row: what ran, how it exited and where its log is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evidence {
    /// The command.
    pub command: String,
    /// Its exit code.
    pub exit_code: i64,
    /// Where its log is, when kept.
    pub log: Option<String>,
}

/// What a caller writes into one output.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutputFields {
    /// `verdict`, `deliverable`, `note` or `handover`.
    pub kind: String,
    /// The role it is written for.
    pub role: String,
    /// What it is about, such as `pr:132`.
    pub subject: String,
    /// The exact head; required for a verdict.
    pub head: Option<String>,
    /// The result; required for a verdict (pass, pass-with-notes, block).
    pub result: Option<String>,
    /// Notes the work still owes.
    pub notes_owed: Vec<String>,
    /// What ran.
    pub evidence: Vec<Evidence>,
    /// An external link, such as the PR review.
    pub link: Option<String>,
    /// The request it answers (ruling C3).
    pub answers: Option<String>,
    /// The seat that asked, when known.
    pub requester: Option<String>,
    /// The body.
    pub body: String,
}

impl OutputFields {
    /// Whether the requester must be woken whatever reply mode it asked for:
    /// a `block` verdict or a `failed` result.
    pub fn always_wakes(&self) -> bool {
        matches!(self.result.as_deref(), Some("block" | "failed"))
    }
}

/// Check the fields before anything is written.
///
/// # Errors
/// `E-RS-ARG` naming what is wrong.
pub fn check(fields: &OutputFields) -> Result<(), Refusal> {
    let arg = |message: String| Err(Refusal::new("E-RS-ARG", message));
    if !KINDS.contains(&fields.kind.as_str()) {
        return arg(format!(
            "output kind must be one of {}, not {:?}",
            KINDS.join(", "),
            fields.kind
        ));
    }
    RolePaths::new(&fields.role)?;
    if fields.subject.trim().is_empty() || fields.subject.contains(['\n', '\r']) {
        return arg("--subject <address> is required and is one line, such as pr:132".into());
    }
    // F159: a subject every notice can show. One outside the address grammar
    // (a bare file path, say) was written, then shown as `invalid` in the FYI.
    if pij_core::render::subject(fields.subject.trim()) != fields.subject.trim() {
        return arg(format!(
            "--subject {} is not an address. {}",
            pij_core::render::subject(fields.subject.trim()),
            pij_core::outputs::SUBJECT_FORMS
        ));
    }
    if fields.kind == "verdict" {
        match (&fields.head, &fields.result) {
            (Some(head), Some(result))
                if !head.trim().is_empty() && VERDICT_RESULTS.contains(&result.as_str()) => {}
            (None, _) => {
                return arg("a verdict needs --head <sha>: the exact head it is for".into());
            }
            _ => {
                return arg(format!(
                    "a verdict needs --result {}",
                    VERDICT_RESULTS.join("|")
                ));
            }
        }
    }
    if fields.kind == "handover" && fields.body.trim().is_empty() {
        return arg("a handover needs a body: what is in flight now".into());
    }
    Ok(())
}

/// The pij/output document.
pub fn document(fields: &OutputFields, writer: &str, holds_role: bool, date: &str) -> Value {
    let mut meta = json!({
        "kind": fields.kind,
        "role": fields.role,
        "subject": fields.subject.trim(),
        "writer": writer,
        "writer_holds_role": holds_role,
        "date": date,
    });
    for (key, value) in [
        ("head", &fields.head),
        ("result", &fields.result),
        ("requester", &fields.requester),
        ("answers", &fields.answers),
        ("link", &fields.link),
    ] {
        if let Some(value) = value {
            meta[key] = json!(value);
        }
    }
    let evidence: Vec<Value> = fields
        .evidence
        .iter()
        .map(|row| {
            let mut out = json!({"command": row.command, "exit_code": row.exit_code});
            if let Some(log) = &row.log {
                out["log"] = json!(log);
            }
            out
        })
        .collect();
    json!({
        "dd": {"schema": "pij/output"},
        "sections": [
            {"name": "meta", "value": meta},
            {"name": "notes_owed", "value": fields.notes_owed},
            {"name": "evidence", "value": evidence},
            {"name": "body", "value": fields.body},
        ],
        "references": [],
    })
}

/// The file this output takes: `roles/<role>/outputs/<yyyy-mm>/<dd>-<kind>-<subject>[-<head7>][-<n>].dd.json`,
/// with the first free `-<n>` so two outputs never share a file. Called under
/// the writer lock, so the choice holds.
///
/// # Errors
/// `E-RS-ARG` for a bad role id.
pub fn path(worktree: &Path, fields: &OutputFields, date: &str) -> Result<PathBuf, Refusal> {
    RolePaths::new(&fields.role)?;
    let month = date.get(..7).unwrap_or(date);
    let day = date.get(8..10).unwrap_or("00");
    let dir = PathBuf::from(COMMUNITY)
        .join("roles")
        .join(&fields.role)
        .join("outputs")
        .join(month);
    let mut stem = format!("{day}-{}-{}", fields.kind, slug(&fields.subject));
    if let Some(head) = &fields.head {
        let short: String = head
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(7)
            .collect::<String>()
            .to_ascii_lowercase();
        if !short.is_empty() {
            stem.push('-');
            stem.push_str(&short);
        }
    }
    let taken = |name: &str| std::fs::symlink_metadata(worktree.join(dir.join(name))).is_ok();
    let mut name = format!("{stem}.dd.json");
    let mut n = 2;
    while taken(&name) {
        name = format!("{stem}-{n}.dd.json");
        n += 1;
    }
    Ok(dir.join(name))
}

/// Everything one `pij output` writes: the document, refused if its path is
/// somehow taken. The role must exist.
///
/// # Errors
/// [`E_ROLE_MISSING`] or a [`check`] refusal.
pub fn files(
    worktree: &Path,
    fields: &OutputFields,
    writer: &str,
    holds_role: bool,
    date: &str,
) -> Result<(Vec<NewFile>, PathBuf), Refusal> {
    check(fields)?;
    let paths = RolePaths::new(&fields.role)?;
    if !worktree.join(&paths.document).is_file() {
        return Err(Refusal::new(
            E_ROLE_MISSING,
            format!(
                "role {:?} does not exist in this project's directory",
                fields.role
            ),
        )
        .with_next(format!(
            "Add it first: pij directory add {} --why <text> ...",
            fields.role
        )));
    }
    let target = path(worktree, fields, date)?;
    let taken = Refusal::new(
        "E-RS-OUTPUT-EXISTS",
        format!("{} already exists", target.display()),
    );
    Ok((
        vec![NewFile {
            path: target.clone(),
            bytes: role::to_bytes(&document(fields, writer, holds_role, date)),
            document: true,
            existing: Existing::Refuse(taken),
        }],
        target,
    ))
}

fn slug(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.truncate(SLUG_MAX);
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "output".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verdict() -> OutputFields {
        OutputFields {
            kind: "verdict".into(),
            role: "reviewer".into(),
            subject: "pr:132".into(),
            head: Some("F232351abcdef".into()),
            result: Some("pass-with-notes".into()),
            notes_owed: vec!["tidy the docs".into()],
            evidence: vec![Evidence {
                command: "just rust-check".into(),
                exit_code: 0,
                log: Some("ci/run/1".into()),
            }],
            link: Some("https://example.invalid/review".into()),
            answers: Some("req-1".into()),
            requester: None,
            body: "looks right".into(),
        }
    }

    /// F159: a subject outside the address grammar (a bare file path) is
    /// refused at the writer with the valid forms, instead of being written
    /// and shown as `invalid` in every notice. `file:<path>` is the form.
    #[test]
    fn a_subject_must_be_an_address_a_notice_can_show() {
        let mut fields = verdict();
        fields.subject = "docs/how/pij-rs-api.md".into();
        let refusal = check(&fields).expect_err("a bare path");
        let text = refusal.message;
        assert!(text.contains("is not an address"), "{text}");
        assert!(text.contains("file:docs/how/pij-rs-api.md"), "{text}");
        fields.subject = "file:docs/how/pij-rs-api.md".into();
        check(&fields).expect("a file subject");
    }

    #[test]
    fn a_verdict_needs_its_exact_head_and_a_known_result() {
        assert!(check(&verdict()).is_ok());
        let mut no_head = verdict();
        no_head.head = None;
        assert!(check(&no_head).unwrap_err().message.contains("--head"));
        let mut bad = verdict();
        bad.result = Some("ok".into());
        assert!(check(&bad).unwrap_err().message.contains("--result"));
        let mut kind = verdict();
        kind.kind = "memo".into();
        assert_eq!(check(&kind).unwrap_err().code, "E-RS-ARG");
        let mut handover = verdict();
        handover.kind = "handover".into();
        handover.body = " ".into();
        assert!(check(&handover).unwrap_err().message.contains("in flight"));
    }

    #[test]
    fn the_document_carries_head_result_notes_evidence_and_the_writer_flag() {
        let document = document(&verdict(), "pij-x", false, "2026-11-06");
        let meta = &document["sections"][0]["value"];
        assert_eq!(meta["head"], "F232351abcdef");
        assert_eq!(meta["result"], "pass-with-notes");
        assert_eq!(meta["writer_holds_role"], false);
        assert_eq!(meta["answers"], "req-1");
        assert!(meta.get("requester").is_none());
        assert_eq!(document["sections"][1]["value"], json!(["tidy the docs"]));
        assert_eq!(document["sections"][2]["value"][0]["exit_code"], 0);
        assert_eq!(document["sections"][2]["value"][0]["log"], "ci/run/1");
    }

    #[test]
    fn a_block_or_a_failed_result_always_wakes() {
        let mut fields = verdict();
        assert!(!fields.always_wakes());
        fields.result = Some("block".into());
        assert!(fields.always_wakes());
        fields.kind = "deliverable".into();
        fields.result = Some("failed".into());
        assert!(fields.always_wakes());
    }

    #[test]
    fn two_outputs_never_share_a_file() {
        let top = std::env::temp_dir().join(format!(
            "pij-output-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let first = path(&top, &verdict(), "2026-11-06").unwrap();
        assert_eq!(
            first,
            PathBuf::from(
                ".harness/government/community/roles/reviewer/outputs/2026-11/06-verdict-pr-132-f232351.dd.json"
            )
        );
        std::fs::create_dir_all(top.join(first.parent().unwrap())).unwrap();
        std::fs::write(top.join(&first), "{}").unwrap();
        let second = path(&top, &verdict(), "2026-11-06").unwrap();
        assert!(
            second.ends_with("06-verdict-pr-132-f232351-2.dd.json"),
            "{second:?}"
        );
        let missing = files(&top, &verdict(), "pij-x", true, "2026-11-06").unwrap_err();
        assert_eq!(missing.code, E_ROLE_MISSING);
        std::fs::remove_dir_all(top).unwrap();
    }
}
