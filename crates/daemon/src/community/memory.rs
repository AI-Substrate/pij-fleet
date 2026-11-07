//! Role memory (plan 201, req-09): one pij/memory note per file under
//! `roles/<id>/memory/`, and the `INDEX.md` pij generates beside them.
//!
//! Reads the governance worktree in-process and builds the files the single
//! writer commits. No git, no launch. Notes are never capped, pruned or
//! deleted, and a hand-written INDEX keeps every line pij did not generate.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::directory::E_ROLE_MISSING;
use super::layout::RolePaths;
use super::writer::{Existing, NewFile};
use super::{Refusal, role};

/// The longest slug a note's file name carries; a longer one is cut back to
/// a word boundary (plan 209, F34).
pub const SLUG_MAX: usize = 40;
/// A note with this date and slug already exists.
pub const E_NOTE_EXISTS: &str = "E-RS-MEMORY-EXISTS";
/// `--note` named no note in the role's memory.
pub const E_NOTE_MISSING: &str = "E-RS-MEMORY-NOTE-MISSING";
/// The line an empty generated INDEX carries; dropped once a note lands.
const PLACEHOLDER: &str = "No notes yet. pij adds one line per note.";
/// Opens the block of INDEX lines pij writes; everything outside it is kept.
pub const FENCE_OPEN: &str = "<!-- pij:generated -->";
/// Closes it.
pub const FENCE_CLOSE: &str = "<!-- /pij:generated -->";
const INDEX: &str = "INDEX.md";
const NOTE: &str = ".dd.json";
const RENDERED: &str = ".dd.md";

/// What a caller writes into one note.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NoteFields {
    /// One line; becomes the INDEX line's link text and the file's slug.
    pub title: String,
    /// Each one line.
    pub tags: Vec<String>,
    /// One line: why a later reader opens this note.
    pub hook: String,
    /// The note itself.
    pub body: String,
    /// `--slug`: the file name's slug, instead of one derived from the title
    /// (plan 209, F34).
    pub slug: Option<String>,
}

