//! The prime's operating picture (plan 203, roadmap 001 s09): what each role
//! is waiting on and what the prime might do next. Pure: the view gathers the
//! store rows, the live seats and the documents, and these functions shape
//! them. Nothing here reads, writes or launches.

use std::collections::HashSet;
use std::path::Path;

use pij_core::directory::shell_word;
use pij_store::{Assignment, CommunityOutput, RoleRequest};
use serde_json::{Value, json};

use super::templates::Catalog;

/// How many of a role's most recent closes may pass without a memory note
/// before the view prompts for one.
pub const MEMORY_CLOSES: usize = 2;

/// Where a project keeps its roadmaps, relative to its repo (s13's layout).
pub const ROADMAPS: &str = "docs/project/roadmaps";

/// A path or system built by at least this many roadmap steps, with no role
/// whose scope covers it, recurs enough to want an owner (C28).
pub const RECURRING_STEPS: usize = 3;

/// A role with nothing to remember and no activity for this many days is put
/// up for retirement at the boundary review (C28).
pub const IDLE_DAYS: u64 = 30;

/// What a role recommendation rests on (ruling C28). Every recommendation to
/// create, split, merge or retire a role names one: [`Recommendation`] cannot
/// be built without it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    /// For: the same system or paths keep coming up across tasks.
    Recurring,
    /// For: the same finding is derived twice.
    Rederived,
    /// For: a judgment must be independent of the author.
    Independent,
    /// For: a seat is bloating because it carries two domains.
    TwoDomains,
    /// For: a path or system needs one owner.
    OneOwner,
    /// For: a core role every PEC starts from (ruling C22).
    Core,
    /// For: a workstream step names the role in its needs.
    Needed,
    /// Against: one-off work.
    OneOff,
    /// Against: it overlaps an existing role; extend that one instead.
    Overlap,
    /// Against: nothing worth remembering.
    NothingToRemember,
    /// Against: it would mostly sit idle.
    MostlyIdle,
}

impl Signal {
    /// C28's signals for a new role, in its order.
    pub const FOR: [Self; 5] = [
        Self::Recurring,
        Self::Rederived,
        Self::Independent,
        Self::TwoDomains,
        Self::OneOwner,
    ];
    /// C28's signals against one.
    pub const AGAINST: [Self; 4] = [
        Self::OneOff,
        Self::Overlap,
        Self::NothingToRemember,
        Self::MostlyIdle,
    ];

    /// The stable code in the view's JSON.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Recurring => "recurring",
            Self::Rederived => "re-derived",
            Self::Independent => "independent-judgment",
            Self::TwoDomains => "two-domains",
            Self::OneOwner => "one-owner",
            Self::Core => "core",
            Self::Needed => "step-needs",
            Self::OneOff => "one-off",
            Self::Overlap => "overlap",
            Self::NothingToRemember => "nothing-to-remember",
            Self::MostlyIdle => "mostly-idle",
        }
    }

    /// The signal in words.
    pub const fn says(self) -> &'static str {
        match self {
            Self::Recurring => "the same system or paths keep coming up across tasks",
            Self::Rederived => "knowledge is re-derived (the same finding twice)",
            Self::Independent => "a judgment must be independent of the author",
            Self::TwoDomains => "a seat is bloating because it carries two domains",
            Self::OneOwner => "a path or system needs one owner",
            Self::Core => "a core role a PEC starts from (C22); its template is an example",
            Self::Needed => "a workstream step names this role in its needs",
            Self::OneOff => "one-off work",
            Self::Overlap => "overlap with an existing role (extend it instead)",
            Self::NothingToRemember => "nothing worth remembering",
            Self::MostlyIdle => "it would mostly sit idle",
        }
    }
}

/// What a recommendation asks the prime to do to a role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    /// Add a role.
    Create,
    /// Give part of the load its own seat or role.
    Split,
    /// Fold a role into another.
    Merge,
    /// Retire a role.
    Retire,
}

impl Move {
    /// The word in the view's JSON.
    pub const fn word(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Split => "split",
            Self::Merge => "merge",
            Self::Retire => "retire",
        }
    }
}

/// One recommendation to create, split, merge or retire a role. The signal is
/// a required field: there is no way to build one without it (C28).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recommendation {
    /// The role it is about.
    pub role: String,
    /// `core`, `recommended`, `needed`, `recurring`, `two-domains` or `idle`.
    pub kind: &'static str,
    /// What to do.
    pub act: Move,
    /// What it rests on.
    pub signal: Signal,
    /// The particulars, in words.
    pub detail: Option<String>,
    /// Roadmap steps that need the role (`<roadmap> <step>`).
    pub needed_by: Vec<String>,
    /// Runnable commands, the first the main one.
    pub commands: Vec<String>,
    /// A note, such as who the designated prime is.
    pub note: Option<String>,
}

