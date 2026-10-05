//! The pairing file, `<state-dir>/peers.toml` (plan 164 ruling 1): this
//! machine's alias, and one `[[peer]]` per paired machine with its URL and the
//! pre-shared key both ends hold.
//!
//! ```toml
//! machine = "mac-studio"
//! [[peer]]
//! alias = "laptop"
//! url   = "http://100.64.0.7:7461"
//! key   = "<pre-shared key for the mac-studio ↔ laptop pair>"
//! ```
//!
//! The file is a credential store, so the daemon refuses to start unless it is
//! owned by the daemon's user and readable by nobody else. Every refusal names
//! aliases and fingerprints, never key bytes.

use std::collections::BTreeSet;
use std::fmt;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use pij_core::config::PeerDefinition;
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The file name under the state directory.
pub const PEERS_FILE: &str = "peers.toml";

/// Fewest characters a pre-shared key may have. `pij-rs peers new-key` mints
/// 64 (256 bits, hex); anything this short is a typo or a placeholder.
pub const MIN_KEY_CHARS: usize = 32;

/// A validated pairing file. `Debug` is safe: [`PeerDefinition`] redacts keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pairing {
    /// This machine's own alias.
    pub machine: String,
    /// Paired machines, in file order.
    pub peers: Vec<PeerDefinition>,
}

/// Why a pairing file is refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairingError {
    /// The file exists but could not be read.
    Unreadable(PathBuf, String),
    /// Group or other permission bits are set.
    Mode(PathBuf, u32),
    /// Owned by a different user than the daemon's.
    Owner(PathBuf, u32, u32),
    /// Not valid TOML, or not the documented shape. Carries WHERE, never a
    /// value: a key typed into the wrong shape must not reach a log (review S6).
    Malformed {
        /// 1-based line of the problem, when the parser could place it.
        line: Option<usize>,
        /// The key or table name on that line, when there is one.
        field: Option<String>,
    },
    /// An alias is empty or uses characters outside `[A-Za-z0-9._-]`.
    BadAlias(String),
    /// A peer uses this machine's own alias.
    SelfAlias(String),
    /// Two peers share an alias.
    DuplicateAlias(String),
    /// A peer URL is not an `http(s)://host[:port]` base URL.
    BadUrl(String, String),
    /// A key is shorter than [`MIN_KEY_CHARS`].
    ShortKey(String),
    /// Two peers share a key, so calls could not be attributed.
    DuplicateKey(String, String),
}

impl fmt::Display for PairingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable(path, error) => {
                write!(formatter, "could not read {}: {error}", path.display())
            }
            Self::Mode(path, mode) => write!(
                formatter,
                "{} is mode {mode:o}; it holds pre-shared keys, so it must be readable by its owner only (chmod 600)",
                path.display()
            ),
            Self::Owner(path, owner, expected) => write!(
                formatter,
                "{} is owned by uid {owner}, not the daemon's uid {expected}; refusing keys another user could have written",
                path.display()
            ),
            Self::Malformed { line, field } => {
                let line = line.map_or_else(
                    || "an unknown line".to_string(),
                    |line| format!("line {line}"),
                );
                let field = field
                    .as_deref()
                    .map(|field| format!(" (`{field}`)"))
                    .unwrap_or_default();
                write!(
                    formatter,
                    "peers.toml is malformed at {line}{field}: expected `machine = \"<alias>\"` and [[peer]] tables of alias, url and key strings (values are never shown)"
                )
            }
            Self::BadAlias(alias) => write!(
                formatter,
                "alias `{alias}` must be non-empty and use only letters, digits, `.`, `_` and `-`"
            ),
            Self::SelfAlias(alias) => write!(
                formatter,
                "peer alias `{alias}` equals this machine's own alias"
            ),
            Self::DuplicateAlias(alias) => {
                write!(formatter, "peer alias `{alias}` is listed more than once")
            }
            Self::BadUrl(alias, why) => write!(
                formatter,
                "peer `{alias}` url must be an http:// base URL such as http://100.64.0.7:7461 ({why})"
            ),
            Self::ShortKey(alias) => write!(
                formatter,
                "peer `{alias}` key is shorter than {MIN_KEY_CHARS} characters; mint one with `pij-rs peers new-key`"
            ),
            Self::DuplicateKey(first, second) => write!(
                formatter,
                "peers `{first}` and `{second}` share one key; every machine pair needs its own"
            ),
        }
    }
}

