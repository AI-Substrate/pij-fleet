//! The Copilot CLI status line, as one process (plan 180).
//!
//! The plan-156 bash script launched ~20 processes per refresh (cat, jq x2-4,
//! git x1-2, awk x2-3, basename, subshells and a `pij-rs whoami` CLI), with ~25
//! Copilot sessions refreshing every 10 s. Defender's EDR inspects every launch.
//! This renders the SAME bytes from inside `pij-rs`, so the managed script is
//! just `exec pij-rs statusline copilot`.
//!
//! Byte parity is the contract: the golden tests run the preserved plan-156
//! script (tests/fixtures/copilot-statusline-plan156.sh) on the same inputs.
//! The helpers below therefore mirror jq, bash and awk semantics, not Rust's.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

/// What `pij-rs --json whoami` would have said for this render.
#[derive(Clone, Debug)]
pub enum Whoami {
    /// The stdout text of `pij-rs --json whoami --pane $TMUX_PANE`.
    Pane(String),
    /// The seat id `pij whoami --json` named, when there is no pane.
    Plain(Option<String>),
}

/// Everything a render reads besides stdin and the git probe.
#[derive(Clone, Debug)]
pub struct Context {
    /// `$PWD`: where the effort settings files are looked up.
    pub pwd: PathBuf,
    /// `$HOME`.
    pub home: Option<PathBuf>,
    /// The whoami answer for this render.
    pub whoami: Whoami,
}

const ESC: &str = "\x1b";

/// Render the status line for one Copilot refresh.
#[must_use]
pub fn render_copilot(
    payload: &[u8],
    context: &Context,
    branch: &dyn Fn(&Path) -> Option<String>,
) -> String {
    let fields = payload_fields(payload).unwrap_or_default();
    let cwd = if fields.cwd.is_empty() {
        ".".to_string()
    } else {
        fields.cwd.clone()
    };
    let dir = basename(&cwd);
    let model = if fields.model.is_empty() {
        "?".to_string()
    } else {
        fields.model.clone()
    };
    let pct = if fields.pct.is_empty() {
        "0".to_string()
    } else {
        fields.pct.clone()
    };

    let cwd_path = Path::new(&cwd);
    let repo_dir = if cwd_path.is_absolute() {
        cwd_path.to_path_buf()
    } else {
        context.pwd.join(cwd_path)
    };
    let branch_fmt = match branch(&repo_dir) {
        Some(name) if !name.is_empty() => format!(" {ESC}[32m⎇ {name}{ESC}[0m"),
        _ => String::new(),
    };

    let pij_fmt = match &context.whoami {
        Whoami::Pane(raw) => pane_identity(trim_newlines(raw)),
        Whoami::Plain(Some(id)) if !id.is_empty() => format!(" {ESC}[34m⛓ {id}{ESC}[0m"),
        Whoami::Plain(_) => format!(" {ESC}[2m⛓ unregistered{ESC}[0m"),
    };

    let effort = effort(context).unwrap_or_else(|| "auto".to_string());

    let mut dur_fmt = String::new();
    if let Some(ms) = bash_integer(&fields.durms).filter(|ms| *ms > 0) {
        let secs = ms / 1000;
        let (hours, minutes) = (secs / 3600, (secs % 3600) / 60);
        dur_fmt = if hours > 0 {
            format!(" • {ESC}[2m{hours}h{minutes}m{ESC}[0m")
        } else {
            format!(" • {ESC}[2m{minutes}m{ESC}[0m")
        };
    }

    let mut usage_fmt = String::new();
    if !fields.aiu.is_empty() {
        usage_fmt = format!(" • {ESC}[2m{} AIU{ESC}[0m", fields.aiu);
    }
    if !fields.premium.is_empty() && awk_number(&fields.premium) > 0.0 {
        usage_fmt.push_str(&format!(
            " • {ESC}[2m{} prem{ESC}[0m",
            bash_printf_2f(&fields.premium)
        ));
    }

    let mut out = format!(
        "{ESC}[2m{pct}%{ESC}[0m {ESC}[36m{dir}{ESC}[0m{branch_fmt}{pij_fmt} • {model} • \
         {ESC}[35m{effort}{ESC}[0m • {ESC}[2m{}/{}{ESC}[0m",
        awk_fmt(&fields.used),
        awk_fmt(&fields.size),
    );
    // `%b%b`: backslash escapes are interpreted; `\c` ends ALL further output.
    for tail in [&usage_fmt, &dur_fmt] {
        let (text, stop) = printf_b(tail);
        out.push_str(&text);
        if stop {
            break;
        }
    }
    out
}

