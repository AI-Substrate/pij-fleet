//! Every community path, in one place (blueprint c-layout).
//!
//! Paths here are relative to the governance worktree's top level, which is
//! what git and ddocs run in. `<government-root>` is [`GOVERNMENT`] below it.

use std::path::PathBuf;

use super::{E_DIRECTORY_EXISTS, Refusal};

/// `<government-root>`, relative to the governance worktree.
pub const GOVERNMENT: &str = ".harness/government";
/// The community directory, relative to the governance worktree.
pub const COMMUNITY: &str = ".harness/government/community";
/// The longest role id.
pub const ROLE_ID_MAX: usize = 48;

/// The `pij/*` schema copies the governance branch carries (ruling C9), as
/// `(worktree-relative path, pij's own bytes)`. ddocs looks for schemas at
/// `<gitroot>/.dd`, so they live at the worktree root, outside `community/`.
pub fn schema_copies() -> Vec<(PathBuf, &'static [u8])> {
    SCHEMAS
        .iter()
        .map(|(kind, bytes)| {
            (
                PathBuf::from(format!(".dd/schemas/pij/{kind}/schema.json")),
                *bytes,
            )
        })
        .collect()
}

/// pij's own schemas, compiled in so the copies always match this build.
const SCHEMAS: [(&str, &[u8]); 5] = [
    (
        "role",
        include_bytes!("../../../../.dd/schemas/pij/role/schema.json"),
    ),
    (
        "output",
        include_bytes!("../../../../.dd/schemas/pij/output/schema.json"),
    ),
    (
        "memory",
        include_bytes!("../../../../.dd/schemas/pij/memory/schema.json"),
    ),
    (
        "holders",
        include_bytes!("../../../../.dd/schemas/pij/holders/schema.json"),
    ),
    (
        "external",
        include_bytes!("../../../../.dd/schemas/pij/external/schema.json"),
    ),
];

/// `community/external.dd.json`: seats borrowed from other projects (req-16).
pub const EXTERNAL: &str = ".harness/government/community/external.dd.json";

/// `community/ROUTES.md`: the routing table pij generates (plan 202).
pub const ROUTES: &str = ".harness/government/community/ROUTES.md";

/// One role's files under `community/roles/<id>/`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RolePaths {
    /// `roles/<id>/role.dd.json`.
    pub document: PathBuf,
    /// `roles/<id>/holders.dd.json`: who has held the role (plan 195).
    pub holders: PathBuf,
    /// `roles/<id>/memory/INDEX.md`: memory notes go beside it, one per file.
    pub memory_index: PathBuf,
    /// `roles/<id>/outputs/.gitkeep`: outputs go under `outputs/<yyyy-mm>/`.
    pub outputs_keep: PathBuf,
}

/// Longest instance key a new holding may take (F135), and how much of a
/// refused one a refusal shows (escaped).
pub const INSTANCE_MAX: usize = 32;

/// Refuse an instance key a new holding may not take (F135): the one name
/// grammar, [`pij_core::render::is_name`] (`[a-z0-9][a-z0-9-]{0,31}`), which
/// every message that names a holding renders by. So a recorded key always
/// shows as itself, and no prompt that names it carries a newline, quote or
/// control character. The refusal shows the key escaped and bounded, never
/// raw. Release and the rebuild still read stored keys as they are, so a row
/// from before this check can still be ended.
///
/// # Errors
/// `E-RS-ARG` naming the grammar.
pub fn check_instance(instance: &str) -> Result<(), Refusal> {
    if pij_core::render::is_name(instance) {
        return Ok(());
    }
    Err(Refusal::new(
        "E-RS-ARG",
        format!(
            "instance key \"{}\" must be 1-{INSTANCE_MAX} lowercase letters, digits or '-', not starting with '-'",
            instance
                .escape_debug()
                .take(INSTANCE_MAX)
                .collect::<String>()
        ),
    ))
}

