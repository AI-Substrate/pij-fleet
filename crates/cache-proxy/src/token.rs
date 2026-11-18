//! The account token's source (blueprint 003 c-token-holder): `gh auth token
//! --user <account>`, with `gh` found once at start and launched by absolute
//! path. The token arrives on gh's stdout and goes straight into memory; it is
//! never logged, written or put in an argument list.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use pij_core::program::Resolver;

/// The variable that names gh's absolute path, as for the daemon (plan 196).
pub const GH_ENV: &str = "PIJ_GH";

/// The launch-counter name for every token fetch.
pub const SPAWN_OP: &str = "gh.cache_proxy_token";

/// How long one `gh auth token` may run.
const GH_TIMEOUT: Duration = Duration::from_secs(15);

/// Find gh once: `$PIJ_GH` when set (absolute and executable), else the first
/// `gh` on `path`, made absolute against `cwd`.
///
/// # Errors
/// Why there is none, naming the variable.
pub fn resolve_gh(
    chosen: Option<&OsStr>,
    path: Option<&OsStr>,
    cwd: &Path,
) -> Result<PathBuf, String> {
    let resolver = Resolver::new();
    if let Some(chosen) = chosen.filter(|value| !value.is_empty()) {
        let chosen = Path::new(chosen);
        if !chosen.is_absolute() {
            return Err(format!(
                "{GH_ENV}={} is not an absolute path",
                chosen.display()
            ));
        }
        let (Some(dir), Some(name)) = (chosen.parent(), chosen.file_name()) else {
            return Err(format!(
                "{GH_ENV}={} is not an executable file",
                chosen.display()
            ));
        };
        // A file posix_spawn could run: searched for in its own directory only.
        return if resolver.resolve(name, Some(dir.as_os_str())) == chosen.as_os_str() {
            Ok(chosen.to_path_buf())
        } else {
            Err(format!(
                "{GH_ENV}={} is not an executable file",
                chosen.display()
            ))
        };
    }
    let absolute = path.and_then(|path| {
        std::env::join_paths(
            std::env::split_paths(path).map(|dir| cwd.join(dir).components().collect::<PathBuf>()),
        )
        .ok()
    });
    let found = resolver.resolve(OsStr::new("gh"), absolute.as_deref().or(path));
    if Path::new(&found).is_absolute() {
        Ok(PathBuf::from(found))
    } else {
        Err(format!(
            "gh is not on the sidecar's PATH and {GH_ENV} is unset"
        ))
    }
}

/// One `gh auth token --user <account>` launch, counted as [`SPAWN_OP`].
///
/// # Errors
/// gh failed, timed out or printed nothing usable. The message carries gh's
/// first stderr line, never its stdout.
pub async fn fetch(gh: &Path, account: &str) -> Result<String, String> {
    pij_core::spawns::record(SPAWN_OP);
    let child = tokio::process::Command::new(gh)
        .args(["auth", "token", "--user", account])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("gh could not start: {error}"))?;
    let output = tokio::time::timeout(GH_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| format!("gh auth token did not finish in {}s", GH_TIMEOUT.as_secs()))?
        .map_err(|error| format!("gh: {error}"))?;
    let token = String::from_utf8(output.stdout)
        .map(|text| text.trim().to_string())
        .unwrap_or_default();
    if output.status.success() && usable(&token) {
        return Ok(token);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let first = stderr.lines().next().unwrap_or("").trim();
    Err(if first.is_empty() {
        format!("gh has no token for {account} ({})", output.status)
    } else {
        format!("gh has no token for {account}: {first}")
    })
}

/// A token that can go in an `Authorization` header: visible ASCII only.
fn usable(token: &str) -> bool {
    !token.is_empty() && token.bytes().all(|byte| byte.is_ascii_graphic())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn pij_gh_must_be_absolute_and_executable() {
        let cwd = Path::new("/");
        assert!(
            resolve_gh(Some(OsStr::new("gh")), None, cwd)
                .unwrap_err()
                .contains("not an absolute path")
        );
        assert!(
            resolve_gh(Some(OsStr::new("/nonexistent/gh")), None, cwd)
                .unwrap_err()
                .contains("not an executable file")
        );
        let dir = std::env::temp_dir().join(format!("pij-cache-proxy-gh-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let gh = dir.join("gh");
        std::fs::write(&gh, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(resolve_gh(Some(gh.as_os_str()), None, cwd), Ok(gh.clone()));
        // Found on PATH; a relative PATH entry is made absolute against cwd.
        assert_eq!(resolve_gh(None, Some(dir.as_os_str()), cwd), Ok(gh.clone()));
        let name = dir.file_name().unwrap();
        assert_eq!(resolve_gh(None, Some(name), dir.parent().unwrap()), Ok(gh));
        assert!(resolve_gh(None, Some(OsStr::new("/nonexistent")), cwd).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_a_visible_ascii_token_is_usable() {
        assert!(usable("gho_abc123"));
        assert!(!usable(""));
        assert!(!usable("two words"));
        assert!(!usable("caf\u{e9}"));
    }
}