impl Recommendation {
    /// A recommendation resting on `signal`, with its runnable `command`.
    pub fn new(
        role: impl Into<String>,
        kind: &'static str,
        act: Move,
        signal: Signal,
        command: String,
    ) -> Self {
        Self {
            role: role.into(),
            kind,
            act,
            signal,
            detail: None,
            needed_by: Vec::new(),
            commands: vec![command],
            note: None,
        }
    }

    /// The view's JSON: `{role, kind, act, signal: {code, says}, command,
    /// commands}`, with `detail`, `needed_by` and `note` when set.
    pub fn json(&self) -> Value {
        let mut out = json!({
            "role": self.role,
            "kind": self.kind,
            "act": self.act.word(),
            "signal": {"code": self.signal.code(), "says": self.signal.says()},
            "command": self.commands.first(),
            "commands": self.commands,
        });
        if let Some(detail) = &self.detail {
            out["detail"] = json!(detail);
        }
        if !self.needed_by.is_empty() {
            out["needed_by"] = json!(self.needed_by);
        }
        if let Some(note) = &self.note {
            out["note"] = json!(note);
        }
        out
    }
}

/// The standing prompt that teaches how to grow a PEC (rulings C26, C28),
/// built from the same [`Signal`]s the recommendations name.
pub fn notice() -> String {
    let list = |signals: &[Signal]| {
        signals
            .iter()
            .map(|signal| signal.says())
            .collect::<Vec<_>>()
            .join("; ")
    };
    format!(
        "Growing this community (PEC): is the work asking for something no role here covers? Recommend a role from need, and keep the fleet small but expert. Signals for a new role: {}. Signals against: {}. A role is not a seat: roles can be many and cold (memory, lineage and queue, revived on demand), while live seats stay few. At each boundary (a wave end, a compact, a PR merge), review: split an overloaded role, merge or retire an idle one. Templates are examples, not a checklist. Each recommendation here names the signal it rests on. The community is whatever the work needs now.",
        list(&Signal::FOR),
        list(&Signal::AGAINST)
    )
}

/// `role`'s queued requests: `{count, oldest_at}`, or `null` when none waits.
pub fn waiting(role: &str, queued: &[RoleRequest]) -> Value {
    let mine: Vec<&RoleRequest> = queued.iter().filter(|r| r.role == role).collect();
    match mine.iter().map(|r| r.created_at).min() {
        Some(oldest) => json!({"count": mine.len(), "oldest_at": oldest}),
        None => Value::Null,
    }
}

/// `role`'s newest output, or `null`.
pub fn latest_output(role: &str, outputs: &[CommunityOutput]) -> Value {
    outputs
        .iter()
        .find(|o| o.role == role)
        .map_or(Value::Null, |o| {
            json!({
                "kind": o.kind, "subject": o.subject, "head": o.head, "result": o.result,
                "writer": o.writer, "at": o.created_at, "path": o.path,
            })
        })
}

