//! 只读 Git 查询命令：状态、分支、日志、提交详情、远端计数。
//!
//! 本模块只做读取与解析投影，不改变仓库状态；写操作见 `mutations`，
//! diff 文本读取见 `diffs`，投影 DTO 见 `types`。

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Context;

use super::exec::{run_git, run_git_with_timeout, validate_git_ref_name};
use super::types::{
    GitBranchInfo, GitCommit, GitCommitDetail, GitCommitFile, GitFileChange, GitRemoteCounts,
};
use super::GitResult;
use crate::shared::error::{CommandResult, IntoCommandResult};

#[tauri::command]
pub async fn git_status(project_path: String) -> CommandResult<Vec<GitFileChange>> {
    git_status_impl(project_path.clone())
        .await
        .with_context(|| format!("读取 Git 状态失败（{}）", project_path))
        .into_command_result()
}

async fn git_status_impl(project_path: String) -> GitResult<Vec<GitFileChange>> {
    let args = vec![
        "-c".to_string(),
        "core.quotePath=false".to_string(),
        "status".to_string(),
        "--porcelain=v1".to_string(),
    ];
    let output = run_git_with_timeout(project_path, args, Duration::from_secs(5)).await?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let mut changes = Vec::new();

    for line in stdout.lines() {
        if line.len() < 3 {
            continue;
        }
        let x = &line[0..1];
        let y = &line[1..2];
        let raw_path = line[3..].to_string();
        let display_path = if raw_path.contains(" -> ") {
            raw_path
                .split(" -> ")
                .last()
                .unwrap_or(&raw_path)
                .to_string()
        } else {
            raw_path
        };

        if x == "?" && y == "?" {
            changes.push(GitFileChange {
                path: display_path,
                status: "?".to_string(),
                staged: false,
            });
        } else {
            if x != " " && x != "?" {
                changes.push(GitFileChange {
                    path: display_path.clone(),
                    status: x.to_string(),
                    staged: true,
                });
            }
            if y != " " && y != "?" {
                changes.push(GitFileChange {
                    path: display_path,
                    status: y.to_string(),
                    staged: false,
                });
            }
        }
    }
    Ok(changes)
}

#[tauri::command]
pub async fn git_list_branches(project_path: String) -> CommandResult<Vec<GitBranchInfo>> {
    git_list_branches_impl(project_path.clone())
        .await
        .with_context(|| format!("读取 Git 分支失败（{}）", project_path))
        .into_command_result()
}

async fn git_list_branches_impl(project_path: String) -> GitResult<Vec<GitBranchInfo>> {
    let output = run_git(&project_path, &["branch", "-a"]).await?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let mut branches = Vec::new();
    for line in stdout.lines() {
        if line.len() < 2 {
            continue;
        }
        let current = line.starts_with("* ");
        let raw = line[2..].trim();
        // Skip HEAD pointer lines like "remotes/origin/HEAD -> origin/main"
        if raw.contains(" -> ") {
            continue;
        }
        if let Some(without_remotes) = raw.strip_prefix("remotes/") {
            // "origin/main" -> remote = "origin", name = "origin/main"
            let name = without_remotes.to_string();
            let remote = name.split('/').next().map(|s| s.to_string());
            branches.push(GitBranchInfo {
                name,
                current,
                remote,
            });
        } else if !raw.is_empty() {
            branches.push(GitBranchInfo {
                name: raw.to_string(),
                current,
                remote: None,
            });
        }
    }
    Ok(branches)
}

#[tauri::command]
pub async fn git_log(
    project_path: String,
    limit: u32,
    search: Option<String>,
    branch: Option<String>,
) -> CommandResult<Vec<GitCommit>> {
    git_log_impl(project_path.clone(), limit, search.clone(), branch.clone())
        .await
        .with_context(|| format!("读取 Git 日志失败（{}）", project_path))
        .into_command_result()
}

async fn git_log_impl(
    project_path: String,
    limit: u32,
    search: Option<String>,
    branch: Option<String>,
) -> GitResult<Vec<GitCommit>> {
    let limit_str = limit.to_string();
    let format = "COMMIT:%H%nSHORT:%h%nAUTHOR:%an%nDATE:%ar%nSUBJECT:%s%nREFS:%D%nEND_RECORD";
    let mut args: Vec<String> = vec![
        "log".into(),
        format!("--format={}", format),
        "-n".into(),
        limit_str,
    ];
    if let Some(ref s) = search {
        if !s.is_empty() {
            args.push("--grep".into());
            args.push(s.clone());
        }
    }
    if let Some(ref b) = branch {
        if !b.is_empty() {
            validate_git_ref_name(b)?;
            args.push(b.clone());
        }
    }

    let output = run_git_with_timeout(project_path, args, Duration::from_secs(10)).await?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let mut commits = Vec::new();
    let mut hash = String::new();
    let mut short_hash = String::new();
    let mut author = String::new();
    let mut date = String::new();
    let mut message = String::new();
    let mut refs: Vec<String> = Vec::new();

    for line in stdout.lines() {
        if let Some(v) = line.strip_prefix("COMMIT:") {
            hash = v.to_string();
        } else if let Some(v) = line.strip_prefix("SHORT:") {
            short_hash = v.to_string();
        } else if let Some(v) = line.strip_prefix("AUTHOR:") {
            author = v.to_string();
        } else if let Some(v) = line.strip_prefix("DATE:") {
            date = v.to_string();
        } else if let Some(v) = line.strip_prefix("SUBJECT:") {
            message = v.to_string();
        } else if let Some(v) = line.strip_prefix("REFS:") {
            refs = v
                .split(", ")
                .filter(|s| !s.is_empty())
                .map(|s| s.trim().to_string())
                .collect();
        } else if line == "END_RECORD" && !hash.is_empty() {
            commits.push(GitCommit {
                hash: hash.clone(),
                short_hash: short_hash.clone(),
                author: author.clone(),
                date: date.clone(),
                message: message.clone(),
                refs: refs.clone(),
            });
            hash.clear();
            short_hash.clear();
            author.clear();
            date.clear();
            message.clear();
            refs.clear();
        }
    }
    Ok(commits)
}