/// The eight fields the plan-156 jq pass produced. `None` is a jq error: the
/// script's `read` then saw no line and every field stayed empty.
#[derive(Debug, Default, PartialEq)]
struct Fields {
    cwd: String,
    model: String,
    used: String,
    size: String,
    pct: String,
    aiu: String,
    premium: String,
    durms: String,
}

fn payload_fields(payload: &[u8]) -> Option<Fields> {
    let root: Value = serde_json::from_slice(payload).ok()?;
    let empty = Value::Object(serde_json::Map::new());
    let c = alt(index(&root, "context_window")?, &empty);
    let default_size = Value::from(200_000);
    let size = alt(
        alt(
            index(c, "displayed_context_limit")?,
            index(c, "context_window_size")?,
        ),
        &default_size,
    );
    let size = if jq_gt_zero(size) {
        size
    } else {
        &default_size
    };
    let tokens = index(c, "current_context_tokens")?;
    let current_pct = index(c, "current_context_used_percentage")?;
    let last_pct = index(c, "used_percentage")?;
    let used = if truthy(tokens) {
        tokens.clone()
    } else if !current_pct.is_null() {
        Value::from(number(current_pct)? * number(size)? / 100.0)
    } else if !last_pct.is_null() {
        Value::from(number(last_pct)? * number(size)? / 100.0)
    } else {
        Value::from(0)
    };
    let pct = if truthy(current_pct) {
        current_pct.clone()
    } else if jq_gt_zero(size) {
        Value::from(number(&used)? / number(size)? * 100.0)
    } else {
        Value::from(0)
    };
    let workspace = index(&root, "workspace")?;
    let cwd = alt(
        alt(index(workspace, "current_dir")?, index(&root, "cwd")?),
        &Value::from("."),
    )
    .clone();
    let model_value = index(&root, "model")?;
    let model = alt(
        alt(
            index(model_value, "display_name")?,
            index(model_value, "id")?,
        ),
        &Value::from("?"),
    )
    .clone();
    let ai_used = index(&root, "ai_used")?;
    let aiu = alt(index(ai_used, "formatted")?, &Value::from("")).clone();
    let cost = index(&root, "cost")?;
    let premium = alt(index(cost, "total_premium_requests")?, &Value::from(0)).clone();
    let durms = alt(index(cost, "total_duration_ms")?, &Value::from(0)).clone();
    // jq evaluates the array in order and fails as a whole.
    let fields = [
        tostring(&cwd),
        tostring(&model),
        floor_string(&used)?,
        floor_string(size)?,
        floor_string(&pct)?,
        tostring(&aiu),
        tostring(&premium),
        tostring(&durms),
    ];
    // `read` takes one line; a value with a newline cuts the record there.
    let line = fields.join("\u{1f}");
    let line = line.split('\n').next().unwrap_or_default();
    let mut parts = line.splitn(8, '\u{1f}').map(str::to_string);
    let mut next = || parts.next().unwrap_or_default();
    Some(Fields {
        cwd: next(),
        model: next(),
        used: next(),
        size: next(),
        pct: next(),
        aiu: next(),
        premium: next(),
        durms: next(),
    })
}

/// jq `.key`: null for null input or a missing key; an error for non-objects.
fn index<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    static NULL: Value = Value::Null;
    match value {
        Value::Null => Some(&NULL),
        Value::Object(map) => Some(map.get(key).unwrap_or(&NULL)),
        _ => None,
    }
}

fn truthy(value: &Value) -> bool {
    !matches!(value, Value::Null | Value::Bool(false))
}