/// The next actions for one role (req-12): no live holder while requests
/// wait; two live holders on a one-holder role; its last [`MEMORY_CLOSES`]
/// closes left no memory note. `rows` are the role's holdings, `live` the
/// registered, untombstoned seats, `memory_latest` its newest note's date.
pub fn actions(
    project: &str,
    role: &str,
    holders: &str,
    rows: &[&Assignment],
    live: &HashSet<String>,
    waiting: usize,
    memory_latest: Option<&str>,
) -> Vec<Value> {
    let p = shell_word(project);
    let r = shell_word(role);
    let mut out = Vec::new();
    let mut holding: Vec<&Assignment> = rows
        .iter()
        .copied()
        .filter(|row| row.to.is_none() && live.contains(&row.seat))
        .collect();
    holding.sort_by(|a, b| a.from.cmp(&b.from));

    if holding.is_empty() && waiting > 0 {
        let mut commands = Vec::new();
        let last = rows
            .iter()
            .max_by(|a, b| {
                a.to.as_deref()
                    .unwrap_or(&a.from)
                    .cmp(b.to.as_deref().unwrap_or(&b.from))
            })
            .map(|row| row.seat.as_str());
        if let Some(last) = last {
            commands.push(format!("pij-rs revive {}", shell_word(last)));
        }
        commands.push(format!(
            "pij directory assign {r} <seat> --why \"<why>\" --project {p}"
        ));
        out.push(json!({
            "role": role,
            "condition": "no-live-holder",
            "says": format!(
                "{role} has no live holder and {waiting} request(s) waiting; revive its last holder or assign a seat (assigning delivers the queue)"
            ),
            "commands": commands,
        }));
    }

    if holders == "one" && holding.len() > 1 {
        let seats: Vec<&str> = holding.iter().map(|row| row.seat.as_str()).collect();
        let newest = holding.len() - 1;
        out.push(json!({
            "role": role,
            "condition": "two-live-holders",
            "says": format!(
                "{role} is a one-holder role with {} live holders ({}); release all but one (newest kept here)",
                seats.len(),
                seats.join(", ")
            ),
            "commands": holding[..newest].iter().map(|row| format!(
                "pij directory release {r} {} --why \"a one-holder role had {} live holders\" --project {p}",
                shell_word(&row.seat),
                seats.len()
            )).collect::<Vec<_>>(),
        }));
    }

    let mut closes: Vec<&str> = rows
        .iter()
        .filter(|row| row.release_why.as_deref() == Some("close"))
        .filter_map(|row| row.to.as_deref())
        .collect();
    closes.sort_unstable();
    let recent = &closes[closes.len().saturating_sub(MEMORY_CLOSES)..];
    let after_memory =
        |to: &&str| memory_latest.is_none_or(|date| to.get(..10).is_some_and(|day| day > date));
    if recent.len() == MEMORY_CLOSES && recent.iter().all(after_memory) {
        out.push(json!({
            "role": role,
            "condition": "memory-stale",
            "says": format!(
                "{role}'s last {MEMORY_CLOSES} closes left no memory note (newest note: {}); ask a holder, or write what you know",
                memory_latest.unwrap_or("none")
            ),
            "commands": [format!(
                "pij directory memory add {r} --title \"<what was learned>\" --hook \"<when to read it>\" --body-file - --project {p}"
            )],
        }));
    }
    out
}

/// Each role a not-complete roadmap step needs (`needs: ["role:<id>"]`), with
/// the steps that need it, `<roadmap> <step>`. Reads the project repo's
/// roadmap documents; an unreadable one is skipped.
pub fn roadmap_needs(repo: &Path) -> Vec<(String, Vec<String>)> {
    let mut files: Vec<_> = std::fs::read_dir(repo.join(ROADMAPS))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.to_str().is_some_and(|p| p.ends_with(".dd.json")))
        .collect();
    files.sort();
    let mut needs: Vec<(String, Vec<String>)> = Vec::new();
    for file in files {
        let Some(document) = std::fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        else {
            continue;
        };
        let name = file
            .file_name()
            .and_then(|n| n.to_str())
            .map_or("", |n| n.trim_end_matches(".dd.json"))
            .to_string();
        let steps = document["sections"]
            .as_array()
            .and_then(|sections| sections.iter().find(|s| s["name"] == "steps"))
            .and_then(|s| s["value"].as_array())
            .cloned()
            .unwrap_or_default();
        for step in steps.iter().filter(|step| step["state"] != "complete") {
            let label = format!("{name} {}", step["id"].as_str().unwrap_or("?"));
            for role in step["needs"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|need| need.as_str()?.strip_prefix("role:"))
            {
                match needs.iter_mut().find(|(id, _)| id == role) {
                    Some((_, steps)) => steps.push(label.clone()),
                    None => needs.push((role.to_string(), vec![label.clone()])),
                }
            }
        }
    }
    needs
}

/// Fold the roadmap needs into the missing-role `prompts` (req-24, req-27):
/// a needed role already prompted gains `needed_by`; any other needed role
/// that does not exist gets a recommendation of kind `needed`, resting on
/// [`Signal::Needed`], from its template when one ships, else as a new
/// definition to fill in.
pub fn fold_needs(
    project: &str,
    prompts: &mut Vec<Recommendation>,
    needs: &[(String, Vec<String>)],
    roles: &[(String, Option<String>)],
    catalog: &Catalog,
) {
    let p = shell_word(project);
    for (role, steps) in needs {
        let exists = roles
            .iter()
            .any(|(id, template)| id == role || template.as_deref() == Some(role.as_str()));
        if exists {
            continue;
        }
        if let Some(prompt) = prompts.iter_mut().find(|prompt| &prompt.role == role) {
            prompt.needed_by.clone_from(steps);
            continue;
        }
        let r = shell_word(role);
        let why = format!("\"{} needs role:{role}\"", steps.join(", "));
        let command = if catalog.latest(role).is_some() {
            format!("pij directory add {r} --from template:{r} --why {why} --project {p}")
        } else {
            format!(
                "pij directory add {r} --title \"<title>\" --kind general --lifecycle long-lived --holders one --rule \"<what it always does>\" --why {why} --project {p}"
            )
        };
        let mut prompt = Recommendation::new(role, "needed", Move::Create, Signal::Needed, command);
        prompt.needed_by.clone_from(steps);
        prompts.push(prompt);
    }
}

