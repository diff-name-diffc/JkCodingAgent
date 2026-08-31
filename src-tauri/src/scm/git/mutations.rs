//! 写操作 Git 命令：暂存、提交、分支创建/切换、推送、拉取。
//!
//! 所有会改变仓库状态的命令集中于此；只读查询见 `queries`。

use anyhow::Context;

use super::exec::{run_git, run_git_check, validate_git_ref_name};
use super::{GitError, GitResult};
use crate::shared::error::{CommandResult, IntoCommandResult};

#[tauri::command]
pub async fn git_checkout_branch(
    project_path: String,
    branch_name: String,
    is_remote: bool,
) -> CommandResult<()> {
    git_checkout_branch_impl(project_path.clone(), branch_name.clone(), is_remote)
        .await
        .with_context(|| format!("切换 Git 分支失败（{} -> {}）", project_path, branch_name))
        .into_command_result()
}

async fn git_checkout_branch_impl(
    project_path: String,
    branch_name: String,
    is_remote: bool,
) -> GitResult<()> {
    validate_git_ref_name(&branch_name)?;
    let args: Vec<String> = if is_remote {
        // "origin/main" -> local name "main", track remote
        let local_name = branch_name
            .split_once('/')
            .map(|(_, n)| n.to_string())
            .unwrap_or_else(|| branch_name.clone());
        vec![
            "checkout".into(),
            "-b".into(),
            local_name,
            "--track".into(),
            format!("remotes/{}", branch_name),
        ]
    } else {
        vec!["checkout".into(), branch_name.clone()]
    };
    run_git_check(&project_path, &args).await
}

#[tauri::command]
pub async fn git_create_branch(
    project_path: String,
    branch_name: String,
    from_branch: String,
) -> CommandResult<()> {
    git_create_branch_impl(
        project_path.clone(),
        branch_name.clone(),
        from_branch.clone(),
    )
    .await
    .with_context(|| {
        format!(
            "创建 Git 分支失败（{}: {} from {}）",
            project_path, branch_name, from_branch
        )
    })
    .into_command_result()
}

async fn git_create_branch_impl(
    project_path: String,
    branch_name: String,
    from_branch: String,
) -> GitResult<()> {
    validate_git_ref_name(&branch_name)?;
    validate_git_ref_name(&from_branch)?;
    run_git_check(
        &project_path,
        &["checkout", "-b", &branch_name, &from_branch],
    )
    .await
}

#[tauri::command]
pub async fn git_stage(project_path: String, file_path: String) -> CommandResult<()> {
    git_stage_impl(project_path.clone(), file_path.clone())
        .await
        .with_context(|| format!("暂存 Git 文件失败（{}: {}）", project_path, file_path))
        .into_command_result()
}

async fn git_stage_impl(project_path: String, file_path: String) -> GitResult<()> {
    run_git_check(&project_path, &["add", "--", &file_path]).await
}

#[tauri::command]
pub async fn git_unstage(project_path: String, file_path: String) -> CommandResult<()> {
    git_unstage_impl(project_path.clone(), file_path.clone())
        .await
        .with_context(|| format!("取消暂存 Git 文件失败（{}: {}）", project_path, file_path))
        .into_command_result()
}

async fn git_unstage_impl(project_path: String, file_path: String) -> GitResult<()> {
    run_git_check(&project_path, &["restore", "--staged", "--", &file_path]).await
}

#[tauri::command]
pub async fn git_stage_all(project_path: String) -> CommandResult<()> {
    git_stage_all_impl(project_path.clone())
        .await
        .with_context(|| format!("暂存全部 Git 变更失败（{}）", project_path))
        .into_command_result()
}

async fn git_stage_all_impl(project_path: String) -> GitResult<()> {
    run_git_check(&project_path, &["add", "-A"]).await
}

#[tauri::command]
pub async fn git_unstage_all(project_path: String) -> CommandResult<()> {
    git_unstage_all_impl(project_path.clone())
        .await
        .with_context(|| format!("取消暂存全部 Git 变更失败（{}）", project_path))
        .into_command_result()
}

async fn git_unstage_all_impl(project_path: String) -> GitResult<()> {
    run_git_check(&project_path, &["restore", "--staged", "."]).await
}

#[tauri::command]
pub async fn git_commit(project_path: String, message: String) -> CommandResult<()> {
    git_commit_impl(project_path.clone(), message)
        .await
        .with_context(|| format!("创建 Git 提交失败（{}）", project_path))
        .into_command_result()
}

async fn git_commit_impl(project_path: String, message: String) -> GitResult<()> {
    run_git_check(&project_path, &["commit", "-m", &message]).await
}

#[tauri::command]
pub async fn git_push(project_path: String, branch: Option<String>) -> CommandResult<String> {
    git_push_impl(project_path.clone(), branch.clone())
        .await
        .with_context(|| format!("推送 Git 分支失败（{}）", project_path))
        .into_command_result()
}

async fn git_push_impl(project_path: String, branch: Option<String>) -> GitResult<String> {
    if let Some(ref b) = &branch {
        validate_git_ref_name(b)?;
    }
    let mut args = vec!["push".to_string()];
    if let Some(ref b) = branch.filter(|s| !s.is_empty()) {
        args.push("origin".to_string());
        args.push(b.clone());
    }
    let output = run_git(&project_path, &args).await?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        return Err(GitError::CommandFailed(combined));
    }
    Ok(combined.trim().to_string())
}

#[tauri::command]
pub async fn git_pull(project_path: String) -> CommandResult<String> {
    git_pull_impl(project_path.clone())
        .await
        .with_context(|| format!("拉取 Git 变更失败（{}）", project_path))
        .into_command_result()
}

async fn git_pull_impl(project_path: String) -> GitResult<String> {
    let output = run_git(&project_path, &["pull"]).await?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        return Err(GitError::CommandFailed(combined));
    }
    Ok(combined.trim().to_string())
}
