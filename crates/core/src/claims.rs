//! Number claims and delivery at merge (plan 196, roadmap 001 s11).
//!
//! Pure rules only. The daemon lists the project's main checkout and runs gh;
//! this module decides what those facts mean: a kind's width and folders, the
//! numbers a file path carries, the PR body's `Delivers:` line, and the ddocs
//! edits `pij delivered` prints (ruling C23: printed, never applied).

use std::collections::BTreeSet;
use std::fmt;

use serde::Serialize;
use serde_json::Value;

/// Kinds pij knows, with their digit widths. Any other lowercase kind is
/// accepted with [`DEFAULT_WIDTH`] and has no folder to scan.
pub const KNOWN_KINDS: [(&str, usize); 7] = [
    ("plan", 3),
    ("prd", 3),
    ("workstream", 3),
    ("roadmap", 3),
    ("blueprint", 3),
    ("workshop", 3),
    ("migration", 4),
];

/// Old kind names and the kind each now names (ruling C29: roadmaps become
/// workstreams). An alias claims, releases and scans as its kind, so one
/// number is never handed out under both names.
pub const KIND_ALIASES: [(&str, &str); 1] = [("roadmap", "workstream")];

/// The kind `kind` names: itself, or what its old name now names.
pub fn canonical(kind: &str) -> &str {
    KIND_ALIASES
        .iter()
        .find(|(alias, _)| *alias == kind)
        .map_or(kind, |(_, current)| *current)
}

/// The width of a kind pij does not know.
pub const DEFAULT_WIDTH: usize = 3;

/// The longest kind name accepted.
pub const MAX_KIND_LEN: usize = 32;

/// The PR body line that names what a PR delivered.
pub const DELIVERS: &str = "Delivers:";

/// `pij claim|claims|delivered --help`: one text for the three verbs, shared
/// by the native CLI (no daemon needed) and the daemon (shim callers).
pub const HELP: &str = "\
pij claim, pij claims, pij delivered: numbers and delivery at merge (plan 196).

Usage:
  pij claim <kind> --for <subject> [--project <slug>]       take the next number, recorded with your seat
  pij claim release <kind> <number> [--why <text>] [--project <slug>]
  pij claims [--project <slug>] [--open]                    open and released claims
  pij delivered <pr> [--pm <seat>] [--project <slug>]       record what a merged PR delivered

Kinds: plan, prd, workstream, blueprint and workshop are three digits; migration
is four; any other lowercase kind is three. roadmap is workstream's old name: it
claims the same numbers, scanning docs/project/workstreams and
docs/project/roadmaps alike. The next number is one past the highest
claimed (open or released) and the highest already in the project's main
checkout. A claim is released when its seat closes, when `pij delivered` sees
its number in a merged PR's files, or by its seat or the prime.

`pij delivered` is run once by whoever merges (ruling C23). It reads the PR with
gh, takes the addresses from the body's `Delivers:` line, records them, posts on
the PM's card (the live pm-<workstream> holder, else --pm) and PRINTS the
`ddocs set` edits for the PM's next docs PR. It changes no project file and pushes
nothing. It ends the holdings the PR finished (reviewer, security and performance
for the PR, coder for each plan its files touch), only those begun before the
merge, in one directory commit, printing one line each; a re-run ends nothing
more.

The project is --project, else the one whose repository holds your directory.
";

/// Whether `kind` is a claimable kind: lowercase ASCII, starting with a
/// letter, then letters, digits or `-`.
pub fn valid_kind(kind: &str) -> bool {
    kind.len() <= MAX_KIND_LEN
        && kind.starts_with(|c: char| c.is_ascii_lowercase())
        && kind
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The digit width numbers of `kind` are written with.
pub fn width(kind: &str) -> usize {
    KNOWN_KINDS
        .iter()
        .find(|(known, _)| *known == kind)
        .map_or(DEFAULT_WIDTH, |(_, width)| *width)
}

/// `number` written at `kind`'s width: `plan 7` is `007`, `migration 33` is `0033`.
pub fn format(kind: &str, number: u64) -> String {
    format!("{number:0width$}", width = width(kind))
}

/// Folders, relative to a project's main checkout, whose entries carry
/// `kind`'s numbers. Empty for a kind pij does not know.
pub fn folders(kind: &str) -> &'static [&'static str] {
    match canonical(kind) {
        "plan" => &["docs/plans", "docs/plans/archive"],
        "prd" => &["docs/project/prds"],
        "workstream" => &["docs/project/workstreams", "docs/project/roadmaps"],
        "blueprint" => &["docs/project/blueprints"],
        "workshop" => &["docs/project/workshops"],
        "migration" => &["crates/store/migrations"],
        _ => &[],
    }
}

