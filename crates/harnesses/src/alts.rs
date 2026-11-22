//! F280: alternate ways to launch a harness, named per harness in
//! `<state-dir>/harness-alts.toml`:
//!
//! ```toml
//! [claude.1]
//! env = { CLAUDE_CONFIG_DIR = "~/.claude-alt" }
//! # bin = "/absolute/path/to/claude"
//! # args = ["--some-flag"]
//! ```
//!
//! `pij-rs spawn --alt <name>` launches with one; the seat records it and
//! revive re-reads it here. The file is read only then (and by
//! [`crate::claude_homes`]), never on a tick. A missing file means no alts.
//! This layer is generic: it knows one harness variable, Claude's
//! `CLAUDE_CONFIG_DIR`, only to say which Claude home an alt uses.

use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};

use pij_core::model::Harness;
use serde::Deserialize;

use crate::spawn::validate_executable_override;

/// The file name inside the pij-rs state directory.
pub const FILE: &str = "harness-alts.toml";
/// The env variable that moves Claude's configuration home.
pub const CLAUDE_HOME_ENV: &str = "CLAUDE_CONFIG_DIR";
/// Refusal code: the seat's or spawn's alt is not in the file.
pub const UNKNOWN_CODE: &str = "E-RS-ALT-UNKNOWN";
/// Refusal code: the file can't be read or is invalid.
pub const CONFIG_CODE: &str = "E-RS-ALT-CONFIG";
/// The longest alt name.
const NAME_LIMIT: usize = 32;

/// One alt as written in the file.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAlt {
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    bin: Option<String>,
    #[serde(default)]
    args: Vec<String>,
}

/// One alt, checked, with `~` expanded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alt {
    /// Its name under the harness, e.g. `1` in `[claude.1]`.
    pub name: String,
    /// Variables set on the launch, in key order.
    pub env: Vec<(String, String)>,
    /// Absolute executable that replaces the harness name, when set.
    pub bin: Option<String>,
    /// Arguments appended after pij's own.
    pub args: Vec<String>,
    /// The Claude home this alt's env names, for a Claude alt that sets one.
    pub claude_home: Option<String>,
}

/// Every alt in the file, by harness and name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HarnessAlts {
    by_harness: BTreeMap<Harness, BTreeMap<String, Alt>>,
}

/// Why an alt can't be used: a stable code and one line naming the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AltRefusal {
    /// [`UNKNOWN_CODE`] or [`CONFIG_CODE`].
    pub code: &'static str,
    /// The refusal, starting with the code.
    pub message: String,
}

impl HarnessAlts {
    /// The alt `name` of `harness`.
    #[must_use]
    pub fn get(&self, harness: Harness, name: &str) -> Option<&Alt> {
        self.by_harness.get(&harness)?.get(name)
    }

    /// The names configured for `harness`, sorted.
    #[must_use]
    pub fn names(&self, harness: Harness) -> Vec<&str> {
        self.by_harness
            .get(&harness)
            .map(|alts| alts.keys().map(String::as_str).collect())
            .unwrap_or_default()
    }

    /// Every alt with its harness, by harness then name.
    pub fn iter(&self) -> impl Iterator<Item = (Harness, &Alt)> {
        self.by_harness
            .iter()
            .flat_map(|(harness, alts)| alts.values().map(move |alt| (*harness, alt)))
    }

    /// The Claude homes the Claude alts name, in name order.
    #[must_use]
    pub fn claude_homes(&self) -> Vec<PathBuf> {
        self.iter()
            .filter(|(harness, _)| *harness == Harness::Claude)
            .filter_map(|(_, alt)| alt.claude_home.as_ref().map(PathBuf::from))
            .collect()
    }

