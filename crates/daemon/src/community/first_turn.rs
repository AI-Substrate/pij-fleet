//! The assembled first turn (PRD 007 req-03 to req-05, blueprint 005
//! c-first-turn): the one message a seat starting in a role receives. Its
//! parts, in order: the seat's type (definition and seed, PRD 006), the role's
//! own text with its parameters filled, the parameters, what the creator added
//! (focus and packet), and the role's memory index. Each part carries the line
//! naming where it came from.
//!
//! [`assemble`] is pure. [`load`] reads the governance worktree in-process
//! (no launch). Cost: a handful of file reads per call; no loop, poll or timer.

use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use super::Refusal;
use super::layout::RolePaths;
use super::{memory, role, types};

/// One part of a first turn.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Part {
    /// The part's heading.
    pub heading: String,
    /// Where it came from: a governance path, a shipped path, or a seat.
    pub source: String,
    /// The text itself.
    pub text: String,
}

/// What the creator added at start (PRD 007 req-04).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Creator {
    /// The creating seat.
    pub seat: String,
    /// `--focus`: a line or two.
    pub focus: Option<String>,
    /// `--packet <file>`: the file's name as given, and its text.
    pub packet: Option<(String, String)>,
}

/// Everything [`assemble`] reads, gathered by [`load`] (or a test).
#[derive(Clone, Debug, PartialEq)]
pub struct Sources {
    /// The role id.
    pub role: String,
    /// The role document.
    pub document: Value,
    /// The role document's governance path.
    pub document_path: String,
    /// The type's seed as [`types::seed`] answers it, when the role is typed.
    pub seed: Option<Value>,
    /// The standing packet's brief, `(path, text)`, when it is a brief.
    pub brief: Option<(String, String)>,
    /// The role's memory index, `(path, text)`.
    pub memory: Option<(String, String)>,
}

/// Read role `id`'s sources from the governance worktree.
///
/// # Errors
/// [`super::directory::E_ROLE_MISSING`] for a role with no readable document;
/// a brief that is not a readable file in the worktree
/// ([`super::E_BRIEF_MISSING`]); an unreadable seed.
pub fn load(worktree: &Path, id: &str) -> Result<Sources, Refusal> {
    let paths = RolePaths::new(id)?;
    let bytes = std::fs::read(worktree.join(&paths.document)).map_err(|_| {
        Refusal::new(
            super::directory::E_ROLE_MISSING,
            format!("role {id:?} does not exist in this project's directory"),
        )
        .with_next(format!(
            "Add it first: pij directory add {id} --type <type> --why <text> ..."
        ))
    })?;
    let document: Value = serde_json::from_slice(&bytes).map_err(|error| {
        Refusal::new(
            "E-RS-ARG",
            format!("role {id:?}'s document is not JSON: {error}"),
        )
    })?;
    let seed = types::of_role(&document)
        .map(|kind| types::seed(Some(worktree), kind))
        .transpose()?;
    role::check_brief(&bytes, worktree)?;
    let brief = role_section(&document, "standing_packet")
        .filter(|packet| packet["source"] == "brief")
        .and_then(|packet| packet["brief"].as_str())
        .and_then(|path| {
            std::fs::read_to_string(worktree.join(path))
                .ok()
                .map(|text| (path.to_string(), text))
        });
    let memory = memory::read_index(worktree, id).ok().and_then(|index| {
        Some((
            index["path"].as_str()?.to_string(),
            index["index"].as_str()?.to_string(),
        ))
    });
    Ok(Sources {
        role: id.to_string(),
        document,
        document_path: paths.document.display().to_string(),
        seed,
        brief,
        memory,
    })
}

/// Role `id`'s first turn as a seat starting in it would get it: [`load`],
/// then the `--set` values checked against the role's parameters
/// ([`role::fill_parameters`]), then [`assemble`]. Writes nothing.
///
/// # Errors
/// What [`load`] and [`role::fill_parameters`] refuse; `E-RS-ARG` for an
/// instance key outside the name grammar.
pub fn build(
    worktree: &Path,
    id: &str,
    instance: &str,
    sets: &[(String, String)],
    creator: &Creator,
) -> Result<(String, Vec<Part>), Refusal> {
    if !instance.is_empty() && !pij_core::render::is_name(instance) {
        return Err(Refusal::new(
            "E-RS-ARG",
            "an instance key is 1-32 lowercase letters, digits or '-', not starting with '-'",
        ));
    }
    let sources = load(worktree, id)?;
    let filled = role::fill_parameters(id, &sources.document, sets)?;
    Ok(assemble(&sources, instance, &filled, creator))
}

