//! Git diff 内容读取：统一命令 `git_diff` 按模式取提交整体 diff、
//! 提交内单文件 diff、工作区/暂存区文件 diff。
//!
//! 只读取 diff 文本并按字节上限截断，供前端差异视图渲染；
//! 仓库元数据列表查询见 `queries`。

use std::time::Duration;

use anyhow::Context;
use serde::Deserialize;

use super::exec::{run_git, run_git_with_timeout};
use super::{GitError, GitResult};
use crate::shared::error::{CommandResult, IntoCommandResult};

/// diff 读取模式：整个提交、提交内单文件、工作区/暂存区单文件。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GitDiffMode {
    Commit,
    CommitFile,
    File,
}

/// 统一的 diff 读取命令：`mode` 决定读取形态——
/// - `commit`：整个提交的 diff，需要 `commit_hash`；
/// - `commit-file`：提交内单文件 diff，需要 `commit_hash` + `file_path`；
/// - `file`：工作区/暂存区单文件 diff，需要 `file_path`（`staged` 切换暂存区）。
#[tauri::command]
pub async fn git_diff(
    project_path: String,
    mode: GitDiffMode,
    commit_hash: Option<String>,
    file_path: Option<String>,
    staged: Option<bool>,
) -> CommandResult<String> {
    let result = match mode {
        GitDiffMode::Commit => {
            let Some(commit_hash) = commit_hash else {
                return Err("commit 模式需要提供 commitHash".to_string());
            };
            git_show_diff_impl(project_path.clone(), commit_hash.clone())
                .await
                .with_context(|| {
                    format!(
                        "读取 Git 提交 diff 失败（{}: {}）",
                        project_path, commit_hash
                    )
                })
        }
        GitDiffMode::CommitFile => {
            let Some(commit_hash) = commit_hash else {
                return Err("commit-file 模式需要提供 commitHash".to_string());
            };
            let Some(file_path) = file_path else {
                return Err("commit-file 模式需要提供 filePath".to_string());
            };
            git_show_file_diff_impl(project_path.clone(), commit_hash.clone(), file_path.clone())
                .await
                .with_context(|| {
                    format!(
                        "读取 Git 提交文件 diff 失败（{}: {} {}）",
                        project_path, commit_hash, file_path
                    )
                })
        }
        GitDiffMode::File => {
            let Some(file_path) = file_path else {
                return Err("file 模式需要提供 filePath".to_string());
            };
            git_file_diff_impl(
                project_path.clone(),
                file_path.clone(),
                staged.unwrap_or(false),
            )
            .await
            .with_context(|| format!("读取 Git 文件 diff 失败（{}: {}）", project_path, file_path))
        }
    };
    result.into_command_result()
}

async fn git_show_diff_impl(project_path: String, commit_hash: String) -> GitResult<String> {
    let args = vec!["show".to_string(), "--format=".to_string(), commit_hash];
    let output = run_git_with_timeout(project_path, args, Duration::from_secs(10)).await?;
    if !output.status.success() {
        return Err(GitError::CommandFailed(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    Ok(truncate_diff_bytes(output.stdout, 500 * 1024))
}

async fn git_file_diff_impl(
    project_path: String,
    file_path: String,
    staged: bool,
) -> GitResult<String> {
    let mut args = vec!["diff".to_string()];
    if staged {
        args.push("--cached".to_string());
    }
    args.push("--".to_string());
    args.push(file_path.clone());

    let output = run_git_with_timeout(project_path.clone(), args, Duration::from_secs(10)).await?;
    let raw = output.stdout;

    // For untracked files, git diff returns nothing — fall back to --no-index diff
    if raw.is_empty() && !staged {
        let abs_path = std::path::Path::new(&project_path).join(&file_path);
        let abs_path_str = abs_path.to_string_lossy().into_owned();
        let fallback_args = vec![
            "diff".to_string(),
            "--no-index".to_string(),
            "/dev/null".to_string(),
            abs_path_str,
        ];
        let fallback =
            run_git_with_timeout(project_path, fallback_args, Duration::from_secs(10)).await?;
        return Ok(truncate_diff_bytes(fallback.stdout, 200 * 1024));
    }

    Ok(truncate_diff_bytes(raw, 200 * 1024))
}

async fn git_show_file_diff_impl(
    project_path: String,
    commit_hash: String,
    file_path: String,
) -> GitResult<String> {
    let output = run_git(
        &project_path,
        &["show", "--format=", &commit_hash, "--", &file_path],
    )
    .await?;
    if !output.status.success() {
        return Err(GitError::CommandFailed(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    Ok(truncate_diff_bytes(output.stdout, 500 * 1024))
}

/// 将 diff 原始字节按上限截断后转为字符串（字节截断语义，前端按文本渲染）。
fn truncate_diff_bytes(raw: Vec<u8>, limit: usize) -> String {
    String::from_utf8_lossy(if raw.len() > limit {
        &raw[..limit]
    } else {
        &raw
    })
    .into_owned()
}
