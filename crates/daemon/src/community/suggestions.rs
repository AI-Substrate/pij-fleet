//! Seat suggestions (PRD 007 req-08, plan 244): a seat that finds an area
//! with no owner, and does not want to create the seat itself, records a
//! suggestion; the prime answers it yes, no or not yet, with a reason. They
//! live in one file, `community/suggestions.json`, written only through the
//! single writer. Recording one wakes no one.
//!
//! Pure: no store, no launch. The writer reads the current file under its
//! lock and hands it to [`apply`].

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::Refusal;
use super::layout::COMMUNITY;

/// `community/suggestions.json`, from the governance worktree's top.
pub fn path() -> PathBuf {
    Path::new(COMMUNITY).join("suggestions.json")
}

/// The answers a suggestion takes.
pub const ANSWERS: [&str; 3] = ["yes", "no", "not-yet"];

/// A suggestion id that names none.
pub const E_SUGGESTION_MISSING: &str = "E-RS-SUGGESTION-MISSING";

/// What one call changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// `suggest <area> --signal <text>`.
    Raise {
        /// The area with no owner: a path, a subject or a role it would be.
        area: String,
        /// What the seat saw.
        signal: String,
    },
    /// `suggest --answer <id> <yes|no|not-yet> --why <text>`.
    Answer {
        /// The suggestion.
        id: String,
        /// One of [`ANSWERS`].
        answer: String,
        /// The reason.
        why: String,
    },
}

fn line(flag: &str, text: &str, max: usize) -> Result<String, Refusal> {
    let text = text.trim();
    if text.is_empty() || text.chars().any(char::is_control) || text.chars().count() > max {
        return Err(Refusal::new(
            "E-RS-ARG",
            format!(
                "{flag} must be one non-empty line of at most {max} characters, with no control characters"
            ),
        ));
    }
    Ok(text.to_string())
}

/// Check a change before taking the writer's lock.
///
/// # Errors
/// `E-RS-ARG` naming the field.
pub fn check(change: &Change) -> Result<(), Refusal> {
    match change {
        Change::Raise { area, signal } => {
            line("<area>", area, 120)?;
            line("--signal", signal, 500)?;
        }
        Change::Answer { answer, why, .. } => {
            if !ANSWERS.contains(&answer.as_str()) {
                return Err(Refusal::new(
                    "E-RS-ARG",
                    format!("answer {answer:?}: give yes, no or not-yet"),
                ));
            }
            line("--why", why, 500)?;
        }
    }
    Ok(())
}

/// The suggestions in `bytes` (the file's content), oldest first; none when
/// there is no file or it is not a suggestions list.
pub fn rows(bytes: Option<&[u8]>) -> Vec<Value> {
    bytes
        .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
        .and_then(|file| file["suggestions"].as_array().cloned())
        .unwrap_or_default()
}

/// The suggestions under `worktree`, for the view: an in-process read.
pub fn read(worktree: &Path) -> Vec<Value> {
    rows(std::fs::read(worktree.join(path())).ok().as_deref())
}

/// The file after `change` by seat `by` at `at`, and the row it wrote.
///
/// # Errors
/// What [`check`] refuses; [`E_SUGGESTION_MISSING`]; `E-RS-ARG` for an answer
/// to one already answered yes or no (not-yet may be answered again).
pub fn apply(
    current: Option<&[u8]>,
    change: &Change,
    by: &str,
    at: &str,
) -> Result<(Vec<u8>, Value), Refusal> {
    check(change)?;
    let mut all = rows(current);
    let row = match change {
        Change::Raise { area, signal } => {
            let row = json!({
                "id": format!("sg-{:04}", all.len() + 1),
                "area": area.trim(),
                "signal": signal.trim(),
                "by": by,
                "at": at,
            });
            all.push(row.clone());
            row
        }
        Change::Answer { id, answer, why } => {
            let Some(row) = all.iter_mut().find(|row| row["id"] == id.as_str()) else {
                return Err(Refusal::new(
                    E_SUGGESTION_MISSING,
                    format!("there is no suggestion {id:?}"),
                )
                .with_next("List them: pij directory"));
            };
            if let Some(given) = row["answer"]["answer"].as_str()
                && given != "not-yet"
            {
                return Err(Refusal::new(
                    "E-RS-ARG",
                    format!("suggestion {id} is already answered {given}"),
                ));
            }
            row["answer"] = json!({"answer": answer, "why": why.trim(), "by": by, "at": at});
            row.clone()
        }
    };
    let mut bytes =
        serde_json::to_vec_pretty(&json!({"suggestions": all})).expect("suggestions serialise");
    bytes.push(b'\n');
    Ok((bytes, row))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raise(area: &str) -> Change {
        Change::Raise {
            area: area.into(),
            signal: "nobody reviews the docs".into(),
        }
    }

    #[test]
    fn a_suggestion_is_raised_then_answered_once_and_not_yet_again() {
        let (file, first) = apply(None, &raise("docs/"), "pij-calm-heron", "t1").unwrap();
        assert_eq!(first["id"], "sg-0001");
        let (file, second) = apply(Some(&file), &raise("ci"), "pij-calm-heron", "t2").unwrap();
        assert_eq!(second["id"], "sg-0002");
        let answer = |answer: &str| Change::Answer {
            id: "sg-0001".into(),
            answer: answer.into(),
            why: "later".into(),
        };
        let (file, later) = apply(Some(&file), &answer("not-yet"), "pij-prime", "t3").unwrap();
        assert_eq!(later["answer"]["answer"], "not-yet");
        let (file, yes) = apply(Some(&file), &answer("yes"), "pij-prime", "t4").unwrap();
        assert_eq!(yes["answer"]["by"], "pij-prime");
        let twice = apply(Some(&file), &answer("no"), "pij-prime", "t5").unwrap_err();
        assert!(twice.message.contains("already answered yes"), "{twice}");
        assert_eq!(rows(Some(&file)).len(), 2);
        let missing = Change::Answer {
            id: "sg-0009".into(),
            answer: "no".into(),
            why: "w".into(),
        };
        assert_eq!(
            apply(Some(&file), &missing, "p", "t").unwrap_err().code,
            E_SUGGESTION_MISSING
        );
        let bad = Change::Answer {
            id: "sg-0001".into(),
            answer: "maybe".into(),
            why: "w".into(),
        };
        assert_eq!(check(&bad).unwrap_err().code, "E-RS-ARG");
        assert!(check(&raise("two\nlines")).is_err());
    }
}
