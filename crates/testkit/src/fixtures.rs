//! The fixture corpus, addressable by name (workshop 001 R6b).
//!
//! Testkit-first, sharpened: not just a framework but a **named, addressable
//! corpus with known answers, malformed cases included**, shipped BEFORE the
//! units that consume it. A packet cites `cli/models.json` and every seat means
//! the same bytes; a reviewer checks a claim without re-capturing anything.
//!
//! The manifest is the index and the CONTRACT: each entry carries the sha256 of
//! the bytes and, more importantly, the `answer` — what a correct implementation
//! must DO with them. A fixture without its expectation is just a file.
//!
//! [`verify_manifest`] re-hashes every entry, so a fixture cannot be edited into
//! meaning something else while its recorded answer quietly stays behind.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The corpus index.
#[derive(Clone, Debug, Deserialize)]
pub struct Manifest {
    /// Manifest schema version.
    pub version: u32,
    /// Every fixture, in file order.
    #[serde(rename = "fixture")]
    pub fixtures: Vec<Fixture>,
}

/// One fixture and the answer it is committed to.
#[derive(Clone, Debug, Deserialize)]
pub struct Fixture {
    /// Path relative to `crates/testkit/fixtures/`.
    pub path: String,
    /// sha256 of the bytes as committed.
    pub sha256: String,
    /// Size in bytes.
    pub bytes: u64,
    /// Where the bytes came from.
    pub note: String,
    /// What a correct implementation must do with them.
    pub answer: String,
}

/// The directory the corpus lives in, resolved from this crate rather than the
/// caller's working directory.
pub fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

/// Absolute path of one fixture.
pub fn path(relative: &str) -> PathBuf {
    dir().join(relative)
}

/// Read a fixture as text.
///
/// # Panics
/// If the fixture is missing — a test citing a fixture that does not exist is a
/// broken test, and failing at the read says so immediately.
pub fn read(relative: &str) -> String {
    let path = path(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("fixture {} is unreadable: {error}", path.display()))
}

/// The parsed manifest.
///
/// # Panics
/// If the committed manifest does not parse.
pub fn manifest() -> Manifest {
    toml::from_str(&read("MANIFEST.toml")).expect("the committed fixture manifest must parse")
}

/// Every entry, keyed by path.
pub fn index() -> BTreeMap<String, Fixture> {
    manifest()
        .fixtures
        .into_iter()
        .map(|fixture| (fixture.path.clone(), fixture))
        .collect()
}

/// What the manifest and the tree disagree about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CorpusFault {
    /// The manifest names a fixture that is not in the tree.
    Missing {
        /// The named path.
        path: String,
    },
    /// The bytes changed since the manifest recorded them, so the committed
    /// `answer` may no longer describe them.
    ContentChanged {
        /// The fixture.
        path: String,
        /// What the manifest recorded.
        expected: String,
        /// What the bytes hash to now.
        found: String,
    },
    /// A file sits in the corpus that no manifest entry describes — an
    /// unaddressable fixture nobody can cite, and the state R6b exists to refuse.
    Unindexed {
        /// The stray file.
        path: String,
    },
}

impl std::fmt::Display for CorpusFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CorpusFault::Missing { path } => write!(
                f,
                "{path}: named in MANIFEST.toml but absent from the corpus"
            ),
            CorpusFault::ContentChanged {
                path,
                expected,
                found,
            } => write!(
                f,
                "{path}: content changed (manifest {expected}, found {found}) — re-record the \
                 sha256 AND re-read the `answer`, which may no longer be true of these bytes"
            ),
            CorpusFault::Unindexed { path } => write!(
                f,
                "{path}: present in the corpus but not in MANIFEST.toml — an unaddressable \
                 fixture cannot be cited in a packet; index it or delete it"
            ),
        }
    }
}

/// Check the corpus against its manifest, both directions.
pub fn verify_manifest() -> Vec<CorpusFault> {
    let index = index();
    let mut faults = Vec::new();

    for (relative, fixture) in &index {
        let full = path(relative);
        let Ok(bytes) = std::fs::read(&full) else {
            faults.push(CorpusFault::Missing {
                path: relative.clone(),
            });
            continue;
        };
        let found = sha256_hex(&bytes);
        if found != fixture.sha256 {
            faults.push(CorpusFault::ContentChanged {
                path: relative.clone(),
                expected: fixture.sha256.clone(),
                found,
            });
        }
    }

    for file in walk(&dir()) {
        let relative = file
            .strip_prefix(dir())
            .expect("walk yields paths under the corpus dir")
            .to_string_lossy()
            .replace('\\', "/");
        if relative == "MANIFEST.toml" || relative.starts_with("arch/") {
            // `arch/` holds the drift gate's own metadata fixtures, which are
            // asserted directly by `tests/arch_drift.rs` rather than cited by
            // path from other units.
            continue;
        }
        if !index.contains_key(&relative) {
            faults.push(CorpusFault::Unindexed { path: relative });
        }
    }

    faults
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out.sort();
    out
}

/// sha256, hex-encoded.
///
/// Hand-rolled rather than pulled in as a dependency: the corpus check is the
/// only hashing this workspace does today, and one reviewed function is cheaper
/// than one more allow-list row. It is a checksum, never a security primitive.
pub fn sha256_hex(bytes: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let mut message = bytes.to_vec();
    let bit_len = (bytes.len() as u64) * 8;
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in message.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (i, word) in chunk.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        for (slot, value) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(value);
        }
    }

    h.iter().map(|word| format!("{word:08x}")).collect()
}
