//! `<state-dir>/cache-proxy.toml` (blueprint 003 c-config-file, d-5):
//! `account = "<login>"` and `enabled = true|false`. No file, or no account,
//! means the proxy is not ready. Written only by `pij-rs cache-proxy
//! account|off|on`, read by the sidecar at start and on reload. It never holds
//! a token.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The file name inside the pij-rs state directory.
pub const CONFIG_FILE: &str = "cache-proxy.toml";

/// The two keys.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyConfig {
    /// The gh login whose token the proxy holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// The machine-wide switch; absent means on.
    #[serde(default = "enabled_default")]
    pub enabled: bool,
}

fn enabled_default() -> bool {
    true
}

impl ProxyConfig {
    /// A config with no account, switched on.
    #[must_use]
    pub fn new() -> Self {
        Self {
            account: None,
            enabled: true,
        }
    }
}

/// The config file's path under `state_dir`.
#[must_use]
pub fn path(state_dir: &Path) -> PathBuf {
    state_dir.join(CONFIG_FILE)
}

/// Read the config. A missing file is `Ok(None)`.
///
/// # Errors
/// The file exists but can't be read or parsed, or names an invalid account.
pub fn load(state_dir: &Path) -> Result<Option<ProxyConfig>, String> {
    let path = path(state_dir);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let config: ProxyConfig =
        toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    if let Some(account) = &config.account {
        validate_account(account).map_err(|error| format!("{}: {error}", path.display()))?;
    }
    Ok(Some(config))
}

/// Write the config atomically (temp file, then rename) with mode 0600.
///
/// # Errors
/// The directory or file can't be written, or the account is invalid.
pub fn save(state_dir: &Path, config: &ProxyConfig) -> Result<(), String> {
    if let Some(account) = &config.account {
        validate_account(account)?;
    }
    std::fs::create_dir_all(state_dir)
        .map_err(|error| format!("{}: {error}", state_dir.display()))?;
    let path = path(state_dir);
    let temp = state_dir.join(format!(".{CONFIG_FILE}.{}.tmp", std::process::id()));
    let text = toml::to_string(config).map_err(|error| error.to_string())?;
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temp)
        .and_then(|mut file| {
            file.write_all(text.as_bytes())?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&temp, &path));
    written.map_err(|error| {
        let _ = std::fs::remove_file(&temp);
        format!("{}: {error}", path.display())
    })
}

/// A gh login: 1 to 100 ASCII letters, digits, `-` or `_`, starting with a
/// letter or digit, so it can never read as a flag in `gh auth token --user`.
///
/// # Errors
/// What is wrong with it.
pub fn validate_account(account: &str) -> Result<(), String> {
    let valid = !account.is_empty()
        && account.len() <= 100
        && account.starts_with(|c: char| c.is_ascii_alphanumeric())
        && account
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if valid {
        Ok(())
    } else {
        Err(format!(
            "{account:?} is not a gh login (letters, digits, - and _, starting with a letter or digit)"
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pij-cache-proxy-config-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_missing_file_is_none_and_a_saved_file_reads_back_private() {
        let dir = temp_dir("roundtrip");
        assert_eq!(load(&dir), Ok(None));
        let config = ProxyConfig {
            account: Some("octo_corp".into()),
            enabled: false,
        };
        save(&dir, &config).unwrap();
        assert_eq!(load(&dir), Ok(Some(config)));
        let mode = std::fs::metadata(path(&dir)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn enabled_defaults_on_and_unknown_keys_or_bad_logins_refuse() {
        let dir = temp_dir("parse");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(path(&dir), "account = \"octocat\"\n").unwrap();
        assert_eq!(
            load(&dir),
            Ok(Some(ProxyConfig {
                account: Some("octocat".into()),
                enabled: true
            }))
        );
        std::fs::write(path(&dir), "token = \"x\"\n").unwrap();
        assert!(load(&dir).is_err());
        std::fs::write(path(&dir), "account = \"--hostname\"\n").unwrap();
        assert!(load(&dir).is_err());
        assert!(validate_account("a b").is_err());
        assert!(validate_account("").is_err());
        assert!(validate_account("octo_corp").is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