/// jq `a // b`.
fn alt<'a>(a: &'a Value, b: &'a Value) -> &'a Value {
    if truthy(a) { a } else { b }
}

/// jq `. > 0` under jq's total order: null < false < true < numbers < strings
/// < arrays < objects.
fn jq_gt_zero(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(_) => false,
        Value::Number(n) => n.as_f64().is_some_and(|n| n > 0.0),
        Value::String(_) | Value::Array(_) | Value::Object(_) => true,
    }
}

/// A number for jq arithmetic; anything else is a jq error.
fn number(value: &Value) -> Option<f64> {
    value.as_f64()
}

fn floor_string(value: &Value) -> Option<String> {
    Some(jq_number(number(value)?.floor()))
}

/// jq `tostring`.
fn tostring(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(n) => n.as_f64().map_or_else(|| n.to_string(), jq_number),
        other => other.to_string(),
    }
}

/// jq's shortest round-trip number text (integers print without a fraction).
fn jq_number(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 1e17 {
        format!("{value:.0}")
    } else {
        format!("{value}")
    }
}

/// `$(...)` strips trailing newlines.
fn trim_newlines(text: &str) -> &str {
    text.trim_end_matches('\n')
}

/// `basename "$CWD"`.
fn basename(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return if path.is_empty() {
            String::new()
        } else {
            "/".to_string()
        };
    }
    let name = trimmed.rsplit('/').next().unwrap_or(trimmed);
    trim_newlines(name).to_string()
}

/// The script's `case "$RS"` over the whoami JSON text.
fn pane_identity(raw: &str) -> String {
    if raw.contains("\"ok\":true") {
        let (id, fyis) = whoami_fields(raw).unwrap_or_default();
        let mut text = format!(" {ESC}[34m⛓ {id}{ESC}[0m");
        let shown = if fyis.is_empty() { "0" } else { fyis.as_str() };
        if bash_integer(shown).is_some_and(|n| n > 0) {
            text.push_str(&format!(" {ESC}[33m✉{fyis}{ESC}[0m"));
        }
        text
    } else if let Some((_, rest)) = raw.split_once("seat `") {
        let dead = rest.split('`').next().unwrap_or(rest);
        format!(" {ESC}[31m⛓ {dead} dead{ESC}[0m")
    } else if raw.contains("\"ok\":false") {
        format!(" {ESC}[2m⛓ unregistered{ESC}[0m")
    } else {
        format!(" {ESC}[2m⛓ daemon down{ESC}[0m")
    }
}

/// jq `[(.data.id // ""), (.data.pending_fyis // 0 | tostring)] | join("\u001f")`.
fn whoami_fields(raw: &str) -> Option<(String, String)> {
    let root: Value = serde_json::from_str(raw).ok()?;
    let data = index(&root, "data")?;
    let id = alt(index(data, "id")?, &Value::from("")).clone();
    let fyis = tostring(alt(index(data, "pending_fyis")?, &Value::from(0)));
    let id = match id {
        Value::String(text) => text,
        Value::Number(_) | Value::Bool(_) => tostring(&id),
        _ => return None,
    };
    let line = format!("{id}\u{1f}{fyis}");
    let line = line.split('\n').next().unwrap_or_default().to_string();
    let mut parts = line.splitn(2, '\u{1f}');
    Some((
        parts.next().unwrap_or_default().to_string(),
        parts.next().unwrap_or_default().to_string(),
    ))
}

/// The first non-empty `jq -r '.effortLevel // empty'` over the settings files.
fn effort(context: &Context) -> Option<String> {
    let home = context.home.clone().unwrap_or_default();
    let files = [
        context.pwd.join(".github/copilot/settings.local.json"),
        context.pwd.join(".github/copilot/settings.json"),
        home.join(".copilot/settings.json"),
    ];
    files.iter().find_map(|file| {
        if !fs::metadata(file).is_ok_and(|meta| meta.is_file()) {
            return None;
        }
        let root: Value = serde_json::from_slice(&fs::read(file).ok()?).ok()?;
        let value = index(&root, "effortLevel")?;
        let text = match value {
            Value::Null | Value::Bool(false) => return None,
            Value::String(text) => text.clone(),
            Value::Array(_) | Value::Object(_) => serde_json::to_string_pretty(value).ok()?,
            other => tostring(other),
        };
        let text = trim_newlines(&text).to_string();
        (!text.is_empty()).then_some(text)
    })
}

