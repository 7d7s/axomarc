// sovereign-git — git clone/checkout/ls-remote via git binary.
//
// Used by deploy_mode: build. The VPS clones the repo, builds the
// image locally. Uses the system `git` binary for reliability.
// Can be swapped for pure-Rust gix in a future version.

use std::path::Path;
use std::process::Command;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GitError {
    #[error("git command failed: {0}")]
    CommandFailed(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("invalid URL: {0}")]
    InvalidUrl(String),

    #[error("branch not found: {0}")]
    BranchNotFound(String),
}

/// Run a git command and return the output.
fn git_cmd(
    args: &[&str],
    repo_url: Option<&str>,
    work_dir: Option<&Path>,
) -> Result<String, GitError> {
    let mut cmd = Command::new("git");
    cmd.args(args);

    if let Some(dir) = work_dir {
        cmd.current_dir(dir);
    }

    // For clone operations, set GIT_SSH_COMMAND if needed
    if let Some(url) = repo_url {
        if url.starts_with("git@") && std::env::var("GIT_SSH_COMMAND").is_ok() {
            // GIT_SSH_COMMAND is already set by the caller
        }
    }

    let output = cmd
        .output()
        .map_err(|e| GitError::CommandFailed(format!("failed to run git: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(GitError::CommandFailed(format!(
            "git {} failed (exit {}): {}",
            args.first().unwrap_or(&"?"),
            output.status.code().unwrap_or(-1),
            stderr.trim()
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Clone a repository (shallow, depth 1 by default).
///
/// - `repo`: SSH URL (`git@github.com:user/repo.git`) or HTTPS
/// - `branch`: branch or tag to checkout
/// - `key`: optional raw SSH private key bytes (written to temp file)
/// - `dest`: destination directory
pub fn clone(repo: &str, branch: &str, key: Option<&[u8]>, dest: &Path) -> Result<(), GitError> {
    let url = normalize_url(repo)?;

    // Set up SSH key if provided
    let _key_guard = if let Some(key_data) = key {
        let guard = write_ssh_key_tempfile(key_data)?;
        std::env::set_var(
            "GIT_SSH_COMMAND",
            format!(
                "ssh -i {} -o StrictHostKeyChecking=accept-new",
                guard.path().display()
            ),
        );
        Some(guard)
    } else {
        None
    };

    tracing::info!(repo = %url, branch, dest = %dest.display(), "git clone");

    // Ensure dest exists
    std::fs::create_dir_all(dest)?;

    // Clone (shallow, single branch)
    git_cmd(
        &[
            "clone",
            "--depth",
            "1",
            "--branch",
            branch,
            &url,
            &dest.to_string_lossy(),
        ],
        None,
        None,
    )?;

    tracing::info!(branch, "git clone complete");
    Ok(())
}

/// Checkout a specific branch in an existing repo.
pub fn checkout(path: &Path, branch: &str) -> Result<(), GitError> {
    tracing::info!(path = %path.display(), branch, "git checkout");

    git_cmd(&["checkout", branch], None, Some(path))?;

    Ok(())
}

/// List remote references (heads and tags).
pub fn ls_remote(repo: &str, key: Option<&[u8]>) -> Result<Vec<(String, String)>, GitError> {
    let url = normalize_url(repo)?;

    let _key_guard = if let Some(key_data) = key {
        let guard = write_ssh_key_tempfile(key_data)?;
        std::env::set_var(
            "GIT_SSH_COMMAND",
            format!(
                "ssh -i {} -o StrictHostKeyChecking=accept-new",
                guard.path().display()
            ),
        );
        Some(guard)
    } else {
        None
    };

    tracing::info!(repo = %url, "git ls-remote");

    let output = git_cmd(&["ls-remote", &url], None, None)?;

    let mut refs = Vec::new();
    for line in output.lines() {
        let parts: Vec<&str> = line.splitn(2, '\t').collect();
        if parts.len() == 2 {
            refs.push((parts[1].to_string(), parts[0].to_string()));
        }
    }

    Ok(refs)
}

/// Get the latest commit SHA for the current HEAD.
pub fn latest_commit(path: &Path) -> Result<String, GitError> {
    let output = git_cmd(&["rev-parse", "HEAD"], None, Some(path))?;
    Ok(output.trim().to_string())
}

/// Normalize the URL for auth (basic validation).
fn normalize_url(repo: &str) -> Result<String, GitError> {
    if repo.starts_with("https://")
        || repo.starts_with("http://")
        || repo.starts_with("git@")
        || (repo.contains('@') && repo.contains(':') && !repo.starts_with("http") && !repo.starts_with("ftp"))
    {
        Ok(repo.to_string())
    } else {
        Err(GitError::InvalidUrl(format!(
            "unsupported URL format: {repo}"
        )))
    }
}

/// Write an SSH private key to a temporary file with 600 permissions.
/// Returns a guard that cleans up the file on drop.
fn write_ssh_key_tempfile(key: &[u8]) -> Result<tempfile::TempDir, GitError> {
    let dir = tempfile::tempdir().map_err(GitError::Io)?;
    let key_path = dir.path().join("deploy_key");

    std::fs::write(&key_path, key)?;

    // Set 600 permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))?;
    }

    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_https_url() {
        let url = normalize_url("https://github.com/user/repo.git").unwrap();
        assert_eq!(url, "https://github.com/user/repo.git");
    }

    #[test]
    fn normalize_ssh_url() {
        let url = normalize_url("git@github.com:user/repo.git").unwrap();
        assert_eq!(url, "git@github.com:user/repo.git");
    }

    #[test]
    fn normalize_invalid_url() {
        let result = normalize_url("ftp://example.com");
        assert!(result.is_err());
    }
}