/// The path or system a blueprint component's `where` names: its first word,
/// cut at any glob or brace, without a trailing slash; `None` for a
/// placeholder such as `<government-root>/…`.
fn root_of(place: &str) -> Option<String> {
    let word = place.split_whitespace().next()?;
    if word.contains('<') {
        return None;
    }
    let cut = word.find(['*', '{']).map_or(word, |at| &word[..at]);
    let root = cut.trim_end_matches('/');
    (!root.is_empty()).then(|| root.to_string())
}

/// Whether a role scope path (a glob) covers `root`, or part of it.
fn covers(scope: &str, root: &str) -> bool {
    let Some(path) = root_of(scope) else {
        return false;
    };
    root == path || root.starts_with(&format!("{path}/")) || path.starts_with(&format!("{root}/"))
}

/// Paths or systems the roadmaps keep building, with the steps that build
/// them: each step's `builds` resolved to its blueprint component's `where`.
/// Reads the repo's roadmaps and the blueprints they point at, in-process.
pub fn recurring_paths(repo: &Path) -> Vec<(String, Vec<String>)> {
    let roadmaps = repo.join(ROADMAPS);
    let mut files: Vec<_> = std::fs::read_dir(&roadmaps)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.to_str().is_some_and(|p| p.ends_with(".dd.json")))
        .collect();
    files.sort();
    let read = |path: &Path| {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
    };
    let section = |document: &Value, name: &str| {
        document["sections"]
            .as_array()
            .and_then(|sections| sections.iter().find(|s| s["name"] == name))
            .and_then(|s| s["value"].as_array())
            .cloned()
            .unwrap_or_default()
    };
    let mut blueprints: std::collections::BTreeMap<std::path::PathBuf, Vec<Value>> =
        std::collections::BTreeMap::new();
    let mut paths: Vec<(String, Vec<String>)> = Vec::new();
    for file in files {
        let Some(document) = read(&file) else {
            continue;
        };
        let name = file
            .file_name()
            .and_then(|n| n.to_str())
            .map_or("", |n| n.trim_end_matches(".dd.json"))
            .to_string();
        for step in section(&document, "steps") {
            let label = format!("{name} {}", step["id"].as_str().unwrap_or("?"));
            for built in step["builds"].as_array().into_iter().flatten() {
                let Some((doc, id)) = built.as_str().and_then(|b| b.split_once("#components/"))
                else {
                    continue;
                };
                let blueprint = roadmaps.join(doc);
                let components = blueprints.entry(blueprint.clone()).or_insert_with(|| {
                    read(&blueprint)
                        .map(|d| section(&d, "components"))
                        .unwrap_or_default()
                });
                let Some(root) = components
                    .iter()
                    .find(|component| component["id"] == id)
                    .and_then(|component| component["where"].as_str())
                    .and_then(root_of)
                else {
                    continue;
                };
                match paths.iter_mut().find(|(path, _)| *path == root) {
                    Some((_, steps)) if steps.contains(&label) => {}
                    Some((_, steps)) => steps.push(label.clone()),
                    None => paths.push((root, vec![label.clone()])),
                }
            }
        }
    }
    paths
}

/// A create recommendation for each path built by at least
/// [`RECURRING_STEPS`] steps that no role's scope covers (C28: the same paths
/// keep coming up, and a path needs one owner). `scopes` are each role's
/// `(id, scope.paths)`.
pub fn unowned(
    project: &str,
    recurring: &[(String, Vec<String>)],
    scopes: &[(String, Vec<String>)],
) -> Vec<Recommendation> {
    let p = shell_word(project);
    recurring
        .iter()
        .filter(|(_, steps)| steps.len() >= RECURRING_STEPS)
        .filter(|(root, _)| {
            !scopes
                .iter()
                .any(|(_, paths)| paths.iter().any(|scope| covers(scope, root)))
        })
        .map(|(root, steps)| {
            let base: String = root
                .rsplit('/')
                .next()
                .unwrap_or(root)
                .chars()
                .map(|ch| if ch.is_ascii_alphanumeric() { ch.to_ascii_lowercase() } else { '-' })
                .collect::<String>()
                .trim_matches('-')
                .to_string();
            let taken = scopes.iter().any(|(id, _)| *id == base) || base.is_empty();
            let id = if taken { format!("{}-owner", if base.is_empty() { "path" } else { &base }) } else { base };
            let mut prompt = Recommendation::new(
                id.clone(),
                "recurring",
                Move::Create,
                Signal::Recurring,
                format!(
                    "pij directory add {} --title \"<title>\" --kind specialist --lifecycle long-lived --holders one --scope-path {} --rule \"<what it always does>\" --why \"{root} recurs across {} steps and no role owns it\" --project {p}",
                    shell_word(&id),
                    shell_word(&format!("{root}/**")),
                    steps.len()
                ),
            );
            prompt.detail = Some(format!(
                "{root} is built by {} steps ({}) and no role's scope covers it; it needs one owner",
                steps.len(),
                steps.join(", ")
            ));
            prompt
        })
        .collect()
}