impl std::error::Error for PairingError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PairingFile {
    machine: String,
    #[serde(default, rename = "peer")]
    peers: Vec<PeerRow>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PeerRow {
    alias: String,
    url: String,
    key: String,
}

fn valid_alias(alias: &str) -> bool {
    !alias.is_empty()
        && alias
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
}

fn check_url(url: &str) -> Result<(), String> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .ok_or_else(|| "scheme must be http:// or https://".to_string())?;
    let authority = rest.trim_end_matches('/');
    if authority.is_empty() {
        return Err("no host".to_string());
    }
    if authority.contains(['/', '?', '#', '@']) {
        return Err("give only scheme, host and port".to_string());
    }
    Ok(())
}

/// Where a parse error is, without what is there: the parser's own message
/// can quote the offending value, which may be a key.
fn malformed(text: &str, error: &toml::de::Error) -> PairingError {
    let line = error
        .span()
        .map(|span| text[..span.start.min(text.len())].matches('\n').count() + 1);
    let field = line
        .and_then(|line| text.lines().nth(line - 1))
        .and_then(field_name);
    PairingError::Malformed { line, field }
}

/// The key (`name = …`) or table (`[[name]]`) a line names, if it is a plain
/// identifier. Anything else on the line is a value and is never returned.
fn field_name(line: &str) -> Option<String> {
    let line = line.trim();
    let name = match line.strip_prefix('[') {
        Some(table) => table.trim_start_matches('[').split(']').next()?,
        None => line.split('=').next()?,
    }
    .trim();
    valid_alias(name).then(|| name.to_string())
}

/// Parse and validate the file's text.
///
/// # Errors
/// Malformed TOML, bad or duplicate aliases, a self alias, a bad URL, a short
/// key, or two peers sharing a key.
pub fn parse(text: &str) -> Result<Pairing, PairingError> {
    let file: PairingFile = toml::from_str(text).map_err(|error| malformed(text, &error))?;
    if !valid_alias(&file.machine) {
        return Err(PairingError::BadAlias(file.machine));
    }
    let mut aliases = BTreeSet::new();
    let mut peers: Vec<PeerDefinition> = Vec::with_capacity(file.peers.len());
    for row in file.peers {
        if !valid_alias(&row.alias) {
            return Err(PairingError::BadAlias(row.alias));
        }
        if row.alias == file.machine {
            return Err(PairingError::SelfAlias(row.alias));
        }
        if !aliases.insert(row.alias.clone()) {
            return Err(PairingError::DuplicateAlias(row.alias));
        }
        check_url(&row.url).map_err(|why| PairingError::BadUrl(row.alias.clone(), why))?;
        if row.key.chars().count() < MIN_KEY_CHARS {
            return Err(PairingError::ShortKey(row.alias));
        }
        if let Some(first) = peers.iter().find(|peer| peer.key == row.key) {
            return Err(PairingError::DuplicateKey(first.alias.clone(), row.alias));
        }
        peers.push(PeerDefinition {
            alias: row.alias,
            url: row.url.trim_end_matches('/').to_string(),
            key: row.key,
        });
    }
    Ok(Pairing {
        machine: file.machine,
        peers,
    })
}

/// Check the file's ownership and mode: owned by `expected_uid`, and no group
/// or other permission bit set.
///
/// # Errors
/// [`PairingError::Owner`] or [`PairingError::Mode`].
pub fn check_permissions(
    path: &Path,
    mode: u32,
    owner: u32,
    expected_uid: u32,
) -> Result<(), PairingError> {
    if owner != expected_uid {
        return Err(PairingError::Owner(path.to_path_buf(), owner, expected_uid));
    }
    if mode & 0o077 != 0 {
        return Err(PairingError::Mode(path.to_path_buf(), mode & 0o7777));
    }
    Ok(())
}