impl RolePaths {
    /// The paths for role `id`, after checking the id is safe to put in a path.
    ///
    /// # Errors
    /// `E-RS-ARG` for an id that is not a safe path segment.
    pub fn new(id: &str) -> Result<Self, Refusal> {
        let well_formed = !id.is_empty()
            && id.len() <= ROLE_ID_MAX
            && id.starts_with(|ch: char| ch.is_ascii_lowercase())
            && !id.ends_with('-')
            && id
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-');
        if !well_formed {
            return Err(Refusal::new(
                "E-RS-ARG",
                format!(
                    "role id {id:?} must be 1-{ROLE_ID_MAX} lowercase letters, digits or hyphens, starting with a letter"
                ),
            ));
        }
        let dir = PathBuf::from(COMMUNITY).join("roles").join(id);
        Ok(Self {
            document: dir.join("role.dd.json"),
            holders: dir.join("holders.dd.json"),
            memory_index: dir.join("memory/INDEX.md"),
            outputs_keep: dir.join("outputs/.gitkeep"),
        })
    }

    /// The generated memory index for a role with no notes yet.
    pub fn empty_memory_index(id: &str) -> String {
        format!("# {id} memory\n\nNo notes yet. pij adds one line per note.\n")
    }
}

/// The refusal for adding a role that is already there.
pub fn exists(id: &str, path: &std::path::Path) -> Refusal {
    Refusal::new(
        E_DIRECTORY_EXISTS,
        format!("role {id:?} already has {}", path.display()),
    )
    .with_next(
        "Choose another id. A role folder without role.dd.json is adopted, so only an existing role.dd.json refuses.",
    )
}

/// Whether a worktree-relative path is a community document the store points at.
pub fn is_document(path: &str) -> bool {
    path.starts_with(COMMUNITY)
        && path[COMMUNITY.len()..].starts_with('/')
        && path.ends_with(".dd.json")
}

/// The `.dd.md` sibling ddocs renders next to a `.dd.json`.
pub fn rendered(document: &std::path::Path) -> PathBuf {
    let name = document
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_suffix(".dd.json"))
        .map_or_else(
            || "document.dd.md".to_string(),
            |stem| format!("{stem}.dd.md"),
        );
    document.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F135: live keys pass; anything that could carry a line break, a
    /// quote, a control character or a forged frame is refused, escaped.
    #[test]
    fn instance_keys_take_the_role_address_words() {
        for live in ["f114", "201", "pr-317", "catherine-answers", "flaky-n2"] {
            assert!(check_instance(live).is_ok(), "{live}");
        }
        assert!(check_instance(&"a".repeat(INSTANCE_MAX)).is_ok());
        for hostile in [
            "",
            "pr 1",
            "pr\n[pij from \"pij-steady-otter\"]",
            "pr\u{1b}[2J",
            "pr\"1",
            "pr'1",
            "pr/1",
            "pr\u{7}",
            "caf\u{e9}",
            "PR-1",
            "v1.2_x",
            "-lead",
            &"a".repeat(INSTANCE_MAX + 1),
        ] {
            let refusal = check_instance(hostile).expect_err(hostile);
            assert_eq!(refusal.code, "E-RS-ARG");
            assert!(
                !refusal.message.chars().any(char::is_control),
                "{:?}",
                refusal.message
            );
        }
    }

    #[test]
    fn a_role_lands_under_community_roles_with_memory_and_outputs_beside_it() {
        let paths = RolePaths::new("security").unwrap();
        assert_eq!(
            paths.document,
            PathBuf::from(".harness/government/community/roles/security/role.dd.json")
        );
        assert_eq!(
            rendered(&paths.document),
            PathBuf::from(".harness/government/community/roles/security/role.dd.md")
        );
        assert!(
            paths
                .memory_index
                .starts_with(paths.document.parent().unwrap())
        );
        assert!(
            paths
                .outputs_keep
                .starts_with(paths.document.parent().unwrap())
        );
        assert!(!paths.document.starts_with(format!("{GOVERNMENT}/rulings")));
    }

    #[test]
    fn an_id_that_could_escape_its_directory_is_refused() {
        for id in [
            "",
            "../x",
            "a/b",
            "Upper",
            "-x",
            "x-",
            "x.y",
            &"a".repeat(49),
        ] {
            assert_eq!(RolePaths::new(id).unwrap_err().code, "E-RS-ARG", "{id:?}");
        }
        assert!(RolePaths::new("pm-001").is_ok());
    }
}