    /// F283: the one Claude alt whose home is `home`, both canonicalised
    /// (symlinks and `..` resolved; a path that can't be resolved is compared
    /// as written). `None` when no alt or more than one alt names it.
    #[must_use]
    pub fn claude_alt_for_home(&self, home: &Path) -> Option<&Alt> {
        let canonical = |path: &Path| std::fs::canonicalize(path).unwrap_or(path.to_path_buf());
        let home = canonical(home);
        let mut matches = self.iter().filter(|(harness, alt)| {
            *harness == Harness::Claude
                && alt
                    .claude_home
                    .as_deref()
                    .is_some_and(|named| canonical(Path::new(named)) == home)
        });
        match (matches.next(), matches.next()) {
            (Some((_, alt)), None) => Some(alt),
            _ => None,
        }
    }
}

/// The file's path under `state_dir`.
#[must_use]
pub fn path(state_dir: &Path) -> PathBuf {
    state_dir.join(FILE)
}

/// The file under the default state directory, `$HOME/.pij-rs`.
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    home().map(|home| path(&home.join(".pij-rs")))
}

/// `$HOME`, when set and non-empty.
#[must_use]
pub fn home() -> Option<PathBuf> {
    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Read the file at `file`. A missing file is no alts.
///
/// # Errors
/// The file exists but can't be read, or [`parse`] refuses it; the error
/// starts with the file's path.
pub fn load(file: &Path, home: Option<&Path>) -> Result<HarnessAlts, String> {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(HarnessAlts::default());
        }
        Err(error) => return Err(format!("{}: {error}", file.display())),
    };
    parse(&text, home).map_err(|error| format!("{}: {error}", file.display()))
}

/// Look up `[<harness>.<name>]` in `file`, read now.
///
/// # Errors
/// [`CONFIG_CODE`] when the file can't be read or is invalid; [`UNKNOWN_CODE`]
/// when it has no such alt, naming the file and the harness's alts.
pub fn resolve(
    file: &Path,
    home: Option<&Path>,
    harness: Harness,
    name: &str,
) -> Result<Alt, AltRefusal> {
    let alts = load(file, home).map_err(|error| AltRefusal {
        code: CONFIG_CODE,
        message: format!("{CONFIG_CODE} harness alts unreadable: {error}"),
    })?;
    if let Some(alt) = alts.get(harness, name) {
        return Ok(alt.clone());
    }
    let known = alts.names(harness);
    let known = if known.is_empty() {
        format!("it has no [{harness}.<name>] alts")
    } else {
        format!("its {harness} alts are {}", known.join(", "))
    };
    Err(AltRefusal {
        code: UNKNOWN_CODE,
        message: format!(
            "{UNKNOWN_CODE} no alt `{}` for {harness} in {} ({known}); add [{harness}.{}] there or choose a configured alt",
            pij_core::render::line(name),
            file.display(),
            pij_core::render::line(name),
        ),
    })
}

/// Parse and check the file's text. `home` expands a leading `~`.
///
/// # Errors
/// A TOML error, an unknown harness or field, a bad alt name or env key, a
/// `PIJ_` env key, a control character, a relative `bin` (or one naming
/// another harness), or a relative Claude home.
pub fn parse(text: &str, home: Option<&Path>) -> Result<HarnessAlts, String> {
    let raw: BTreeMap<String, BTreeMap<String, RawAlt>> =
        toml::from_str(text).map_err(|error| error.to_string())?;
    let mut by_harness = BTreeMap::new();
    for (word, alts) in raw {
        let Some(harness) = Harness::parse(&word) else {
            return Err(format!(
                "unknown harness `{}`; tables are [<harness>.<name>] with harness one of claude, copilot, codex, pi, omp",
                pij_core::render::line(&word),
            ));
        };
        let mut checked = BTreeMap::new();
        for (name, raw) in alts {
            let alt = check(harness, &name, raw, home)?;
            checked.insert(name, alt);
        }
        by_harness.insert(harness, checked);
    }
    Ok(HarnessAlts { by_harness })
}