/// Read `<state_dir>/peers.toml`. `Ok(None)` when it does not exist: no
/// machine is paired, which is the default and the safe state.
///
/// # Errors
/// An unreadable file, wrong owner or mode, or invalid contents.
pub fn load(state_dir: &Path, expected_uid: u32) -> Result<Option<Pairing>, PairingError> {
    use std::os::unix::fs::MetadataExt as _;
    let path = state_dir.join(PEERS_FILE);
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(PairingError::Unreadable(path, error.to_string())),
    };
    check_permissions(&path, metadata.mode(), metadata.uid(), expected_uid)?;
    let text = std::fs::read_to_string(&path)
        .map_err(|error| PairingError::Unreadable(path.clone(), error.to_string()))?;
    parse(&text).map(Some)
}

/// The printable stand-in for a key: the first 8 hex characters of its SHA-256.
pub fn fingerprint(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    let mut text = String::with_capacity(8);
    for byte in &digest[..4] {
        write!(&mut text, "{byte:02x}").expect("writing to a String cannot fail");
    }
    text
}

/// A fresh 256-bit pre-shared key, hex-encoded, from OS entropy.
///
/// # Errors
/// The OS refused randomness.
pub fn new_key() -> pij_core::error::Result<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| pij_core::error::PijError::Adapter {
        adapter: "daemon/pairing".to_string(),
        message: format!("the OS refused randomness for a key: {error}"),
    })?;
    let mut key = String::with_capacity(64);
    for byte in bytes {
        write!(&mut key, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{MIN_KEY_CHARS, PairingError, check_permissions, fingerprint, new_key, parse};

    const KEY_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const KEY_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn file(peers: &[(&str, &str, &str)]) -> String {
        let mut text = "machine = \"mac-studio\"\n".to_string();
        for (alias, url, key) in peers {
            text.push_str(&format!(
                "[[peer]]\nalias = \"{alias}\"\nurl = \"{url}\"\nkey = \"{key}\"\n"
            ));
        }
        text
    }

    #[test]
    fn a_valid_file_names_this_machine_and_its_peers_in_order() {
        let pairing = parse(&file(&[
            ("laptop", "http://100.64.0.7:7461/", KEY_A),
            ("desktop", "http://100.64.0.8:7461", KEY_B),
        ]))
        .expect("valid");
        assert_eq!(pairing.machine, "mac-studio");
        let aliases: Vec<&str> = pairing
            .peers
            .iter()
            .map(|peer| peer.alias.as_str())
            .collect();
        assert_eq!(aliases, ["laptop", "desktop"]);
        assert_eq!(pairing.peers[0].url, "http://100.64.0.7:7461");
        assert!(
            parse("machine = \"solo\"\n")
                .expect("no peers")
                .peers
                .is_empty()
        );
    }

    #[test]
    fn every_unsafe_or_ambiguous_pairing_is_refused_without_echoing_keys() {
        let cases = [
            (
                file(&[
                    ("laptop", "http://h:1", KEY_A),
                    ("laptop", "http://h:2", KEY_B),
                ]),
                PairingError::DuplicateAlias("laptop".into()),
            ),
            (
                file(&[
                    ("laptop", "http://h:1", KEY_A),
                    ("desktop", "http://h:2", KEY_A),
                ]),
                PairingError::DuplicateKey("laptop".into(), "desktop".into()),
            ),
            (
                file(&[("mac-studio", "http://h:1", KEY_A)]),
                PairingError::SelfAlias("mac-studio".into()),
            ),
            (
                file(&[("lap@top", "http://h:1", KEY_A)]),
                PairingError::BadAlias("lap@top".into()),
            ),
            (
                file(&[("laptop", "http://h:1", &KEY_A[..MIN_KEY_CHARS - 1])]),
                PairingError::ShortKey("laptop".into()),
            ),
        ];
        for (text, expected) in cases {
            let refused = parse(&text).expect_err("refused");
            assert_eq!(refused, expected);
            let shown = refused.to_string();
            assert!(!shown.contains(KEY_A) && !shown.contains(KEY_B), "{shown}");
        }
        assert!(matches!(
            parse(&file(&[("laptop", "ftp://h:1", KEY_A)])),
            Err(PairingError::BadUrl(..))
        ));
        assert!(matches!(
            parse(&file(&[("laptop", "http://h:1/v1/send", KEY_A)])),
            Err(PairingError::BadUrl(..))
        ));
        assert!(matches!(
            parse("machine = \"m\"\nsecret = 1\n"),
            Err(PairingError::Malformed { .. })
        ));
    }

    /// Review S6: a malformed file names the line and field, never a value, so
    /// a key typed into the wrong shape is not echoed into a log.
    #[test]
    fn a_malformed_file_never_echoes_a_value() {
        for text in [
            format!("machine = \"m\"\npeer = [\"{KEY_A}\"]\n"),
            format!(
                "machine = \"m\"\n[[peer]]\nalias = \"laptop\"\nurl = \"http://h:1\"\nkey = [\"{KEY_A}\"]\n"
            ),
            format!("machine = \"m\"\nmystery = \"{KEY_A}\"\n"),
        ] {
            let refused = parse(&text).expect_err("malformed").to_string();
            assert!(!refused.contains(KEY_A), "{refused}");
            assert!(refused.contains("line"), "names the line: {refused}");
        }
    }

    /// Review F10: an authority no HTTP client can use is a configuration
    /// error at load, not a background retry loop later.
    #[test]
    fn an_unusable_peer_url_is_refused_at_load() {
        for url in [
            "http://:7461",
            "http://127.0.0.1:99999",
            "http://user:pass@100.64.0.7:7461",
            "http://100.64.0.7:7461/base",
            "http://100.64.0.7:7461?x=1",
            "http://100.64.0.7:7461#frag",
            "http://",
        ] {
            assert!(
                matches!(
                    parse(&file(&[("laptop", url, KEY_A)])),
                    Err(PairingError::BadUrl(..))
                ),
                "{url} must be refused"
            );
        }
        for url in [
            "http://100.64.0.7:7461",
            "https://studio.tailnet.ts.net",
            "http://[fd7a:115c:a1e0::1]:7461/",
        ] {
            assert!(
                parse(&file(&[("laptop", url, KEY_A)])).is_ok(),
                "{url} is usable"
            );
        }
    }

    /// Review F09: the bytes read are the bytes whose owner and mode were
    /// checked. The checked descriptor is read even after the path is replaced.
    #[test]
    fn the_checked_descriptor_is_the_one_read() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = pij_testkit::fresh_dir("pij-pairing-fd");
        let path = dir.join(super::PEERS_FILE);
        std::fs::write(&path, file(&[("laptop", "http://h:1", KEY_A)])).expect("write");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
        let opened = std::fs::File::open(&path).expect("open");
        // Swap a world-readable impostor into the path after the open.
        let impostor = dir.join("impostor.toml");
        std::fs::write(&impostor, file(&[("laptop", "http://h:1", KEY_B)])).expect("write");
        std::fs::set_permissions(&impostor, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        std::fs::rename(&impostor, &path).expect("swap");
        let uid = std::os::unix::fs::MetadataExt::uid(&opened.metadata().expect("fstat"));
        let pairing = super::read_checked(opened, &path, uid).expect("the checked file");
        assert_eq!(pairing.peers[0].key, KEY_A);
    }

    #[test]
    fn the_file_must_be_the_daemon_users_and_private() {
        let path = Path::new("/state/peers.toml");
        assert_eq!(check_permissions(path, 0o100600, 501, 501), Ok(()));
        assert_eq!(check_permissions(path, 0o100400, 501, 501), Ok(()));
        assert_eq!(
            check_permissions(path, 0o100644, 501, 501),
            Err(PairingError::Mode(path.into(), 0o644))
        );
        assert_eq!(
            check_permissions(path, 0o100610, 501, 501),
            Err(PairingError::Mode(path.into(), 0o610))
        );
        assert_eq!(
            check_permissions(path, 0o100600, 0, 501),
            Err(PairingError::Owner(path.into(), 0, 501))
        );
    }

    #[test]
    fn fingerprints_are_stable_short_and_keys_are_fresh() {
        assert_eq!(fingerprint("abc"), "ba7816bf");
        let (first, second) = (new_key().expect("key"), new_key().expect("key"));
        assert_eq!(first.len(), 64);
        assert!(first.chars().all(|character| character.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }
}