fn role_section<'a>(document: &'a Value, name: &str) -> Option<&'a Value> {
    document["sections"]
        .as_array()?
        .iter()
        .find(|section| section["name"] == name)
        .map(|section| &section["value"])
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// `text` with each filled parameter's `<name>` placeholder replaced.
pub fn fill(text: &str, filled: &[(String, String)]) -> String {
    filled.iter().fold(text.to_string(), |text, (name, value)| {
        text.replace(&format!("<{name}>"), value)
    })
}

fn bullets(title: &str, items: &[String], filled: &[(String, String)]) -> String {
    if items.is_empty() {
        return String::new();
    }
    let mut out = format!("\n{title}:\n");
    for item in items {
        out.push_str(&format!("- {}\n", fill(item, filled)));
    }
    out
}

/// The first turn for a seat starting in `sources.role` (instance
/// `instance`, empty for none), with `filled` parameters and what `creator`
/// added: the whole text, and its parts.
pub fn assemble(
    sources: &Sources,
    instance: &str,
    filled: &[(String, String)],
    creator: &Creator,
) -> (String, Vec<Part>) {
    let document = &sources.document;
    let meta = role_section(document, "meta")
        .cloned()
        .unwrap_or(Value::Null);
    let mut parts = Vec::new();

    match types::of_role(document).and_then(|kind| types::get(kind).ok()) {
        Some(def) => {
            let shown = def.show();
            let mut text = format!(
                "You are a {} seat ({}).\n\n{}\n\nLifecycle: {}\nKeeps: {}\n\nRules of your type:\n",
                def.id,
                shown["title"].as_str().unwrap_or(&def.id),
                shown["purpose"].as_str().unwrap_or_default(),
                shown["lifecycle"].as_str().unwrap_or_default(),
                shown["keeps"].as_str().unwrap_or_default(),
            );
            for rule in &def.rules {
                text.push_str(&format!(
                    "- {} ({}){}\n",
                    rule.text,
                    if rule.fixed { "fixed" } else { "default" },
                    changed(document, &rule.id)
                        .map(|why| format!("; your role changes it: {why}"))
                        .unwrap_or_default()
                ));
            }
            if let Some(seed) = &sources.seed {
                text.push('\n');
                text.push_str(seed["text"].as_str().unwrap_or_default().trim_end());
                text.push('\n');
            }
            let seed_source = sources
                .seed
                .as_ref()
                .map(|seed| {
                    format!(
                        "; seed {} ({})",
                        seed["path"].as_str().unwrap_or("?"),
                        seed["source"].as_str().unwrap_or("?")
                    )
                })
                .unwrap_or_default();
            parts.push(Part {
                heading: format!("Your seat type: {}", def.id),
                source: format!("{} (shipped v{}){seed_source}", shown["path"].as_str().unwrap_or("?"), def.version),
                text,
            });
        }
        None => parts.push(Part {
            heading: "Your seat type: none recorded".to_string(),
            source: sources.document_path.clone(),
            text: format!(
                "Role {} was written before seat types and names none. The prime gives it one with `pij directory retype` or `pij directory edit {} --set type=<type>`.\n",
                sources.role, sources.role
            ),
        }),
    }

    let address = if instance.is_empty() {
        sources.role.clone()
    } else {
        format!("{}/{instance}", sources.role)
    };
    let mut text = format!(
        "You hold role {address}: {}.\n",
        meta["title"].as_str().unwrap_or(&sources.role)
    );
    if let Some(summary) = meta["summary"].as_str() {
        text.push_str(&format!("\n{}\n", fill(summary, filled)));
    }
    let scope = role_section(document, "scope");
    text.push_str(&bullets(
        "Rules",
        &strings(scope.map(|scope| &scope["rules"])),
        filled,
    ));
    text.push_str(&bullets(
        "Paths you own",
        &strings(scope.map(|scope| &scope["paths"])),
        filled,
    ));
    text.push_str(&bullets(
        "Out of scope",
        &strings(role_section(document, "out_of_scope")),
        filled,
    ));
    text.push_str(&bullets(
        "Never touch",
        &strings(role_section(document, "forbidden_paths")),
        filled,
    ));
    let mut source = sources.document_path.clone();
    match (&sources.brief, role_section(document, "standing_packet")) {
        (Some((path, brief)), _) => {
            text.push_str(&format!("\n{}\n", fill(brief, filled).trim_end()));
            source.push_str(&format!("; brief {path}"));
        }
        (None, Some(packet)) => {
            if let Some(inline) = packet["text"].as_str() {
                text.push_str(&format!("\n{}\n", fill(inline, filled).trim_end()));
            }
        }
        (None, None) => {}
    }
    parts.push(Part {
        heading: format!("Your role: {address}"),
        source,
        text,
    });

    if !filled.is_empty() {
        parts.push(Part {
            heading: "Parameters".to_string(),
            source: format!("set at start by {}", creator.seat),
            text: filled
                .iter()
                .map(|(name, value)| format!("- {name} = {value}\n"))
                .collect(),
        });
    }
    if let Some(focus) = &creator.focus {
        parts.push(Part {
            heading: format!("Focus from {}", creator.seat),
            source: format!("--focus by {}", creator.seat),
            text: format!("{}\n", focus.trim_end()),
        });
    }
    if let Some((file, packet)) = &creator.packet {
        parts.push(Part {
            heading: format!("Packet from {}", creator.seat),
            source: format!("--packet {file} by {}", creator.seat),
            text: format!("{}\n", packet.trim_end()),
        });
    }
    if let Some((path, index)) = &sources.memory {
        parts.push(Part {
            heading: format!("What role {} remembers", sources.role),
            source: path.clone(),
            text: format!(
                "{}\n\nRead a note with `pij directory memory {} --note <slug>`.\n",
                index.trim_end(),
                sources.role
            ),
        });
    }

    let mut whole = format!("# First turn: role {address}\n");
    for part in &parts {
        whole.push_str(&format!(
            "\n## {}\nsource: {}\n\n{}",
            part.heading, part.source, part.text
        ));
    }
    (whole, parts)
}