/// bash `[ "$x" -gt 0 ]`'s integer reading: optional blanks and sign, digits.
fn bash_integer(text: &str) -> Option<i64> {
    let trimmed = text.trim_matches([' ', '\t']);
    let digits = trimmed.strip_prefix(['+', '-']).unwrap_or(trimmed);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    trimmed.trim_start_matches('+').parse().ok()
}

/// awk's string-to-number: the longest leading decimal prefix, else 0.
fn awk_number(text: &str) -> f64 {
    let text = text.trim_start_matches([' ', '\t', '\n']);
    let bytes = text.as_bytes();
    let mut end = 0;
    if matches!(bytes.first(), Some(b'+' | b'-')) {
        end = 1;
    }
    let mut digits = 0;
    while bytes.get(end).is_some_and(u8::is_ascii_digit) {
        end += 1;
        digits += 1;
    }
    if bytes.get(end) == Some(&b'.') {
        end += 1;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return 0.0;
    }
    if matches!(bytes.get(end), Some(b'e' | b'E')) {
        let mut exp = end + 1;
        if matches!(bytes.get(exp), Some(b'+' | b'-')) {
            exp += 1;
        }
        if bytes.get(exp).is_some_and(u8::is_ascii_digit) {
            while bytes.get(exp).is_some_and(u8::is_ascii_digit) {
                exp += 1;
            }
            end = exp;
        }
    }
    text[..end].parse().unwrap_or(0.0)
}

/// The script's awk `fmt`: 12345 -> 12k; 1234 -> 1.2k; 1250000 -> 1.2M.
fn awk_fmt(text: &str) -> String {
    let n = awk_number(text);
    if n >= 1_000_000.0 {
        format!("{}M", c_fixed(n / 1_000_000.0, 1))
    } else if n >= 10_000.0 {
        format!("{}k", (n / 1000.0).trunc() as i64)
    } else if n >= 1000.0 {
        format!("{}k", c_fixed(n / 1000.0, 1))
    } else {
        format!("{}", n.trunc() as i64)
    }
}

/// C `printf("%.Nf")`: the exact binary value, ties to even.
fn c_fixed(value: f64, places: usize) -> String {
    format!("{value:.places$}")
}

/// bash `printf '%.2f' "$x" || echo "$x"` for the premium-request count.
fn bash_printf_2f(text: &str) -> String {
    match text.trim_start_matches([' ', '\t']).parse::<f64>() {
        Ok(value) => c_fixed(value, 2),
        Err(_) => text.to_string(),
    }
}

/// bash `printf %b`: interpret backslash escapes; `\c` stops all output.
fn printf_b(text: &str) -> (String, bool) {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        let Some(&next) = chars.peek() else {
            out.push('\\');
            break;
        };
        let simple = match next {
            '\\' => Some('\\'),
            'a' => Some('\x07'),
            'b' => Some('\x08'),
            'e' | 'E' => Some('\x1b'),
            'f' => Some('\x0c'),
            'n' => Some('\n'),
            'r' => Some('\r'),
            't' => Some('\t'),
            'v' => Some('\x0b'),
            _ => None,
        };
        if let Some(mapped) = simple {
            chars.next();
            out.push(mapped);
            continue;
        }
        match next {
            'c' => return (out, true),
            '0' => {
                chars.next();
                let mut value = 0u32;
                for _ in 0..3 {
                    match chars.peek().and_then(|c| c.to_digit(8)) {
                        Some(digit) => {
                            value = value * 8 + digit;
                            chars.next();
                        }
                        None => break,
                    }
                }
                out.push(char::from_u32(value).unwrap_or('\0'));
            }
            'x' | 'u' | 'U' => {
                let width = match next {
                    'x' => 2,
                    'u' => 4,
                    _ => 8,
                };
                chars.next();
                let mut digits = String::new();
                while digits.len() < width {
                    match chars.peek() {
                        Some(c) if c.is_ascii_hexdigit() => {
                            digits.push(*c);
                            chars.next();
                        }
                        _ => break,
                    }
                }
                if digits.is_empty() {
                    out.push('\\');
                    out.push(next);
                } else if let Some(c) = u32::from_str_radix(&digits, 16)
                    .ok()
                    .and_then(char::from_u32)
                {
                    out.push(c);
                }
            }
            _ => out.push('\\'),
        }
    }
    (out, false)
}

