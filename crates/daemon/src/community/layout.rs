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
const SCHEMAS: [(&str, &[u8]); 3] = [
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
];

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
        format!("role {id:?} already exists at {}", path.display()),
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