/// The number a folder entry's name carries for `kind`: exactly the kind's
/// width in digits, then `-` (or `_` for migrations). `None` otherwise.
pub fn number_in_name(kind: &str, name: &str) -> Option<u64> {
    let width = width(kind);
    let separator = if kind == "migration" { '_' } else { '-' };
    let digits = name.get(..width)?;
    if !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if !name[width..].starts_with(separator) {
        return None;
    }
    digits.parse().ok()
}

/// Every (kind, number) the repository-relative `paths` name, by the same
/// folder rules the checkout scan uses: the entry directly under a kind's folder.
pub fn numbers_in_paths<'a>(paths: impl IntoIterator<Item = &'a str>) -> BTreeSet<(String, u64)> {
    let mut found = BTreeSet::new();
    for path in paths {
        for (kind, _) in KNOWN_KINDS {
            if canonical(kind) != kind {
                continue;
            }
            for folder in folders(kind) {
                let Some(rest) = path
                    .strip_prefix(folder)
                    .and_then(|rest| rest.strip_prefix('/'))
                else {
                    continue;
                };
                let entry = rest.split('/').next().unwrap_or_default();
                if let Some(number) = number_in_name(kind, entry) {
                    found.insert((kind.to_string(), number));
                }
            }
        }
    }
    found
}

/// The plan folders (`docs/plans/NNN-slug`) the repository-relative `paths` touch.
pub fn plan_folders<'a>(paths: impl IntoIterator<Item = &'a str>) -> BTreeSet<String> {
    paths
        .into_iter()
        .filter_map(|path| {
            let rest = path.strip_prefix("docs/plans/")?;
            let (entry, inside) = rest.split_once('/')?;
            (number_in_name("plan", entry).is_some() && !inside.is_empty())
                .then(|| format!("docs/plans/{entry}"))
        })
        .collect()
}

/// One delivered ddocs address: `<path>#<section>/<id>`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Address {
    /// The document, relative to the repository root.
    pub path: String,
    /// The section, e.g. `requirements` or `steps`.
    pub section: String,
    /// The row id, e.g. `req-20` or `s11`.
    pub id: String,
}

impl Address {
    /// Parse `<path>.dd.json#<section>/<id>`. The path is repository-relative.
    pub fn parse(text: &str) -> Option<Self> {
        let (path, fragment) = text.split_once('#')?;
        let (section, id) = fragment.split_once('/')?;
        let plain = |part: &str| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        };
        let relative = !path.starts_with('/')
            && path
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != "..");
        (path.ends_with(".dd.json")
            && relative
            && path.chars().all(|c| !c.is_whitespace() && c != '\'')
            && plain(section)
            && plain(id))
        .then(|| Self {
            path: path.to_string(),
            section: section.to_string(),
            id: id.to_string(),
        })
    }

    /// The workstream number when this is a step of
    /// `docs/project/workstreams/NNN-*` (or its old folder, `roadmaps`).
    pub fn workstream(&self) -> Option<u64> {
        let name = folders("workstream")
            .iter()
            .find_map(|folder| self.path.strip_prefix(*folder)?.strip_prefix('/'))?;
        (self.section == "steps" && !name.contains('/'))
            .then(|| number_in_name("workstream", name))
            .flatten()
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}/{}", self.path, self.section, self.id)
    }
}

/// Why a PR body named nothing usable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeliversError {
    /// No `Delivers:` line, or one naming nothing.
    Missing,
    /// A token that is not a ddocs address, and the line it is on.
    Bad {
        /// The token.
        token: String,
        /// The whole `Delivers:` line, as written (plan 209, F46).
        line: String,
    },
}

impl fmt::Display for DeliversError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => write!(
                f,
                "the PR body has no `{DELIVERS}` line naming ddocs addresses \
                 (e.g. `{DELIVERS} docs/project/prds/001-pij-community.dd.json#requirements/req-20`, \
                 or `{DELIVERS} none`)"
            ),
            Self::Bad { token, line } => write!(
                f,
                "the line {line:?} names {token:?}, which is not a ddocs address; \
                 write full addresses separated by commas, as \
                 `{DELIVERS} docs/project/prds/001-pij-community.dd.json#requirements/req-20`, \
                 or `{DELIVERS} none`"
            ),
        }
    }
}