/// A split recommendation for each live seat holding two or more roles at
/// once (C28: a seat carrying two domains): give each later role its own seat.
pub fn two_domains(
    project: &str,
    rows: &[Assignment],
    live: &HashSet<String>,
) -> Vec<Recommendation> {
    let p = shell_word(project);
    let mut seats: Vec<&str> = rows
        .iter()
        .filter(|row| row.to.is_none() && live.contains(&row.seat))
        .map(|row| row.seat.as_str())
        .collect();
    seats.sort_unstable();
    seats.dedup();
    let mut out = Vec::new();
    for seat in seats {
        let mut held: Vec<&Assignment> = rows
            .iter()
            .filter(|row| row.to.is_none() && row.seat == seat)
            .collect();
        held.sort_by(|a, b| a.from.cmp(&b.from));
        held.dedup_by(|a, b| a.role == b.role);
        if held.len() < 2 {
            continue;
        }
        let names: Vec<&str> = held.iter().map(|row| row.role.as_str()).collect();
        for later in &held[1..] {
            let r = shell_word(&later.role);
            let mut prompt = Recommendation::new(
                later.role.clone(),
                "two-domains",
                Move::Split,
                Signal::TwoDomains,
                format!(
                    "pij directory release {r} {} --why \"{seat} carries {} roles; {} gets its own seat\" --project {p}",
                    shell_word(seat),
                    names.len(),
                    later.role
                ),
            );
            prompt.commands.push(format!(
                "pij directory assign {r} <seat> --why \"its own seat, split from {seat}\" --project {p}"
            ));
            prompt.detail = Some(format!("{seat} holds {} at once", names.join(" and ")));
            out.push(prompt);
        }
    }
    out
}

/// What the view knows of one role for the idle check.
pub struct RoleActivity<'a> {
    /// The role id.
    pub role: &'a str,
    /// The role document's first changelog date, `yyyy-mm-dd`.
    pub created: Option<&'a str>,
    /// Its holdings.
    pub rows: &'a [&'a Assignment],
    /// Whether any holder's seat is live.
    pub live_holder: bool,
    /// Queued requests.
    pub waiting: usize,
    /// Whether it has any output.
    pub has_output: bool,
    /// Memory notes.
    pub notes: usize,
}

/// A retire recommendation (merge as the alternative) for a role with nothing
/// to remember and no activity for [`IDLE_DAYS`] (C28's signals against):
/// no memory, no outputs, no waiting request, no live holder, and no holding
/// started or ended (else the document's first date) since `today` minus
/// [`IDLE_DAYS`]. A cold role with memory is kept: a role is not a seat.
pub fn idle(project: &str, role: &RoleActivity<'_>, cutoff: &str) -> Option<Recommendation> {
    if role.notes > 0 || role.has_output || role.waiting > 0 || role.live_holder {
        return None;
    }
    let last = role
        .rows
        .iter()
        .flat_map(|row| [Some(row.from.as_str()), row.to.as_deref()])
        .flatten()
        .filter_map(|at| at.get(..10))
        .max()
        .or(role.created)?;
    if last >= cutoff {
        return None;
    }
    let p = shell_word(project);
    let r = shell_word(role.role);
    let mut prompt = Recommendation::new(
        role.role,
        "idle",
        Move::Retire,
        Signal::MostlyIdle,
        format!(
            "pij directory retire {r} --why \"idle since {last}, nothing to remember\" --project {p}"
        ),
    );
    prompt.commands.push(format!(
        "pij directory merge {r} --into <role> --why \"folded into <role>: idle since {last}\" --project {p}"
    ));
    prompt.detail = Some(format!(
        "{} has no memory, outputs, waiting requests or live holder, and nothing has happened since {last}",
        role.role
    ));
    Some(prompt)
}

