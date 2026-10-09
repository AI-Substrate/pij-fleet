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
        /// The schema field on that line, when it names one.
        field: Option<&'static str>,
    },
    /// An alias is empty, longer than 63 characters, or uses characters outside
    /// `[A-Za-z0-9._-]`. Names WHICH alias, never its text: it is not a valid
    /// name, so it may be anything, a pasted key included.
    BadAlias(AliasSite),
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
                let what = match field {
                    Some(field) => format!("`{field}` at {line} is not in the documented shape"),
                    None => format!("unrecognised content at {line}"),
                };
                write!(
                    formatter,
                    "peers.toml: {what}: expected `machine = \"<alias>\"` and [[peer]] tables of alias, url and key strings (file text is never shown)"
                )
            }
            Self::BadAlias(site) => write!(
                formatter,
                "{site} must be 1-63 characters of letters, digits, `.`, `_` and `-`"
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

/// Which alias a [`PairingError::BadAlias`] is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AliasSite {
    /// The file's own `machine = …`.
    Machine,
    /// The `alias` of the Nth `[[peer]]` (1-based).
    Peer(usize),
}

impl fmt::Display for AliasSite {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Machine => formatter.write_str("`machine`"),
            Self::Peer(index) => write!(formatter, "the alias of [[peer]] #{index}"),
        }
    }
}

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
    // At most 63 (a DNS label): a 64-character generated key is never a name.
    (1..=63).contains(&alias.len())
        && alias
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
}

/// A peer URL must be a usable HTTP base: `http(s)://host[:port]` and nothing
/// else, parsed by the HTTP client's own URL parser so an authority it cannot
/// use (no host, port out of range) is refused here, not retried forever later.
fn check_url(url: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|error| error.to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("scheme must be http:// or https://".to_string());
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err("no host".to_string());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("no credentials in the URL; the key is the credential".to_string());
    }
    if parsed.path() != "/" || parsed.query().is_some() || parsed.fragment().is_some() {
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

/// Every name the schema knows. Only these are ever printed back: any other
/// text on a line may be a value, a pasted key included (review F05).
const KNOWN_FIELDS: [&str; 5] = ["machine", "peer", "alias", "url", "key"];

/// The schema field a line names (`name = …`, `[name]` or `[[name]]`), only if
/// it is one of [`KNOWN_FIELDS`]. Otherwise `None`: the line is reported as
/// unrecognised content and none of its text is shown.
fn field_name(line: &str) -> Option<&'static str> {
    let line = line.trim();
    let name = if let Some(table) = line.strip_prefix("[[") {
        table.strip_suffix("]]")?
    } else if let Some(table) = line.strip_prefix('[') {
        table.strip_suffix(']')?
    } else {
        line.split_once('=')?.0
    }
    .trim();
    KNOWN_FIELDS.into_iter().find(|known| *known == name)
}

/// Parse and validate the file's text.
///
/// # Errors
/// Malformed TOML, bad or duplicate aliases, a self alias, a bad URL, a short
/// key, or two peers sharing a key.
pub fn parse(text: &str) -> Result<Pairing, PairingError> {
    let file: PairingFile = toml::from_str(text).map_err(|error| malformed(text, &error))?;
    if !valid_alias(&file.machine) {
        return Err(PairingError::BadAlias(AliasSite::Machine));
    }
    let mut aliases = BTreeSet::new();
    let mut peers: Vec<PeerDefinition> = Vec::with_capacity(file.peers.len());
    for (index, row) in file.peers.into_iter().enumerate() {
        if !valid_alias(&row.alias) {
            return Err(PairingError::BadAlias(AliasSite::Peer(index + 1)));
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
    let path = state_dir.join(PEERS_FILE);
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(PairingError::Unreadable(path, error.to_string())),
    };
    read_checked(file, &path, expected_uid).map(Some)
}

/// Check the OPEN descriptor's owner and mode (fstat), then read that same
/// descriptor (review F09). Checking the path and reading it again would let a
/// file swapped in between be loaded unchecked.
///
/// # Errors
/// Not a regular file, wrong owner or mode, unreadable, or invalid contents.
pub fn read_checked(
    mut file: std::fs::File,
    path: &Path,
    expected_uid: u32,
) -> Result<Pairing, PairingError> {
    use std::io::Read as _;
    use std::os::unix::fs::MetadataExt as _;
    let metadata = file
        .metadata()
        .map_err(|error| PairingError::Unreadable(path.to_path_buf(), error.to_string()))?;
    if !metadata.is_file() {
        return Err(PairingError::Unreadable(
            path.to_path_buf(),
            "not a regular file".to_string(),
        ));
    }
    check_permissions(path, metadata.mode(), metadata.uid(), expected_uid)?;
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|error| PairingError::Unreadable(path.to_path_buf(), error.to_string()))?;
    parse(&text)
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
                PairingError::BadAlias(super::AliasSite::Peer(1)),
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

    /// Review F05 follow-up: whatever malformed shape a generated key is
    /// typed into, no error text (Display or Debug, the source of every
    /// stdout/stderr line and error envelope) contains it.
    #[test]
    fn no_malformed_shape_ever_echoes_a_generated_key() {
        let key = new_key().expect("key");
        let ok_head = "machine = \"m\"\n[[peer]]\nalias = \"laptop\"\nurl = \"http://h:1\"\n";
        let shapes = [
            format!("machine = \"m\"\npeer=[\n{key}\n]\n"),
            format!("machine = \"m\"\npeer = [\"{key}\"]\n"),
            format!("machine = \"m\"\npeer = {{ key = \"{key}\" }}\n"),
            format!("{ok_head}key = \"{key}\n"),
            format!("{ok_head}key = {key}\n"),
            format!("{ok_head}key = [\"{key}\"]\n"),
            format!("{ok_head}key = {{ value = \"{key}\" }}\n"),
            format!("{ok_head}key = 1\n{key}\n"),
            format!("{key}\n"),
            format!("machine = \"m\"\n{key} = \"x\"\n"),
            format!("machine = \"m\"\n[{key}]\n"),
            format!("machine = \"m\"\n[[{key}]]\n"),
            format!("machine = \"{key}\"\n"),
            format!(
                "{ok_head}key = \"{key}\"\n[[peer]]\nalias = \"{key}\"\nurl = \"http://h:2\"\nkey = \"{key}x\"\n"
            ),
            format!(
                "machine = \"m\"\n[[peer]]\nalias = \"laptop\"\nurl = \"http://{key}:1\"\nkey = \"{key}\"\n"
            ),
            format!(
                "machine = \"m\"\n[[peer]]\nalias = \"laptop\"\nurl = \"{key}\"\nkey = \"{key}\"\n"
            ),
        ];
        for shape in shapes {
            match parse(&shape) {
                Ok(_) => {}
                Err(refused) => {
                    let shown = format!("{refused} {refused:?}");
                    assert!(!shown.contains(&key), "{shape:?} echoed the key: {shown}");
                    assert!(
                        !shown.contains(&key[..16]),
                        "{shape:?} echoed part of the key: {shown}"
                    );
                }
            }
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
