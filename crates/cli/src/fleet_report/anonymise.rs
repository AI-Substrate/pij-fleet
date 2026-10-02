//! `--anonymise`: names become roles and letters; paths, ids and content go.
//!
//! The output must be safe to share, so the corpus itself is rewritten before
//! any aggregate or table is computed: nothing downstream can reach a name it
//! was never given. Seats become `Orchestrator A` / `Worker C`, other senders
//! `Peer B`, sessions and transcripts opaque `s0001` / `t0001` tokens that keep
//! every join intact. Roles keep only their class; model ids only when they are a
//! known family's catalogue id; working directories, folders, message ids, opener
//! heads and limit-reset texts are dropped. Times and the UTC offset remain.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use pij_core::fleet::Corpus;

/// A role that governs others reads as an orchestrator; everything else works.
fn class(role: Option<&str>) -> &'static str {
    let role = role.unwrap_or_default().trim().to_ascii_lowercase();
    let lead = |prefix: &str| {
        role.strip_prefix(prefix)
            .is_some_and(|rest| !rest.starts_with(|c: char| c.is_ascii_alphabetic()))
    };
    if role.contains("prime") || role.contains("orchestrat") || lead("pa") || lead("pm") {
        "Orchestrator"
    } else {
        "Worker"
    }
}

/// `A`, `B`, … `Z`, `AA`, `AB`, …
fn letters(mut n: usize) -> String {
    let mut out = Vec::new();
    loop {
        out.push(b'A' + (n % 26) as u8);
        if n < 26 {
            break;
        }
        n = n / 26 - 1;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// Opaque, order-preserving tokens for a set of strings.
fn tokens(values: BTreeSet<String>, prefix: &str) -> HashMap<String, String> {
    values
        .into_iter()
        .enumerate()
        .map(|(i, value)| (value, format!("{prefix}{:04}", i + 1)))
        .collect()
}

/// Model families a catalogue id starts with; anything else is free text.
const MODEL_FAMILIES: [&str; 14] = [
    "claude-", "gpt-", "o1", "o3", "o4", "gemini-", "codex", "grok-", "kimi-", "glm-", "qwen",
    "deepseek", "mistral", "llama",
];

/// A model id kept only when it is a known family's catalogue id. A model is
/// whatever the harness recorded, which can be text the operator typed after
/// `/model` or a private endpoint name.
fn safe_model(model: &str) -> String {
    let id = model
        .rsplit('/')
        .next()
        .unwrap_or(model)
        .to_ascii_lowercase();
    if MODEL_FAMILIES.iter().any(|family| id.starts_with(family))
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | ':'))
    {
        id
    } else {
        "other".to_string()
    }
}

/// Rewrite `corpus` so it is safe to share.
pub fn anonymise_corpus(corpus: &mut Corpus) {
    let mut seats: Vec<(String, &'static str)> = corpus
        .seats
        .iter()
        .map(|seat| (seat.id.clone(), class(seat.role.as_deref())))
        .collect();
    seats.sort();
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut names: HashMap<String, String> = HashMap::new();
    for (id, class) in seats {
        let n = counts.entry(class).or_default();
        names.insert(id, format!("{class} {}", letters(*n)));
        *n += 1;
    }
    let mut others: BTreeSet<String> = corpus
        .turns
        .iter()
        .filter_map(|turn| turn.sender.clone())
        .collect();
    for m in &corpus.messages {
        others.insert(m.from.clone());
        others.insert(m.to.clone());
    }
    others.retain(|other| !names.contains_key(other));
    for (i, sender) in others.into_iter().enumerate() {
        names.insert(sender, format!("Peer {}", letters(i)));
    }
    let name = |id: &str| {
        names
            .get(id)
            .cloned()
            .unwrap_or_else(|| "Peer ?".to_string())
    };

    let mut session_ids = BTreeSet::new();
    for session in &corpus.sessions {
        session_ids.extend(session.session_id.clone());
        session_ids.extend(session.parent_session_id.clone());
    }
    for seat in &corpus.seats {
        session_ids.extend(seat.sessions.iter().cloned());
    }
    let session_of = tokens(session_ids, "s");
    let session = |id: &str| session_of.get(id).cloned().unwrap_or_default();

    let mut sources: BTreeSet<String> = corpus.sessions.iter().map(|s| s.source.clone()).collect();
    sources.extend(corpus.calls.iter().map(|c| c.source.clone()));
    sources.extend(corpus.turns.iter().map(|t| t.source.clone()));
    sources.extend(corpus.events.iter().map(|e| e.source.clone()));
    let source_of = tokens(sources, "t");
    let source = |key: &str| source_of.get(key).cloned().unwrap_or_default();

    for seat in &mut corpus.seats {
        // A role can name a project ("project prime (wow)"): keep only its class.
        seat.role = Some(class(seat.role.as_deref()).to_string());
        seat.id = name(&seat.id);
        seat.parent = seat.parent.as_deref().map(name);
        seat.folder = String::new();
        seat.sessions = seat.sessions.iter().map(|id| session(id)).collect();
    }
    for s in &mut corpus.sessions {
        s.source = source(&s.source);
        s.session_id = s.session_id.as_deref().map(session);
        s.parent_session_id = s.parent_session_id.as_deref().map(session);
        s.cwd = None;
    }
    for call in &mut corpus.calls {
        call.source = source(&call.source);
        call.model = call.model.as_deref().map(safe_model);
    }
    for turn in &mut corpus.turns {
        turn.source = source(&turn.source);
        turn.sender = turn.sender.as_deref().map(name);
        turn.pij_msg_id = None;
        turn.head = None;
    }
    for m in &mut corpus.messages {
        m.from = name(&m.from);
        m.to = name(&m.to);
    }
    corpus.primes = corpus.primes.iter().map(|id| name(id)).collect();
    // A project slug names the project.
    corpus.prime_projects.clear();
    for event in &mut corpus.events {
        event.source = source(&event.source);
        // A reset notice names the operator's timezone.
        event.resets_at = None;
        event.model = event.model.as_deref().map(safe_model);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_catalogue_model_ids_survive() {
        assert_eq!(safe_model("claude-opus-5-5"), "claude-opus-5-5");
        assert_eq!(safe_model("github-copilot/gpt-5.6-luna"), "gpt-5.6-luna");
        assert_eq!(safe_model("opys"), "other");
        assert_eq!(safe_model("claude-my private note"), "other");
    }

    /// Project slugs name projects: they are dropped, not renamed.
    #[test]
    fn prime_projects_are_dropped() {
        let mut corpus = Corpus {
            primes: vec!["pij-boss".into()],
            prime_projects: [("pij-boss".to_string(), vec!["zebra-project".to_string()])].into(),
            ..Corpus::default()
        };
        anonymise_corpus(&mut corpus);
        assert!(
            corpus.prime_projects.is_empty(),
            "{:?}",
            corpus.prime_projects
        );
    }

    #[test]
    fn letters_run_past_z() {
        assert_eq!(letters(0), "A");
        assert_eq!(letters(25), "Z");
        assert_eq!(letters(26), "AA");
        assert_eq!(letters(27), "AB");
    }

    #[test]
    fn governing_roles_read_as_orchestrators() {
        for role in [
            "o-prime",
            "project prime (wow)",
            "PA",
            "PA (assistant)",
            "pm",
        ] {
            assert_eq!(class(Some(role)), "Orchestrator", "{role}");
        }
        for role in ["stream s07", "worker", "parser", "pmax"] {
            assert_eq!(class(Some(role)), "Worker", "{role}");
        }
        assert_eq!(class(None), "Worker");
    }
}
