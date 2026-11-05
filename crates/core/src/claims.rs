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
pub const KNOWN_KINDS: [(&str, usize); 6] = [
    ("plan", 3),
    ("prd", 3),
    ("roadmap", 3),
    ("blueprint", 3),
    ("workshop", 3),
    ("migration", 4),
];

/// The width of a kind pij does not know.
pub const DEFAULT_WIDTH: usize = 3;

/// The longest kind name accepted.
pub const MAX_KIND_LEN: usize = 32;

/// The PR body line that names what a PR delivered.
pub const DELIVERS: &str = "Delivers:";

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
    match kind {
        "plan" => &["docs/plans", "docs/plans/archive"],
        "prd" => &["docs/project/prds"],
        "roadmap" => &["docs/project/roadmaps"],
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
    if name[width..].chars().next() != Some(separator) {
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

    /// The roadmap number when this is a step of `docs/project/roadmaps/NNN-*`.
    pub fn roadmap(&self) -> Option<u64> {
        let name = self.path.strip_prefix("docs/project/roadmaps/")?;
        (self.section == "steps" && !name.contains('/'))
            .then(|| number_in_name("roadmap", name))
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
    /// A token that is not a ddocs address.
    Bad(String),
}

impl fmt::Display for DeliversError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => write!(
                f,
                "the PR body has no `{DELIVERS}` line naming ddocs addresses \
                 (e.g. `{DELIVERS} docs/project/prds/001-pij-community.dd.json#requirements/req-20`)"
            ),
            Self::Bad(token) => write!(
                f,
                "`{DELIVERS}` names {token:?}, which is not a ddocs address \
                 (<path>.dd.json#<section>/<id>)"
            ),
        }
    }
}

/// The addresses every `Delivers:` line in `body` names, deduplicated in order.
/// Separators are commas and whitespace; backticks and angle brackets around a
/// token are ignored, and a bare `none` delivers nothing on purpose.
///
/// # Errors
/// [`DeliversError::Missing`] when no line names an address;
/// [`DeliversError::Bad`] for the first token that is not one.
pub fn delivers(body: &str) -> Result<Vec<Address>, DeliversError> {
    let mut addresses = Vec::new();
    for line in body.lines() {
        let line = line.trim().trim_start_matches(['-', '*', ' ']);
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
            let address = Address::parse(token).ok_or_else(|| DeliversError::Bad(token.into()))?;
            if !addresses.contains(&address) {
                addresses.push(address);
            }
        }
    }
    if addresses.is_empty() {
        return Err(DeliversError::Missing);
    }
    Ok(addresses)
}

/// `pr` appended to a `delivered_by` cell (a comma-separated PR list such as
/// `#110, #113`), or `None` when the cell already names it. Never overwrites.
pub fn delivered_by(existing: Option<&str>, pr: u64) -> Option<String> {
    let reference = format!("#{pr}");
    let mut list: Vec<&str> = existing
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .collect();
    if list.contains(&reference.as_str()) {
        return None;
    }
    list.push(&reference);
    Some(list.join(", "))
}

/// A row's string field in a ddocs document, or `None` when the section,
/// row or field is absent.
pub fn row_field(document: &Value, section: &str, id: &str, field: &str) -> Option<String> {
    document["sections"]
        .as_array()?
        .iter()
        .find(|candidate| candidate["name"] == section)?["value"]
        .as_array()?
        .iter()
        .find(|row| row["id"] == id)?[field]
        .as_str()
        .map(str::to_owned)
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
        assert_eq!(addresses[1].roadmap(), Some(1));
        assert_eq!(addresses[0].roadmap(), None);
        assert_eq!(delivers("no line here"), Err(DeliversError::Missing));
        assert_eq!(delivers("Delivers:   \n"), Err(DeliversError::Missing));
        assert_eq!(
            delivers("Delivers: req-20"),
            Err(DeliversError::Bad("req-20".into()))
        );
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
    fn delivered_by_appends_and_never_overwrites_or_repeats() {
        assert_eq!(delivered_by(None, 121).as_deref(), Some("#121"));
        assert_eq!(delivered_by(Some(""), 121).as_deref(), Some("#121"));
        assert_eq!(
            delivered_by(Some("#110"), 121).as_deref(),
            Some("#110, #121")
        );
        assert_eq!(delivered_by(Some("#110, #121"), 121), None);
        assert_eq!(
            delivered_by(Some("#110,#113"), 121).as_deref(),
            Some("#110, #113, #121")
        );
    }

    #[test]
    fn edits_read_the_row_and_quote_for_the_shell() {
        let document = json!({"sections": [
            {"name": "requirements", "value": [{"id": "req-20", "delivered_by": "#110"}]}
        ]});
        assert_eq!(
            row_field(&document, "requirements", "req-20", "delivered_by").as_deref(),
            Some("#110")
        );
        assert_eq!(row_field(&document, "requirements", "req-21", "delivered_by"), None);
        assert_eq!(row_field(&document, "steps", "s11", "plan"), None);
        let address = Address::parse("docs/x.dd.json#requirements/req-20").unwrap();
        let edit = Edit::set(&address, "delivered_by", "#110, #121".to_string());
        assert_eq!(
            edit.command,
            "ddocs set 'docs/x.dd.json#requirements/req-20/delivered_by' '#110, #121'"
        );
        assert_eq!(quote("it's"), r"'it'\''s'");
    }
}