fn changed<'a>(document: &'a Value, rule: &str) -> Option<&'a str> {
    role_section(document, "type_changes")?
        .as_array()?
        .iter()
        .find(|row| row["rule"] == rule)?["why"]
        .as_str()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn sources(document: Value) -> Sources {
        Sources {
            role: "notes".into(),
            seed: types::of_role(&document).map(|kind| types::seed(None, kind).unwrap()),
            document,
            document_path: ".harness/government/community/roles/notes/role.dd.json".into(),
            brief: None,
            memory: Some((
                ".harness/government/community/roles/notes/memory/INDEX.md".into(),
                "# notes memory\n- [first call](2026-11-01-first.dd.md)\n".into(),
            )),
        }
    }

    fn document() -> Value {
        json!({"sections": [
            {"name": "meta", "value": {"id": "notes", "title": "Note taker", "type": "reviewer",
                "summary": "Takes notes for <subject>."}},
            {"name": "scope", "value": {"paths": [], "rules": ["Write one note per <subject> call."]}},
            {"name": "type_changes", "value": [{"rule": "compact-after-job", "why": "keeps the call's context"}]},
            {"name": "standing_packet", "value": {"source": "inline", "text": "Publish to <session>."}},
        ]})
    }

    #[test]
    fn the_first_turn_is_type_role_parameters_creator_then_memory_each_with_its_source() {
        let filled = vec![
            ("subject".to_string(), "acme".to_string()),
            ("session".to_string(), "/tmp/s1".to_string()),
        ];
        let creator = Creator {
            seat: "pij-brisk-marten".into(),
            focus: Some("Start with the agenda.".into()),
            packet: Some(("packet.md".into(), "Bring last week's actions.".into())),
        };
        let (text, parts) = assemble(&sources(document()), "", &filled, &creator);
        let headings: Vec<&str> = parts.iter().map(|part| part.heading.as_str()).collect();
        assert_eq!(
            headings,
            [
                "Your seat type: reviewer",
                "Your role: notes",
                "Parameters",
                "Focus from pij-brisk-marten",
                "Packet from pij-brisk-marten",
                "What role notes remembers",
            ]
        );
        assert!(
            parts[0]
                .source
                .contains("templates/types/reviewer/v1/type.dd.json")
        );
        assert!(
            parts[0]
                .source
                .contains("seed templates/types/reviewer/v1/seed.md (shipped)")
        );
        assert!(
            parts[0]
                .text
                .contains("your role changes it: keeps the call's context")
        );
        assert!(
            parts[1].text.contains("Takes notes for acme."),
            "{}",
            parts[1].text
        );
        assert!(parts[1].text.contains("Write one note per acme call."));
        assert!(parts[1].text.contains("Publish to /tmp/s1."));
        assert_eq!(parts[3].source, "--focus by pij-brisk-marten");
        assert!(text.starts_with("# First turn: role notes\n"));
        assert_eq!(text.matches("\nsource: ").count(), parts.len());
    }

    #[test]
    fn a_role_from_before_types_still_assembles_and_says_how_to_type_it() {
        let mut untyped = document();
        untyped["sections"][0]["value"]
            .as_object_mut()
            .unwrap()
            .remove("type");
        let (_, parts) = assemble(&sources(untyped), "42", &[], &Creator::default());
        assert_eq!(parts[0].heading, "Your seat type: none recorded");
        assert!(parts[0].text.contains("pij directory retype"));
        assert_eq!(parts[1].heading, "Your role: notes/42");
        assert!(
            parts[1].text.contains("<subject>"),
            "an unfilled placeholder stays"
        );
    }
}