/// `yyyy-mm-dd` of `now_ms` minus [`IDLE_DAYS`], UTC.
pub fn idle_cutoff(now_ms: u64) -> String {
    let seconds = i64::try_from(now_ms / 1000).unwrap_or(i64::MAX)
        - i64::try_from(IDLE_DAYS * 86_400).unwrap_or(0);
    time::OffsetDateTime::from_unix_timestamp(seconds).map_or_else(
        |_| "0000-00-00".to_string(),
        |at| {
            format!(
                "{:04}-{:02}-{:02}",
                at.year(),
                u8::from(at.month()),
                at.day()
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(seat: &str, from: &str, to: Option<&str>, why: Option<&str>) -> Assignment {
        Assignment {
            project: "fleet".into(),
            role: "reviewer".into(),
            instance: String::new(),
            seat: seat.into(),
            harness: None,
            session: None,
            machine: None,
            from: from.into(),
            to: to.map(Into::into),
            assigned_by: "prime".into(),
            released_by: None,
            why: "w".into(),
            release_why: why.map(Into::into),
            end_pending: false,
        }
    }

    fn live(seats: &[&str]) -> HashSet<String> {
        seats.iter().map(|s| (*s).to_string()).collect()
    }

    fn conditions(actions: &[Value]) -> Vec<&str> {
        actions
            .iter()
            .map(|a| a["condition"].as_str().unwrap())
            .collect()
    }

    #[test]
    fn no_live_holder_with_requests_waiting_names_revive_and_assign() {
        let old = row(
            "pij-old",
            "2026-11-01T00:00:00Z",
            Some("2026-11-02T00:00:00Z"),
            Some("release"),
        );
        let dead = row("pij-dead", "2026-11-03T00:00:00Z", None, None);
        let rows = [&old, &dead];
        let acts = actions("fleet", "reviewer", "one", &rows, &live(&[]), 2, None);
        assert_eq!(conditions(&acts), ["no-live-holder"]);
        assert_eq!(
            acts[0]["commands"],
            json!([
                "pij-rs revive pij-dead",
                "pij directory assign reviewer <seat> --why \"<why>\" --project fleet"
            ])
        );
        assert!(actions("fleet", "reviewer", "one", &rows, &live(&[]), 0, None).is_empty());
        assert!(
            actions(
                "fleet",
                "reviewer",
                "one",
                &rows,
                &live(&["pij-dead"]),
                2,
                None
            )
            .is_empty()
        );
    }

    #[test]
    fn two_live_holders_on_a_one_holder_role_release_all_but_the_newest() {
        let a = row("pij-a", "2026-11-01T00:00:00Z", None, None);
        let b = row("pij-b", "2026-11-02T00:00:00Z", None, None);
        let rows = [&b, &a];
        let acts = actions(
            "fleet",
            "reviewer",
            "one",
            &rows,
            &live(&["pij-a", "pij-b"]),
            0,
            None,
        );
        assert_eq!(conditions(&acts), ["two-live-holders"]);
        assert_eq!(
            acts[0]["commands"],
            json!([
                "pij directory release reviewer pij-a --why \"a one-holder role had 2 live holders\" --project fleet"
            ])
        );
        assert!(
            actions(
                "fleet",
                "reviewer",
                "many",
                &rows,
                &live(&["pij-a", "pij-b"]),
                0,
                None
            )
            .is_empty()
        );
        assert!(
            actions(
                "fleet",
                "reviewer",
                "one",
                &rows,
                &live(&["pij-a"]),
                0,
                None
            )
            .is_empty()
        );
    }

    #[test]
    fn memory_is_stale_only_when_the_last_two_closes_postdate_the_newest_note() {
        let c1 = row(
            "pij-1",
            "2026-11-01T00:00:00Z",
            Some("2026-11-03T09:00:00Z"),
            Some("close"),
        );
        let c2 = row(
            "pij-2",
            "2026-11-03T10:00:00Z",
            Some("2026-11-05T09:00:00Z"),
            Some("close"),
        );
        let rows = [&c1, &c2];
        let stale = |latest| {
            conditions(&actions(
                "fleet",
                "reviewer",
                "one",
                &rows,
                &live(&[]),
                0,
                latest,
            ))
            .contains(&"memory-stale")
        };
        assert!(stale(None));
        assert!(stale(Some("2026-11-02")));
        assert!(
            !stale(Some("2026-11-03")),
            "a note on a close's day covers it"
        );
        assert!(
            !actions("fleet", "reviewer", "one", &rows[..1], &live(&[]), 0, None)
                .iter()
                .any(|a| a["condition"] == "memory-stale"),
            "one close is not a pattern"
        );
        let acts = actions("fleet", "reviewer", "one", &rows, &live(&[]), 0, None);
        assert!(
            acts[0]["commands"][0]
                .as_str()
                .unwrap()
                .starts_with("pij directory memory add reviewer ")
        );
    }

    #[test]
    fn a_needed_role_is_prompted_from_its_template_or_as_a_new_definition() {
        let catalog = Catalog::shipped();
        let needs = vec![
            ("security".to_string(), vec!["001 s09".to_string()]),
            (
                "docs-system".to_string(),
                vec!["001 s09".to_string(), "001 s10".to_string()],
            ),
            ("coder".to_string(), vec!["001 s10".to_string()]),
        ];
        let mut prompts = vec![Recommendation::new(
            "security",
            "recommended",
            Move::Create,
            Signal::Independent,
            "x".into(),
        )];
        let roles = vec![("coder".to_string(), None)];
        fold_needs("fleet", &mut prompts, &needs, &roles, &catalog);
        assert_eq!(prompts[0].needed_by, ["001 s09"]);
        assert_eq!(
            prompts.len(),
            2,
            "coder exists; security was already prompted"
        );
        let docs = prompts[1].json();
        assert_eq!(docs["role"], "docs-system");
        assert_eq!(docs["kind"], "needed");
        assert_eq!(docs["signal"]["code"], "step-needs");
        let command = docs["command"].as_str().unwrap();
        let expected_template = catalog.latest("docs-system").is_some();
        assert_eq!(
            command.contains("--from template:docs-system"),
            expected_template,
            "{command}"
        );
        assert!(
            command.contains("--why \"001 s09, 001 s10 needs role:docs-system\""),
            "{command}"
        );
        assert!(command.ends_with("--project fleet"), "{command}");
    }

    #[test]
    fn roadmap_needs_skip_complete_steps() {
        let dir = pij_testkit::fresh_dir("picture-needs");
        let roadmaps = dir.join(ROADMAPS);
        std::fs::create_dir_all(&roadmaps).unwrap();
        std::fs::write(
            roadmaps.join("001-x.dd.json"),
            json!({"sections": [{"name": "steps", "value": [
                {"id": "s01", "state": "complete", "needs": ["role:old"]},
                {"id": "s02", "state": "planned", "needs": ["role:qa", "role:docs"]},
                {"id": "s03", "needs": ["role:qa"]}
            ]}]})
            .to_string(),
        )
        .unwrap();
        std::fs::write(roadmaps.join("broken.dd.json"), "{").unwrap();
        assert_eq!(
            roadmap_needs(&dir),
            vec![
                (
                    "qa".to_string(),
                    vec!["001-x s02".to_string(), "001-x s03".to_string()]
                ),
                ("docs".to_string(), vec!["001-x s02".to_string()]),
            ]
        );
    }

    #[test]
    fn a_path_built_by_three_steps_with_no_owner_gets_a_create_resting_on_recurring() {
        let dir = pij_testkit::fresh_dir("picture-recurring");
        let roadmaps = dir.join(ROADMAPS);
        let blueprints = dir.join("docs/project/blueprints");
        std::fs::create_dir_all(&roadmaps).unwrap();
        std::fs::create_dir_all(&blueprints).unwrap();
        std::fs::write(
            blueprints.join("001-x.dd.json"),
            json!({"sections": [{"name": "components", "value": [
                {"id": "c-a", "where": "crates/daemon delivery"},
                {"id": "c-b", "where": "crates/daemon"},
                {"id": "c-c", "where": "<government-root>/community/"},
                {"id": "c-d", "where": "crates/cli"}
            ]}]})
            .to_string(),
        )
        .unwrap();
        let build = |ids: &[&str]| -> Vec<String> {
            ids.iter()
                .map(|id| format!("../blueprints/001-x.dd.json#components/{id}"))
                .collect()
        };
        std::fs::write(
            roadmaps.join("001-x.dd.json"),
            json!({"sections": [{"name": "steps", "value": [
                {"id": "s01", "state": "complete", "builds": build(&["c-a", "c-c"])},
                {"id": "s02", "builds": build(&["c-a", "c-b", "c-d"])},
                {"id": "s03", "builds": build(&["c-b", "c-c"])}
            ]}]})
            .to_string(),
        )
        .unwrap();
        let recurring = recurring_paths(&dir);
        assert_eq!(
            recurring,
            vec![
                (
                    "crates/daemon".to_string(),
                    vec!["001-x s01".into(), "001-x s02".into(), "001-x s03".into()]
                ),
                ("crates/cli".to_string(), vec!["001-x s02".into()]),
            ],
            "a placeholder where is skipped; a step counts once per path"
        );
        let found = unowned(
            "fleet",
            &recurring,
            &[("coder".into(), vec!["docs/**".into()])],
        );
        assert_eq!(found.len(), 1);
        let json = found[0].json();
        assert_eq!(json["role"], "daemon");
        assert_eq!(json["act"], "create");
        assert_eq!(json["signal"]["code"], "recurring");
        assert!(
            json["detail"]
                .as_str()
                .unwrap()
                .contains("built by 3 steps"),
            "{json}"
        );
        assert!(
            json["command"]
                .as_str()
                .unwrap()
                .contains("--scope-path 'crates/daemon/**'"),
            "{json}"
        );
        let owned = [(
            "daemon-team".to_string(),
            vec!["crates/daemon/src/**".to_string()],
        )];
        assert!(
            unowned("fleet", &recurring, &owned).is_empty(),
            "a role scoped inside it owns it"
        );
    }

    #[test]
    fn a_seat_holding_two_roles_gets_a_split_resting_on_two_domains() {
        let mut pm = row("pij-a", "2026-11-01T00:00:00Z", None, None);
        pm.role = "pm".into();
        let reviewer = row("pij-a", "2026-11-02T00:00:00Z", None, None);
        let other = row("pij-b", "2026-11-02T00:00:00Z", None, None);
        let rows = vec![reviewer, pm, other];
        let split = two_domains("fleet", &rows, &live(&["pij-a", "pij-b"]));
        assert_eq!(split.len(), 1);
        let json = split[0].json();
        assert_eq!(json["role"], "reviewer", "the later role moves");
        assert_eq!(json["act"], "split");
        assert_eq!(json["signal"]["code"], "two-domains");
        assert_eq!(json["detail"], "pij-a holds pm and reviewer at once");
        assert!(
            json["commands"][0]
                .as_str()
                .unwrap()
                .starts_with("pij directory release reviewer pij-a ")
        );
        assert!(
            two_domains("fleet", &rows, &live(&["pij-b"])).is_empty(),
            "a dead seat is not bloating"
        );
    }

    #[test]
    fn only_a_role_with_nothing_to_remember_and_nothing_recent_is_put_up_for_retirement() {
        let old = row(
            "pij-x",
            "2026-09-01T00:00:00Z",
            Some("2026-09-02T00:00:00Z"),
            Some("close"),
        );
        let rows = [&old];
        let base = RoleActivity {
            role: "qa",
            created: Some("2026-08-01"),
            rows: &rows,
            live_holder: false,
            waiting: 0,
            has_output: false,
            notes: 0,
        };
        let json = idle("fleet", &base, "2026-10-07").expect("idle").json();
        assert_eq!(json["act"], "retire");
        assert_eq!(json["signal"]["code"], "mostly-idle");
        assert_eq!(
            json["commands"][0],
            "pij directory retire qa --why \"idle since 2026-09-02, nothing to remember\" --project fleet"
        );
        assert!(
            json["commands"][1]
                .as_str()
                .unwrap()
                .starts_with("pij directory merge qa --into <role> ")
        );
        let kept = RoleActivity { notes: 1, ..base };
        assert!(
            idle("fleet", &kept, "2026-10-07").is_none(),
            "a role is not a seat: a cold role with memory stays"
        );
        assert!(
            idle("fleet", &base, "2026-09-02").is_none(),
            "recent enough"
        );
        let fresh = RoleActivity {
            rows: &[],
            created: Some("2026-11-01"),
            ..base
        };
        assert!(
            idle("fleet", &fresh, "2026-10-07").is_none(),
            "a new role is not idle"
        );
        for busy in [
            RoleActivity {
                live_holder: true,
                ..base
            },
            RoleActivity { waiting: 1, ..base },
            RoleActivity {
                has_output: true,
                ..base
            },
        ] {
            assert!(idle("fleet", &busy, "2026-10-07").is_none());
        }
    }

    #[test]
    fn the_notice_teaches_c28_from_the_same_signals_the_recommendations_name() {
        let notice = notice();
        for signal in Signal::FOR.iter().chain(&Signal::AGAINST) {
            assert!(notice.contains(signal.says()), "{signal:?}: {notice}");
        }
        for words in [
            "A role is not a seat",
            "Templates are examples",
            "a wave end, a compact, a PR merge",
            "split an overloaded role, merge or retire an idle one",
            "no role here covers",
            "whatever the work needs now",
        ] {
            assert!(notice.contains(words), "{words}: {notice}");
        }
        assert_eq!(idle_cutoff(1_793_934_000_000), "2026-10-07");
    }
}