/// The branch `git -C <dir> rev-parse --abbrev-ref HEAD` names (falling back to
/// `--short HEAD` when detached), read from the repository files when the
/// answer is plain, and from git itself otherwise: a detached or unborn HEAD,
/// an ambiguous name, packed or unusual layouts. The common case launches
/// nothing.
#[must_use]
pub fn git_branch(dir: &Path) -> Option<String> {
    match plain_branch(dir) {
        Some(Plain::Branch(name)) => Some(name),
        Some(Plain::NotARepo) => None,
        None => git_branch_by_git(dir),
    }
}

enum Plain {
    Branch(String),
    NotARepo,
}

fn plain_branch(dir: &Path) -> Option<Plain> {
    if !dir.is_dir() {
        return Some(Plain::NotARepo);
    }
    let mut current = Some(dir);
    while let Some(candidate) = current {
        let dot_git = candidate.join(".git");
        if let Ok(meta) = fs::symlink_metadata(&dot_git) {
            let git_dir = if meta.is_dir() {
                dot_git
            } else {
                let text = fs::read_to_string(&dot_git).ok()?;
                let target = text.strip_prefix("gitdir: ")?.trim_end_matches('\n');
                let target = Path::new(target);
                if target.is_absolute() {
                    target.to_path_buf()
                } else {
                    candidate.join(target)
                }
            };
            return read_head(&git_dir).map(Plain::Branch);
        }
        current = candidate.parent();
    }
    // No `.git` anywhere up the tree: git answers "not a repository" unless
    // something unusual (GIT_DIR, a bare repo) is in play; let git decide then.
    if std::env::var_os("GIT_DIR").is_some() {
        return None;
    }
    Some(Plain::NotARepo)
}

/// `refs/heads/<name>` from HEAD, only when that ref exists and no other ref
/// shares its short name (git would then print a disambiguated name).
fn read_head(git_dir: &Path) -> Option<String> {
    let head = fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let name = head.strip_suffix('\n')?.strip_prefix("ref: refs/heads/")?;
    if name.is_empty() || name.contains(['\n', '\r']) {
        return None;
    }
    // Refs live in the common dir for linked worktrees.
    let common = match fs::read_to_string(git_dir.join("commondir")) {
        Ok(text) => {
            let path = Path::new(text.trim_end_matches('\n'));
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                git_dir.join(path)
            }
        }
        Err(_) => git_dir.to_path_buf(),
    };
    let packed = fs::read_to_string(common.join("packed-refs")).unwrap_or_default();
    let exists = |reference: &str| {
        common.join(reference).is_file()
            || packed
                .lines()
                .any(|line| line.split_once(' ').is_some_and(|(_, r)| r == reference))
    };
    if !exists(&format!("refs/heads/{name}")) {
        return None;
    }
    for other in [
        format!("refs/{name}"),
        format!("refs/tags/{name}"),
        format!("refs/remotes/{name}"),
        format!("refs/remotes/{name}/HEAD"),
    ] {
        if exists(&other) {
            return None;
        }
    }
    if name == "HEAD" {
        return None;
    }
    Some(name.to_string())
}

fn git_branch_by_git(dir: &Path) -> Option<String> {
    let run = |args: &[&str]| {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        Some(trim_newlines(&String::from_utf8_lossy(&output.stdout)).to_string())
    };
    let branch = run(&["rev-parse", "--abbrev-ref", "HEAD"])?;
    if branch == "HEAD" {
        run(&["rev-parse", "--short", "HEAD"])
    } else {
        Some(branch)
    }
}

#[cfg(test)]
#[path = "statusline_tests.rs"]
mod tests;