/// The addresses every `Delivers:` line in `body` names, deduplicated in order.
/// Separators are commas and whitespace; backticks and angle brackets around a
/// token are ignored. `Delivers: none` says on purpose that the PR delivers no
/// row, and gives an empty list.
///
/// # Errors
/// [`DeliversError::Missing`] when no line names an address or `none`;
/// [`DeliversError::Bad`] for the first token that is neither.
pub fn delivers(body: &str) -> Result<Vec<Address>, DeliversError> {
    let mut addresses = Vec::new();
    let mut none = false;
    for written in body.lines() {
        let line = written.trim().trim_start_matches(['-', '*', ' ']);
        let Some(rest) = line
            .get(..DELIVERS.len())
            .filter(|head| head.eq_ignore_ascii_case(DELIVERS))
            .map(|_| &line[DELIVERS.len()..])
        else {
            continue;
        };
        for token in rest.split(|c: char| c == ',' || c.is_whitespace()) {
            let token = token.trim_matches(|c| matches!(c, '`' | '<' | '>' | '*'));
            if token.is_empty() {
                continue;
            }
            if token.eq_ignore_ascii_case("none") {
                none = true;
                continue;
            }
            let address = Address::parse(token).ok_or_else(|| DeliversError::Bad {
                token: token.into(),
                line: written.trim().into(),
            })?;
            if !addresses.contains(&address) {
                addresses.push(address);
            }
        }
    }
    if addresses.is_empty() && !none {
        return Err(DeliversError::Missing);
    }
    Ok(addresses)
}

/// A `delivered_by` cell (a comma-separated PR list such as `#110, #113`)
/// with every PR in `prs` it does not name appended, in order; `None` when it
/// already names them all. Never drops what the cell holds: the caller passes
/// every PR recorded for the row, so edits printed for several merges before
/// the PM's docs PR each carry all of them (review of #131, F002).
pub fn delivered_by(existing: &str, prs: &[u64]) -> Option<String> {
    let mut list: Vec<String> = existing
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect();
    let before = list.len();
    for pr in prs {
        let reference = format!("#{pr}");
        if !list.contains(&reference) {
            list.push(reference);
        }
    }
    (list.len() > before).then(|| list.join(", "))
}

/// What a row's field holds, as far as an edit may rely on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cell {
    /// The row exists and the field is absent or empty.
    Empty,
    /// The row exists and the field is this text.
    Text(String),
    /// The field could not be established; an edit must not replace it
    /// (review of #131, F003). Says why.
    Unknown(String),
}

/// Row `id`'s `field` in section `section` of a ddocs document.
pub fn cell(document: &Value, section: &str, id: &str, field: &str) -> Cell {
    let Some(rows) = document["sections"].as_array().and_then(|sections| {
        sections
            .iter()
            .find(|candidate| candidate["name"] == section)
            .and_then(|found| found["value"].as_array())
    }) else {
        return Cell::Unknown(format!("has no {section} section"));
    };
    let Some(row) = rows.iter().find(|row| row["id"] == id) else {
        return Cell::Unknown(format!("has no row {id} in {section}"));
    };
    match &row[field] {
        Value::Null => Cell::Empty,
        Value::String(text) if text.trim().is_empty() => Cell::Empty,
        Value::String(text) => Cell::Text(text.clone()),
        _ => Cell::Unknown(format!("holds a {field} that is not text")),
    }
}

/// One printed ddocs edit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Edit {
    /// The field address the edit sets.
    pub address: String,
    /// The value it sets.
    pub value: String,
    /// The exact command to run from the repository root.
    pub command: String,
}

impl Edit {
    /// `ddocs set '<address>/<field>' '<value>'`.
    pub fn set(address: &Address, field: &str, value: String) -> Self {
        let address = format!("{address}/{field}");
        let command = format!("ddocs set {} {}", quote(&address), quote(&value));
        Self {
            address,
            value,
            command,
        }
    }
}