#[tauri::command]
pub async fn git_commit_detail(
    project_path: String,
    commit_hash: String,
) -> CommandResult<GitCommitDetail> {
    git_commit_detail_impl(project_path.clone(), commit_hash.clone())
        .await
        .with_context(|| format!("读取 Git 提交详情失败（{}: {}）", project_path, commit_hash))
        .into_command_result()
}

async fn git_commit_detail_impl(
    project_path: String,
    commit_hash: String,
) -> GitResult<GitCommitDetail> {
    // Run all 3 git commands in parallel instead of sequentially
    let info_args: Vec<&str> = vec![
        "show",
        "--no-patch",
        "--format=HASH:%H%nSHORT:%h%nAUTHOR:%an%nDATE:%ar%nSUBJECT:%s",
        &commit_hash,
    ];
    let ns_args: Vec<&str> = vec![
        "diff-tree",
        "--no-commit-id",
        "-r",
        "--name-status",
        &commit_hash,
    ];
    let num_args: Vec<&str> = vec![
        "diff-tree",
        "--no-commit-id",
        "-r",
        "--numstat",
        &commit_hash,
    ];
    let (info_out, ns_out, num_out) = tokio::try_join!(
        run_git(&project_path, &info_args),
        run_git(&project_path, &ns_args),
        run_git(&project_path, &num_args),
    )?;

    let info_str = String::from_utf8_lossy(&info_out.stdout);
    let mut hash = String::new();
    let mut short_hash = String::new();
    let mut author = String::new();
    let mut date = String::new();
    let mut message = String::new();
    for line in info_str.lines() {
        if let Some(v) = line.strip_prefix("HASH:") {
            hash = v.to_string();
        } else if let Some(v) = line.strip_prefix("SHORT:") {
            short_hash = v.to_string();
        } else if let Some(v) = line.strip_prefix("AUTHOR:") {
            author = v.to_string();
        } else if let Some(v) = line.strip_prefix("DATE:") {
            date = v.to_string();
        } else if let Some(v) = line.strip_prefix("SUBJECT:") {
            message = v.to_string();
        }
    }

    let mut file_statuses: HashMap<String, String> = HashMap::new();
    for line in String::from_utf8_lossy(&ns_out.stdout).lines() {
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        match parts.as_slice() {
            [st, path] => {
                file_statuses.insert(
                    path.to_string(),
                    if st.starts_with('R') {
                        "R".to_string()
                    } else {
                        st.to_string()
                    },
                );
            }
            [st, _old, new_path] => {
                file_statuses.insert(
                    new_path.to_string(),
                    if st.starts_with('R') {
                        "R".to_string()
                    } else {
                        st.to_string()
                    },
                );
            }
            _ => {}
        }
    }

    let mut files = Vec::new();
    let mut total_additions = 0i32;
    let mut total_deletions = 0i32;

    for line in String::from_utf8_lossy(&num_out.stdout).lines() {
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        if parts.len() == 3 {
            let additions: i32 = parts[0].parse().unwrap_or(0);
            let deletions: i32 = parts[1].parse().unwrap_or(0);
            let path = parts[2].to_string();
            total_additions += additions;
            total_deletions += deletions;
            let status = file_statuses
                .get(&path)
                .cloned()
                .unwrap_or_else(|| "M".to_string());
            files.push(GitCommitFile {
                path,
                status,
                additions,
                deletions,
            });
        }
    }

    Ok(GitCommitDetail {
        hash,
        short_hash,
        author,
        date,
        message,
        files,
        total_additions,
        total_deletions,
    })
}

#[tauri::command]
pub async fn git_remote_counts(
    project_path: String,
    branch: Option<String>,
) -> CommandResult<GitRemoteCounts> {
    git_remote_counts_impl(project_path.clone(), branch.clone())
        .await
        .with_context(|| format!("读取 Git 远端计数失败（{}）", project_path))
        .into_command_result()
}

async fn git_remote_counts_impl(
    project_path: String,
    branch: Option<String>,
) -> GitResult<GitRemoteCounts> {
    let branch = if let Some(b) = branch.filter(|s| !s.is_empty()) {
        b
    } else {
        let branch_out = run_git(&project_path, &["rev-parse", "--abbrev-ref", "HEAD"]).await?;
        String::from_utf8_lossy(&branch_out.stdout)
            .trim()
            .to_string()
    };

    let rev_str = format!("{}...@{{u}}", branch);
    let rev_out = run_git(
        &project_path,
        &["rev-list", "--count", "--left-right", &rev_str],
    )
    .await;

    let (ahead, behind) = match rev_out {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout);
            let trimmed = s.trim();
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() == 2 {
                (parts[0].parse().unwrap_or(0), parts[1].parse().unwrap_or(0))
            } else {
                (0, 0)
            }
        }
        _ => (0, 0),
    };

    Ok(GitRemoteCounts {
        ahead,
        behind,
        branch,
    })
}