fn check(harness: Harness, name: &str, raw: RawAlt, home: Option<&Path>) -> Result<Alt, String> {
    let label = format!("[{harness}.{}]", pij_core::render::line(name));
    if name.is_empty()
        || name.chars().count() > NAME_LIMIT
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(format!(
            "{label}: an alt name is 1 to {NAME_LIMIT} of A-Z, a-z, 0-9, `.`, `_` and `-`"
        ));
    }
    let printable = |what: &str, value: &str| {
        if value.chars().any(char::is_control) {
            Err(format!("{label}: {what} has a control character"))
        } else {
            Ok(())
        }
    };
    let mut env = Vec::with_capacity(raw.env.len());
    let mut claude_home = None;
    for (key, value) in raw.env {
        let mut chars = key.chars();
        let valid = chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid {
            return Err(format!(
                "{label}: env key `{}` is not a variable name ([A-Za-z_][A-Za-z0-9_]*)",
                pij_core::render::line(&key)
            ));
        }
        if key.starts_with("PIJ_") {
            return Err(format!(
                "{label}: env key {key} is pij's own; an alt can't set a PIJ_ variable"
            ));
        }
        printable(&format!("env {key}"), &value)?;
        let value = expand(&label, &value, home)?;
        if harness == Harness::Claude && key == CLAUDE_HOME_ENV {
            if !Path::new(&value).is_absolute() {
                return Err(format!(
                    "{label}: {CLAUDE_HOME_ENV} must be absolute (or start with ~/), not `{value}`"
                ));
            }
            claude_home = Some(value.clone());
        }
        env.push((key, value));
    }
    let bin = match raw.bin {
        Some(bin) => {
            printable("bin", &bin)?;
            let bin = expand(&label, &bin, home)?;
            validate_executable_override(harness, Some(&bin))
                .map_err(|error| format!("{label}: bin: {error}"))?;
            Some(bin)
        }
        None => None,
    };
    for arg in &raw.args {
        printable("an arg", arg)?;
    }
    Ok(Alt {
        name: name.to_string(),
        env,
        bin,
        args: raw.args,
        claude_home,
    })
}