/// POSIX single-quoting.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn kinds_widths_and_names() {
        assert!(valid_kind("plan") && valid_kind("adr-2") && valid_kind("x"));
        assert!(!valid_kind("Plan") && !valid_kind("2x") && !valid_kind("") && !valid_kind("a/b"));
        assert!(!valid_kind(&"a".repeat(MAX_KIND_LEN + 1)));
        assert_eq!(format("plan", 7), "007");
        assert_eq!(format("migration", 33), "0033");
        assert_eq!(format("adr", 12), "012");
        assert_eq!(number_in_name("plan", "196-claims-delivery"), Some(196));
        assert_eq!(number_in_name("prd", "001-pij-community.dd.json"), Some(1));
        assert_eq!(number_in_name("migration", "0032_community.sql"), Some(32));
        assert_eq!(number_in_name("migration", "032_x.sql"), None);
        assert_eq!(number_in_name("plan", "1960-x"), None);
        assert_eq!(number_in_name("plan", "archive"), None);
        assert_eq!(number_in_name("plan", "19"), None);
        assert!(folders("adr").is_empty());
    }

    #[test]
    fn paths_name_numbers_and_plan_folders() {
        let paths = [
            "docs/plans/196-claims-delivery/plan.dd.json",
            "docs/plans/archive/150-old/plan.dd.json",
            "crates/store/migrations/0033_claims.sql",
            "docs/project/prds/002-pij-base.dd.json",
            "docs/project/roadmaps/001-pij-community.dd.json",
            "docs/project/workstreams/002-next.dd.json",
            "docs/plans/README.md",
            "crates/store/src/lib.rs",
        ];
        let found: Vec<(String, u64)> = numbers_in_paths(paths).into_iter().collect();
        assert_eq!(
            found,
            [
                ("migration".to_string(), 33),
                ("plan".to_string(), 150),
                ("plan".to_string(), 196),
                ("prd".to_string(), 2),
                ("workstream".to_string(), 1),
                ("workstream".to_string(), 2),
            ]
        );
        assert_eq!(
            plan_folders(paths).into_iter().collect::<Vec<_>>(),
            ["docs/plans/196-claims-delivery"]
        );
    }

    #[test]
    fn a_delivers_line_names_addresses_and_its_absence_refuses() {
        let body = "## What\n\nDelivers: `docs/project/prds/001-pij-community.dd.json#requirements/req-20`, \
                    docs/project/roadmaps/001-pij-community.dd.json#steps/s11\n\
                    - delivers: docs/project/prds/001-pij-community.dd.json#requirements/req-20\n";
        let addresses = delivers(body).unwrap();
        assert_eq!(addresses.len(), 2);
        assert_eq!(
            addresses[0].to_string(),
            "docs/project/prds/001-pij-community.dd.json#requirements/req-20"
        );
        assert_eq!(addresses[1].workstream(), Some(1));
        assert_eq!(addresses[0].workstream(), None);
        let renamed = Address::parse("docs/project/workstreams/003-x.dd.json#steps/s01").unwrap();
        assert_eq!(renamed.workstream(), Some(3));
        assert_eq!(
            (canonical("roadmap"), canonical("plan")),
            ("workstream", "plan")
        );
        assert_eq!(delivers("no line here"), Err(DeliversError::Missing));
        assert_eq!(delivers("Delivers:   \n"), Err(DeliversError::Missing));
        assert_eq!(delivers("Delivers: none"), Ok(Vec::new()));
        assert_eq!(
            delivers("Delivers: req-20"),
            Err(DeliversError::Bad {
                token: "req-20".into(),
                line: "Delivers: req-20".into()
            })
        );
        // F46: #132's prose line is named whole, with the form to write.
        let prose = "Delivers: roadmap 001 s07 (role memory); req-09 shipped.";
        let refused = delivers(prose).unwrap_err().to_string();
        assert!(refused.contains(&format!("{prose:?}")), "{refused}");
        assert!(refused.contains("or `Delivers: none`"), "{refused}");
        for bad in [
            "/abs/x.dd.json#requirements/req-1",
            "../x.dd.json#requirements/req-1",
            "x.json#requirements/req-1",
            "x.dd.json#requirements",
            "x.dd.json#requirements/req 1",
        ] {
            assert!(Address::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn delivered_by_appends_every_recorded_pr_and_never_overwrites_or_repeats() {
        assert_eq!(delivered_by("", &[121]).as_deref(), Some("#121"));
        assert_eq!(delivered_by("#110", &[121]).as_deref(), Some("#110, #121"));
        assert_eq!(delivered_by("#110, #121", &[121]), None);
        assert_eq!(
            delivered_by("#110,#113", &[121]).as_deref(),
            Some("#110, #113, #121")
        );
        assert_eq!(
            delivered_by("#110", &[153, 154]).as_deref(),
            Some("#110, #153, #154"),
            "two merges before the docs PR: both kept"
        );
        assert_eq!(
            delivered_by("#110, #153", &[153, 154]).as_deref(),
            Some("#110, #153, #154")
        );
    }

    #[test]
    fn edits_read_the_row_and_quote_for_the_shell() {
        let document = json!({"sections": [
            {"name": "requirements", "value": [
                {"id": "req-20", "delivered_by": "#110"},
                {"id": "req-21"},
                {"id": "req-22", "delivered_by": 7}
            ]}
        ]});
        assert_eq!(
            cell(&document, "requirements", "req-20", "delivered_by"),
            Cell::Text("#110".into())
        );
        assert_eq!(
            cell(&document, "requirements", "req-21", "delivered_by"),
            Cell::Empty
        );
        for (section, id) in [
            ("requirements", "req-22"),
            ("requirements", "req-99"),
            ("steps", "s11"),
        ] {
            assert!(
                matches!(
                    cell(&document, section, id, "delivered_by"),
                    Cell::Unknown(_)
                ),
                "{section}/{id} is not knowable"
            );
        }
        let address = Address::parse("docs/x.dd.json#requirements/req-20").unwrap();
        let edit = Edit::set(&address, "delivered_by", "#110, #121".to_string());
        assert_eq!(
            edit.command,
            "ddocs set 'docs/x.dd.json#requirements/req-20/delivered_by' '#110, #121'"
        );
        assert_eq!(quote("it's"), r"'it'\''s'");
    }
}