/// The file-name slug for `title`: lowercase ASCII letters and digits, with
/// every other run of characters as one hyphen.
///
/// # Errors
/// `E-RS-ARG` when nothing usable is left.
pub fn slug(title: &str) -> Result<String, Refusal> {
    let mut out = String::new();
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    if out.len() > SLUG_MAX {
        // Cut mid-word only when the first word alone is too long.
        let whole = out.as_bytes()[SLUG_MAX] == b'-';
        out.truncate(SLUG_MAX);
        if !whole && let Some(boundary) = out.rfind('-') {
            out.truncate(boundary);
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        return Err(Refusal::new(
            "E-RS-ARG",
            format!("--title {title:?} needs at least one ASCII letter or digit"),
        ));
    }
    Ok(out)
}

/// The note's slug: `--slug` as given, checked, or one derived from the title.
///
/// # Errors
/// `E-RS-ARG` for a `--slug` that is not lowercase ASCII letters and digits
/// joined by single hyphens, or longer than [`SLUG_MAX`]; or a title with no
/// slug.
pub fn note_slug(fields: &NoteFields) -> Result<String, Refusal> {
    let Some(given) = fields.slug.as_deref() else {
        return slug(&fields.title);
    };
    let plain = !given.is_empty()
        && given.len() <= SLUG_MAX
        && !given.starts_with('-')
        && !given.ends_with('-')
        && !given.contains("--")
        && given
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-');
    if plain {
        Ok(given.to_string())
    } else {
        Err(Refusal::new(
            "E-RS-ARG",
            format!(
                "--slug {given:?} must be lowercase letters and digits joined by single hyphens, \
                 at most {SLUG_MAX} characters"
            ),
        ))
    }
}

/// The note's path, worktree-relative: `roles/<id>/memory/<date>-<slug>.dd.json`.
///
/// # Errors
/// A bad role id, a bad `--slug`, or a title with no slug.
pub fn note_path(role: &str, date: &str, fields: &NoteFields) -> Result<PathBuf, Refusal> {
    Ok(memory_dir(&RolePaths::new(role)?).join(format!("{date}-{}{NOTE}", note_slug(fields)?)))
}

/// The pij/memory document for `fields`, dated `date`.
///
/// # Errors
/// `E-RS-ARG` for an empty title, hook or body, or a title, hook or tag that
/// is not one line.
pub fn note(date: &str, fields: &NoteFields) -> Result<Value, Refusal> {
    let line = |name: &str, value: &str| {
        if value.trim().is_empty() {
            return Err(Refusal::new(
                "E-RS-ARG",
                format!("--{name} <text> is required"),
            ));
        }
        if value.contains(['\n', '\r']) {
            return Err(Refusal::new(
                "E-RS-ARG",
                format!("--{name} must be one line: it goes into INDEX.md"),
            ));
        }
        Ok(value.trim().to_string())
    };
    let title = line("title", &fields.title)?;
    let hook = line("hook", &fields.hook)?;
    let tags = fields
        .tags
        .iter()
        .map(|tag| line("tags", tag))
        .collect::<Result<Vec<_>, _>>()?;
    if fields.body.trim().is_empty() {
        return Err(Refusal::new(
            "E-RS-ARG",
            "--body <text> or --body-file <path> is required",
        ));
    }
    Ok(json!({
        "dd": {"schema": "pij/memory"},
        "sections": [
            {"name": "meta", "value": {"date": date, "title": title, "tags": tags, "hook": hook}},
            {"name": "body", "value": fields.body},
        ],
        "references": [],
    }))
}

/// Everything one `memory add` writes: the note, refused if it exists, and
/// the regenerated INDEX. Called under the writer lock, so two adds to one
/// role read each other's notes and both INDEX lines survive.
///
/// # Errors
/// [`E_ROLE_MISSING`], [`E_NOTE_EXISTS`], or a refused field.
pub fn files(
    worktree: &Path,
    role_id: &str,
    date: &str,
    fields: &NoteFields,
) -> Result<Vec<NewFile>, Refusal> {
    let paths = RolePaths::new(role_id)?;
    require_role(worktree, role_id, &paths, true)?;
    let document = note(date, fields)?;
    let path = note_path(role_id, date, fields)?;
    let exists = Refusal::new(
        E_NOTE_EXISTS,
        format!(
            "{} already exists; nothing is ever replaced",
            path.display()
        ),
    )
    .with_next("Give the note a different --title.");
    if std::fs::symlink_metadata(worktree.join(&path)).is_ok() {
        return Err(exists);
    }
    let mut notes = notes_in(&worktree.join(memory_dir(&paths)));
    notes.push((stem(&path), document.clone()));
    notes.sort_by(|a, b| a.0.cmp(&b.0));
    let existing = std::fs::read_to_string(worktree.join(&paths.memory_index)).ok();
    Ok(vec![
        NewFile {
            path,
            bytes: role::to_bytes(&document),
            document: true,
            existing: Existing::Refuse(exists),
        },
        NewFile {
            path: paths.memory_index,
            bytes: index(role_id, existing.as_deref(), &notes).into_bytes(),
            document: false,
            existing: Existing::Replace,
        },
    ])
}

/// The INDEX for `role_id`: everything in `existing` outside pij's generated
/// block, as written, then the block, rewritten whole with one line per note,
/// oldest first. The block's fence is how pij knows a line is its own; no line
/// outside it is ever judged by its shape, moved or removed (review F004). The
/// only text pij drops outside it is its own empty-index placeholder line.
pub fn index(role_id: &str, existing: Option<&str>, notes: &[(String, Value)]) -> String {
    let text = existing.map_or_else(|| format!("# {role_id} memory\n"), str::to_string);
    if notes.is_empty() {
        return match existing {
            Some(existing) => existing.to_string(),
            None => format!("{}\n{PLACEHOLDER}\n", text),
        };
    }
    let (before, after) = match (text.find(FENCE_OPEN), text.find(FENCE_CLOSE)) {
        (Some(open), Some(close)) if close > open => {
            (&text[..open], &text[close + FENCE_CLOSE.len()..])
        }
        _ => (text.as_str(), "\n"),
    };
    let before: Vec<&str> = before
        .lines()
        .filter(|line| line.trim() != PLACEHOLDER)
        .collect();
    let mut out = before.join("\n").trim_end().to_string();
    out.push_str("\n\n");
    out.push_str(FENCE_OPEN);
    out.push('\n');
    for (stem, document) in notes {
        out.push_str(&line(stem, document));
        out.push('\n');
    }
    out.push_str(FENCE_CLOSE);
    out.push_str(if after.is_empty() { "\n" } else { after });
    out
}

/// `pij directory memory <role>`: the INDEX and every note's file name.
///
/// # Errors
/// [`E_ROLE_MISSING`].
pub fn read_index(worktree: &Path, role_id: &str) -> Result<Value, Refusal> {
    let paths = RolePaths::new(role_id)?;
    require_role(worktree, role_id, &paths, false)?;
    let index = std::fs::read_to_string(worktree.join(&paths.memory_index))
        .unwrap_or_else(|_| RolePaths::empty_memory_index(role_id));
    Ok(json!({
        "role": role_id,
        "path": paths.memory_index.display().to_string(),
        "index": index,
        "notes": note_names(&worktree.join(memory_dir(&paths))),
    }))
}

/// `pij directory memory <role> --note <slug>`: one note, by file name, stem,
/// or the slug after its date.
///
/// # Errors
/// [`E_ROLE_MISSING`], [`E_NOTE_MISSING`], or `E-RS-ARG` for a name that is
/// not a plain file name or matches more than one note.
pub fn read_note(worktree: &Path, role_id: &str, wanted: &str) -> Result<Value, Refusal> {
    let paths = RolePaths::new(role_id)?;
    require_role(worktree, role_id, &paths, false)?;
    if wanted.is_empty() || wanted.contains(['/', '\\']) || wanted.starts_with('.') {
        return Err(Refusal::new(
            "E-RS-ARG",
            format!("--note {wanted:?} must be a note's file name or slug"),
        ));
    }
    let dir = memory_dir(&paths);
    let names = note_names(&worktree.join(&dir));
    let exact: Vec<&String> = names
        .iter()
        .filter(|name| *name == wanted || bare(name) == wanted)
        .collect();
    let found = if exact.is_empty() {
        names
            .iter()
            .filter(|name| dated(bare(name)) == Some(wanted))
            .collect()
    } else {
        exact
    };
    let name = match found.as_slice() {
        [one] => (*one).clone(),
        [] => {
            return Err(Refusal::new(
                E_NOTE_MISSING,
                format!("role {role_id:?} has no memory note {wanted:?}"),
            )
            .with_next(format!("List them: pij directory memory {role_id}")));
        }
        many => {
            return Err(Refusal::new(
                "E-RS-ARG",
                format!(
                    "--note {wanted:?} matches {}; give the full file name",
                    many.iter()
                        .map(|name| name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
    };
    let path = dir.join(&name);
    let text = std::fs::read_to_string(worktree.join(&path)).map_err(|error| {
        Refusal::new(
            E_NOTE_MISSING,
            format!("cannot read {}: {error}", path.display()),
        )
    })?;
    let mut data = json!({"role": role_id, "note": name, "path": path.display().to_string()});
    match serde_json::from_str::<Value>(&text)
        .ok()
        .filter(|_| name.ends_with(NOTE))
    {
        Some(document) => {
            data["meta"] = section(&document, "meta").clone();
            data["body"] = section(&document, "body").clone();
        }
        None => data["text"] = json!(text),
    }
    Ok(data)
}

fn memory_dir(paths: &RolePaths) -> PathBuf {
    paths
        .memory_index
        .parent()
        .expect("the INDEX sits in the memory folder")
        .to_path_buf()
}

/// How fresh a role's memory is (plan 203): `{notes, latest}`, the number of
/// notes and the newest dated note's `yyyy-mm-dd` (`null` when none is dated).
/// One folder read in-process; launches nothing. `None` for a bad role id.
pub fn freshness(worktree: &Path, role_id: &str) -> Option<Value> {
    let paths = RolePaths::new(role_id).ok()?;
    let names = note_names(&worktree.join(memory_dir(&paths)));
    let latest = names
        .iter()
        .filter_map(|name| {
            let stem = bare(name);
            dated(stem).map(|_| stem[..10].to_string())
        })
        .max();
    Some(json!({"notes": names.len(), "latest": latest}))
}

/// A role is there when its document is; reading also accepts a hand-made
/// folder that has memory but no document yet.
fn require_role(
    worktree: &Path,
    role_id: &str,
    paths: &RolePaths,
    writing: bool,
) -> Result<(), Refusal> {
    let document = worktree.join(&paths.document).is_file();
    if document || (!writing && worktree.join(memory_dir(paths)).is_dir()) {
        return Ok(());
    }
    Err(Refusal::new(
        E_ROLE_MISSING,
        format!("role {role_id:?} does not exist in this project's directory"),
    )
    .with_next(format!(
        "Add it first: pij directory add {role_id} --why <text> ..."
    )))
}

/// Every note in `dir`: pij's `.dd.json` and hand-written `.md`, never the
/// INDEX or a rendered `.dd.md`, sorted by name.
fn note_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| {
            name != INDEX
                && !name.ends_with(RENDERED)
                && (name.ends_with(NOTE) || name.ends_with(".md"))
        })
        .collect();
    names.sort();
    names
}

/// pij's notes in `dir`, by stem, with their documents (`null` if unreadable).
fn notes_in(dir: &Path) -> Vec<(String, Value)> {
    note_names(dir)
        .into_iter()
        .filter(|name| name.ends_with(NOTE))
        .map(|name| {
            let document = std::fs::read(dir.join(&name))
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or(Value::Null);
            (name.trim_end_matches(NOTE).to_string(), document)
        })
        .collect()
}

fn stem(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.trim_end_matches(NOTE).to_string())
        .unwrap_or_default()
}

/// A file name without its `.dd.json` or `.md`.
fn bare(name: &str) -> &str {
    name.strip_suffix(NOTE)
        .or_else(|| name.strip_suffix(".md"))
        .unwrap_or(name)
}

/// The slug after a `yyyy-mm-dd-` prefix, when there is one.
fn dated(stem: &str) -> Option<&str> {
    let (date, rest) = (stem.get(..10)?, stem.get(10..)?.strip_prefix('-')?);
    let digits = date.chars().enumerate().all(|(at, ch)| {
        if at == 4 || at == 7 {
            ch == '-'
        } else {
            ch.is_ascii_digit()
        }
    });
    digits.then_some(rest)
}

fn section<'a>(document: &'a Value, name: &str) -> &'a Value {
    document["sections"]
        .as_array()
        .and_then(|sections| sections.iter().find(|section| section["name"] == name))
        .map_or(&Value::Null, |section| &section["value"])
}

/// One generated INDEX line: `- [<date> <title>](<stem>.dd.md): <hook> (<tags>)`.
fn line(stem: &str, document: &Value) -> String {
    let meta = section(document, "meta");
    let text = |key: &str| meta[key].as_str().unwrap_or_default().trim().to_string();
    let label = [text("date"), text("title")]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let label = if label.is_empty() {
        stem.to_string()
    } else {
        label
    };
    let mut out = format!("- [{}]({stem}{RENDERED})", escape(&label));
    let hook = text("hook");
    if !hook.is_empty() {
        out.push_str(": ");
        out.push_str(&escape(&hook));
    }
    let tags: Vec<String> = meta["tags"]
        .as_array()
        .map(|tags| tags.iter().filter_map(Value::as_str).map(escape).collect())
        .unwrap_or_default();
    if !tags.is_empty() {
        out.push_str(&format!(" ({})", tags.join(", ")));
    }
    out
}

/// Markdown's punctuation escaped, so a title, hook or tag renders as text
/// and can never close or open a link.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(
            ch,
            '\\' | '[' | ']' | '(' | ')' | '*' | '_' | '`' | '<' | '>' | '!' | '#' | '|'
        ) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(title: &str) -> NoteFields {
        NoteFields {
            title: title.to_string(),
            tags: vec!["cost".to_string(), "spine".to_string()],
            hook: "why to open it".to_string(),
            body: "what I learned\n".to_string(),
            slug: None,
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pij-memory-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let role = dir.join(".harness/government/community/roles/qa");
        std::fs::create_dir_all(role.join("memory")).unwrap();
        std::fs::write(role.join("role.dd.json"), "{}").unwrap();
        dir
    }

    #[test]
    fn a_slug_is_lowercase_hyphenated_ascii_and_never_empty() {
        assert_eq!(
            slug("Plan 197: AFTER (cpu)!").unwrap(),
            "plan-197-after-cpu"
        );
        assert_eq!(slug("--x--").unwrap(), "x");
        assert_eq!(slug(&"a".repeat(100)).unwrap().len(), SLUG_MAX);
        // F34: a long title is cut back to a word boundary.
        assert_eq!(
            slug("Verdict: a schema change against the live documents").unwrap(),
            "verdict-a-schema-change-against-the-live"
        );
        assert_eq!(
            slug("Plan 202 routing table regeneration inside the writer").unwrap(),
            "plan-202-routing-table-regeneration"
        );
        for title in ["", "  ", "日本", "../.."] {
            assert_eq!(slug(title).unwrap_err().code, "E-RS-ARG", "{title:?}");
        }
        assert_eq!(
            note_path("qa", "2026-11-06", &fields("../../etc")).unwrap(),
            PathBuf::from(".harness/government/community/roles/qa/memory/2026-11-06-etc.dd.json")
        );
    }

    /// F34: `--slug` names the file instead of the title, and is checked.
    #[test]
    fn a_given_slug_names_the_file_and_must_be_plain() {
        let mut given = fields("A long title the slug replaces");
        given.slug = Some("shim-stdin".to_string());
        assert_eq!(
            note_path("qa", "2026-11-06", &given).unwrap(),
            PathBuf::from(
                ".harness/government/community/roles/qa/memory/2026-11-06-shim-stdin.dd.json"
            )
        );
        for bad in [
            "",
            "Shim",
            "-x",
            "x-",
            "a--b",
            "a/b",
            "../x",
            &"a".repeat(41),
        ] {
            given.slug = Some(bad.to_string());
            assert_eq!(
                note_path("qa", "2026-11-06", &given).unwrap_err().code,
                "E-RS-ARG",
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_note_needs_one_line_title_hook_and_tags_and_a_body() {
        let document = note("2026-11-06", &fields("T")).unwrap();
        assert_eq!(document["dd"]["schema"], "pij/memory");
        assert_eq!(section(&document, "meta")["tags"], json!(["cost", "spine"]));
        assert_eq!(section(&document, "body"), "what I learned\n");
        let mut bad = fields("T");
        bad.hook = "two\nlines".to_string();
        assert!(note("d", &bad).unwrap_err().message.contains("one line"));
        let mut bad = fields("T");
        bad.body = " ".to_string();
        assert_eq!(note("d", &bad).unwrap_err().code, "E-RS-ARG");
        assert_eq!(note("d", &fields(" ")).unwrap_err().code, "E-RS-ARG");
    }

    fn first() -> (String, Value) {
        (
            "2026-11-06-first".to_string(),
            note("2026-11-06", &fields("First")).unwrap(),
        )
    }

    #[test]
    fn regenerating_keeps_every_hand_written_line_and_adds_one_line_per_note() {
        let hand = "# pa memory\n\n\
            - [2026-11-04-a.md](2026-11-04-a.md): linked\n\n\
            - 2026-11-04-b.md: bare name\n\
            - 2026-11-04 · Dotted · tags · hook\n";
        let text = index("pa", Some(hand), &[first()]);
        assert!(text.starts_with(hand), "{text}");
        assert!(text.ends_with(&format!(
            "{FENCE_OPEN}\n- [2026-11-06 First](2026-11-06-first.dd.md): why to open it (cost, spine)\n{FENCE_CLOSE}\n"
        )));
        let two = (
            "2026-11-06-second".to_string(),
            note("2026-11-06", &fields("Second")).unwrap(),
        );
        let again = index("pa", Some(&text), &[first(), two]);
        assert_eq!(again.matches("2026-11-06-first.dd.md").count(), 1);
        assert_eq!(again.matches("2026-11-06-second.dd.md").count(), 1);
        assert!(again.starts_with(hand));
    }

    /// Review F004 (a): a hand-written bullet linking a `.dd.md` is the
    /// writer's, not pij's, and is kept.
    #[test]
    fn a_hand_written_line_shaped_like_a_generated_one_is_kept() {
        let hand =
            "# qa memory\n\n- [Read first](historical.dd.md): keep this hand-written explanation\n";
        let once = index("qa", Some(hand), &[first()]);
        let twice = index("qa", Some(&once), &[first()]);
        assert!(twice.starts_with(hand), "{twice}");
        assert_eq!(twice, once, "a second regeneration changes nothing");
    }

    /// Review F004 (b): a title with Markdown in it is escaped and its note
    /// keeps exactly one line however often the INDEX is regenerated.
    #[test]
    fn a_title_with_a_link_in_it_is_escaped_and_never_duplicated() {
        let tricky = (
            "2026-11-06-read-delivery-flow".to_string(),
            note("2026-11-06", &fields("Read [delivery](flow)")).unwrap(),
        );
        let once = index("qa", None, std::slice::from_ref(&tricky));
        assert!(
            once.contains(
                r"- [2026-11-06 Read \[delivery\]\(flow\)](2026-11-06-read-delivery-flow.dd.md)"
            ),
            "{once}"
        );
        let twice = index("qa", Some(&once), std::slice::from_ref(&tricky));
        let thrice = index("qa", Some(&twice), &[tricky]);
        assert_eq!(
            thrice
                .matches("2026-11-06-read-delivery-flow.dd.md")
                .count(),
            1
        );
        assert_eq!(thrice, once);
    }

    /// Review F004: everything outside the block, before and after it, is kept
    /// exactly across two regenerations, and edits inside it are replaced.
    #[test]
    fn a_hand_written_index_keeps_every_line_across_two_regenerations() {
        let hand = format!(
            "# reviewer memory\n\nIntro, by hand.\n\n- [x](x.dd.md): looks generated, is not\n\n{FENCE_OPEN}\n- stale line edited by hand\n{FENCE_CLOSE}\n\n## Later\nKept after the block.\n"
        );
        let once = index("reviewer", Some(&hand), &[first()]);
        let twice = index("reviewer", Some(&once), &[first()]);
        assert_eq!(twice, once);
        for kept in [
            "Intro, by hand.",
            "- [x](x.dd.md): looks generated, is not",
            "## Later",
            "Kept after the block.",
        ] {
            assert_eq!(twice.matches(kept).count(), 1, "{kept:?} in\n{twice}");
        }
        assert!(!twice.contains("stale line edited by hand"));
        assert!(twice.ends_with(&format!(
            "{FENCE_CLOSE}\n\n## Later\nKept after the block.\n"
        )));
    }

    #[test]
    fn the_empty_placeholder_goes_when_the_first_note_lands() {
        let empty = RolePaths::empty_memory_index("qa");
        let text = index("qa", Some(&empty), &[first()]);
        assert!(!text.contains(PLACEHOLDER));
        assert!(text.starts_with(&format!(
            "# qa memory\n\n{FENCE_OPEN}\n- [2026-11-06 First]"
        )));
        assert_eq!(
            index("qa", None, &[]),
            format!("# qa memory\n\n{PLACEHOLDER}\n")
        );
    }

    #[test]
    fn files_refuse_a_missing_role_and_an_existing_note() {
        let top = scratch("files");
        let built = files(&top, "qa", "2026-11-06", &fields("Seen it")).unwrap();
        assert_eq!(built.len(), 2);
        assert!(built[0].document);
        assert!(
            String::from_utf8(built[1].bytes.clone())
                .unwrap()
                .contains("(2026-11-06-seen-it.dd.md)")
        );
        std::fs::write(top.join(&built[0].path), &built[0].bytes).unwrap();
        assert_eq!(
            files(&top, "qa", "2026-11-06", &fields("Seen it"))
                .unwrap_err()
                .code,
            E_NOTE_EXISTS
        );
        assert_eq!(
            files(&top, "nobody", "2026-11-06", &fields("x"))
                .unwrap_err()
                .code,
            E_ROLE_MISSING
        );
        std::fs::remove_dir_all(top).unwrap();
    }

    #[test]
    fn read_finds_a_note_by_name_stem_or_dated_slug_and_lists_hand_notes() {
        let top = scratch("read");
        let dir = top.join(".harness/government/community/roles/qa/memory");
        let built = files(&top, "qa", "2026-11-06", &fields("Seen it")).unwrap();
        for file in &built {
            std::fs::write(top.join(&file.path), &file.bytes).unwrap();
        }
        std::fs::write(dir.join("2026-11-06-seen-it.dd.md"), "rendered").unwrap();
        std::fs::write(dir.join("2026-11-04-hand.md"), "by hand").unwrap();

        let view = read_index(&top, "qa").unwrap();
        assert_eq!(
            view["notes"],
            json!(["2026-11-04-hand.md", "2026-11-06-seen-it.dd.json"])
        );
        assert!(view["index"].as_str().unwrap().contains("Seen it"));

        for wanted in [
            "seen-it",
            "2026-11-06-seen-it",
            "2026-11-06-seen-it.dd.json",
        ] {
            let read = read_note(&top, "qa", wanted).unwrap();
            assert_eq!(read["meta"]["title"], "Seen it", "{wanted}");
            assert_eq!(read["body"], "what I learned\n");
        }
        assert_eq!(read_note(&top, "qa", "hand").unwrap()["text"], "by hand");
        assert_eq!(
            read_note(&top, "qa", "nope").unwrap_err().code,
            E_NOTE_MISSING
        );
        for bad in ["../x", ".hidden", ""] {
            assert_eq!(read_note(&top, "qa", bad).unwrap_err().code, "E-RS-ARG");
        }
        assert_eq!(read_index(&top, "ghost").unwrap_err().code, E_ROLE_MISSING);
        std::fs::remove_dir_all(top).unwrap();
    }
}