/// `~` or a leading `~/` becomes `home`; anything else is kept.
fn expand(label: &str, value: &str, home: Option<&Path>) -> Result<String, String> {
    let rest = match value.strip_prefix('~') {
        Some("") => "",
        Some(rest) if rest.starts_with('/') => &rest[1..],
        _ => return Ok(value.to_string()),
    };
    let Some(home) = home else {
        return Err(format!(
            "{label}: `{value}` starts with ~ but HOME is not set"
        ));
    };
    let expanded = if rest.is_empty() {
        home.to_path_buf()
    } else {
        home.join(rest)
    };
    expanded
        .into_os_string()
        .into_string()
        .map_err(|_| format!("{label}: HOME is not valid UTF-8"))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use pij_core::model::Harness;

    use super::{CONFIG_CODE, UNKNOWN_CODE, load, parse, resolve};

    const HOME: &str = "/home/someone";

    fn home() -> Option<&'static Path> {
        Some(Path::new(HOME))
    }

    #[test]
    fn a_full_file_parses_with_tilde_expanded() {
        let alts = parse(
            r#"
[claude.1]
env = { CLAUDE_CONFIG_DIR = "~/.claude-alt", OTHER = "~not-a-home" }
args = ["--verbose"]

[claude.work]
bin = "~/bin/claude"

[copilot.x]
env = { COPILOT_HOME = "~" }
"#,
            home(),
        )
        .expect("parses");
        let one = alts.get(Harness::Claude, "1").expect("claude.1");
        assert_eq!(
            one.env,
            vec![
                (
                    "CLAUDE_CONFIG_DIR".to_string(),
                    format!("{HOME}/.claude-alt")
                ),
                ("OTHER".to_string(), "~not-a-home".to_string()),
            ]
        );
        assert_eq!(one.args, ["--verbose"]);
        assert_eq!(one.bin, None);
        assert_eq!(
            one.claude_home.as_deref(),
            Some("/home/someone/.claude-alt")
        );
        let work = alts.get(Harness::Claude, "work").expect("claude.work");
        assert_eq!(work.bin.as_deref(), Some("/home/someone/bin/claude"));
        assert_eq!(work.claude_home, None);
        let copilot = alts.get(Harness::Copilot, "x").expect("copilot.x");
        assert_eq!(copilot.env[0].1, HOME);
        assert_eq!(
            copilot.claude_home, None,
            "only a Claude alt names a Claude home"
        );
        assert_eq!(alts.names(Harness::Claude), ["1", "work"]);
        assert_eq!(
            alts.claude_homes(),
            [PathBuf::from("/home/someone/.claude-alt")]
        );
        assert!(alts.get(Harness::Omp, "1").is_none());
    }

    #[test]
    fn each_fault_refuses_the_file_by_name() {
        for (text, fault) in [
            ("[cluade.1]\n", "unknown harness `cluade`"),
            ("[claude.1]\nenvs = {}\n", "unknown field `envs`"),
            ("[claude.\"a b\"]\n", "alt name"),
            ("[claude.\"\"]\n", "alt name"),
            (
                "[claude.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa]\n",
                "alt name is 1 to 32",
            ),
            (
                "[claude.1]\nenv = { \"1X\" = \"v\" }\n",
                "not a variable name",
            ),
            ("[claude.1]\nenv = { PIJ_SESSION_ID = \"v\" }\n", "PIJ_"),
            ("[claude.1]\nenv = { X = \"a\\nb\" }\n", "control character"),
            ("[claude.1]\nargs = [\"a\\u0000\"]\n", "control character"),
            ("[claude.1]\nbin = \"claude\"\n", "absolute"),
            (
                "[claude.1]\nbin = \"/usr/bin/copilot\"\n",
                "selects copilot",
            ),
            (
                "[claude.1]\nenv = { CLAUDE_CONFIG_DIR = \"rel/dir\" }\n",
                "must be absolute",
            ),
            ("[claude]\n1 = 2\n", "invalid type"),
        ] {
            let error = parse(text, home()).expect_err(text);
            assert!(error.contains(fault), "{text:?}: {error}");
        }
        let error = parse("[claude.1]\nbin = \"~/c\"\n", None).expect_err("no home");
        assert!(error.contains("HOME is not set"), "{error}");
    }

    #[test]
    fn a_missing_file_is_no_alts_and_lookups_refuse_with_codes() {
        let dir = pij_testkit::fresh_dir("pij-harness-alts");
        let file = dir.join("harness-alts.toml");
        assert_eq!(load(&file, home()).expect("missing"), Default::default());
        let none = resolve(&file, home(), Harness::Claude, "1").expect_err("no file");
        assert_eq!(none.code, UNKNOWN_CODE);
        assert!(
            none.message.contains("it has no [claude.<name>] alts"),
            "{}",
            none.message
        );

        std::fs::write(&file, "[claude.1]\n[claude.2]\n").expect("write");
        let unknown = resolve(&file, home(), Harness::Claude, "3").expect_err("unknown");
        assert_eq!(unknown.code, UNKNOWN_CODE);
        assert!(
            unknown.message.contains(&file.display().to_string())
                && unknown.message.contains("its claude alts are 1, 2"),
            "{}",
            unknown.message
        );
        assert_eq!(
            resolve(&file, home(), Harness::Claude, "2")
                .expect("known")
                .name,
            "2"
        );

        std::fs::write(&file, "[claude.1\n").expect("write");
        let broken = resolve(&file, home(), Harness::Claude, "1").expect_err("broken");
        assert_eq!(broken.code, CONFIG_CODE);
        assert!(
            broken.message.contains(&file.display().to_string()),
            "{}",
            broken.message
        );
    }
}
