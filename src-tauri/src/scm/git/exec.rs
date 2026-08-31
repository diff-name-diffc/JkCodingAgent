//! git 子进程执行管道与引用名校验。
//!
//! 所有 git 命令共用的底座：统一走 `spawn_blocking`，不阻塞 Tokio 运行时；
//! 可选超时包裹；失败时携带 cwd 与参数上下文。

use std::path::PathBuf;
use std::process::Output;
use std::time::Duration;

use super::GitError;

pub(crate) type GitResult<T> = std::result::Result<T, GitError>;

/// Validate a Git ref name (branch, tag, etc.) against a whitelist of safe characters.
pub(super) fn validate_git_ref_name(name: &str) -> GitResult<()> {
    if name.is_empty() {
        return Err(GitError::RefNameEmpty);
    }
    if name.len() > 256 {
        return Err(GitError::RefNameTooLong { len: name.len() });
    }
    let is_safe = name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '/' || c == '-' || c == '_' || c == '.' || c == '@');
    if !is_safe {
        return Err(GitError::RefNameIllegalChars {
            name: name.to_string(),
        });
    }
    // Block path traversal patterns
    if name.contains("..") {
        return Err(GitError::RefNameTraversal {
            name: name.to_string(),
        });
    }
    Ok(())
}

/// 执行 git 命令并返回原始 Output（spawn_blocking 版本，不阻塞 Tokio 运行时）。
pub(super) async fn run_git<S: AsRef<std::ffi::OsStr>>(
    project_path: &str,
    args: &[S],
) -> GitResult<Output> {
    let pp = project_path.to_string();
    let args: Vec<String> = args
        .iter()
        .map(|s| s.as_ref().to_string_lossy().into_owned())
        .collect();
    let cwd = PathBuf::from(&pp);
    let args_for_error = args.clone();
    tokio::task::spawn_blocking(move || {
        std::process::Command::new("git")
            .args(&args)
            .current_dir(&pp)
            .output()
            .map_err(|source| GitError::CommandIo {
                cwd,
                args: args_for_error,
                source,
            })
    })
    .await?
}

/// 带超时的 git 命令执行。
pub(super) async fn run_git_with_timeout(
    project_path: String,
    args: Vec<String>,
    timeout: Duration,
) -> GitResult<Output> {
    let cwd = PathBuf::from(&project_path);
    let args_for_error = args.clone();
    tokio::time::timeout(
        timeout,
        tokio::task::spawn_blocking(move || {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(&project_path)
                .output()
                .map_err(|source| GitError::CommandIo {
                    cwd,
                    args: args_for_error,
                    source,
                })
        }),
    )
    .await
    .map_err(|_| GitError::Timeout {
        secs: timeout.as_secs(),
    })??
}

/// 执行 git 命令，若退出码非零则将 stderr 作为错误返回（spawn_blocking 版本）。
pub(super) async fn run_git_check<S: AsRef<std::ffi::OsStr>>(
    project_path: &str,
    args: &[S],
) -> GitResult<()> {
    let output = run_git(project_path, args).await?;
    if !output.status.success() {
        return Err(GitError::CommandFailed(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(())
}
