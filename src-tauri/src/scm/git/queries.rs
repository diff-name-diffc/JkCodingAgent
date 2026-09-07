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
        let Some((x, y, origin_path, display_path)) = parse_porcelain_fields(line) else {
            continue;
        };

        if x == "?" && y == "?" {
            changes.push(GitFileChange {
                path: display_path,
                status: "?".to_string(),
                staged: false,
                origin_path: None,
            });
        } else {
            if x != " " && x != "?" {
                changes.push(GitFileChange {
                    path: display_path.clone(),
                    status: x.to_string(),
                    staged: true,
                    origin_path: origin_path.clone(),
                });
            }
            if y != " " && y != "?" {
                changes.push(GitFileChange {
                    path: display_path,
                    status: y.to_string(),
                    staged: false,
                    origin_path,
                });
            }
        }
    }
    Ok(changes)
}

/// porcelain v1 单行解析：返回 (暂存列, 工作区列, 重命名原路径, 现路径)。
/// 状态列与分隔空格均为 ASCII，字节切片不会落在多字节字符中间。
fn parse_porcelain_fields(line: &str) -> Option<(&str, &str, Option<String>, String)> {
    if line.len() < 3 {
        return None;
    }
    let x = &line[0..1];
    let y = &line[1..2];
    let raw_path = &line[3..];
    let (origin_path, display_path) = match raw_path.split_once(" -> ") {
        Some((origin, next)) => (Some(origin.to_string()), next.to_string()),
        None => (None, raw_path.to_string()),
    };
    Some((x, y, origin_path, display_path))
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
    skip: Option<u32>,
) -> CommandResult<Vec<GitCommit>> {
    git_log_impl(project_path.clone(), limit, search.clone(), branch.clone(), skip)
        .await
        .with_context(|| format!("读取 Git 日志失败（{}）", project_path))
        .into_command_result()
}

/// 构建 `git log` 的分页数值参数（`-n <limit>` + 可选 `--skip <n>`）。
/// 抽为纯函数供单测：skip 为 None 或 0 时不产生 `--skip`（首页语义）。
fn git_log_paging_args(limit: u32, skip: Option<u32>) -> Vec<String> {
    let mut args: Vec<String> = vec!["-n".into(), limit.to_string()];
    if let Some(s) = skip {
        if s > 0 {
            args.push("--skip".into());
            args.push(s.to_string());
        }
    }
    args
}

async fn git_log_impl(
    project_path: String,
    limit: u32,
    search: Option<String>,
    branch: Option<String>,
    skip: Option<u32>,
) -> GitResult<Vec<GitCommit>> {
    let format = "COMMIT:%H%nSHORT:%h%nAUTHOR:%an%nDATE:%ar%nSUBJECT:%s%nREFS:%D%nEND_RECORD";
    let mut args: Vec<String> = vec!["log".into(), format!("--format={}", format)];
    args.extend(git_log_paging_args(limit, skip));
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

#[cfg(test)]
mod tests {
    use super::{git_log_paging_args, parse_porcelain_fields};

    #[test]
    fn parses_plain_status_line() {
        let (x, y, origin, path) = parse_porcelain_fields("M  src/a.ts").expect("valid line");
        assert_eq!((x, y), ("M", " "));
        assert_eq!(origin, None);
        assert_eq!(path, "src/a.ts");
    }

    #[test]
    fn keeps_rename_origin_path() {
        let (x, y, origin, path) =
            parse_porcelain_fields("R  old-name.ts -> new-name.ts").expect("valid line");
        assert_eq!((x, y), ("R", " "));
        assert_eq!(origin.as_deref(), Some("old-name.ts"));
        assert_eq!(path, "new-name.ts");
    }

    #[test]
    fn parses_untracked_and_worktree_columns() {
        let (x, y, origin, path) = parse_porcelain_fields("?? draft.txt").expect("valid line");
        assert_eq!((x, y), ("?", "?"));
        assert_eq!(origin, None);
        assert_eq!(path, "draft.txt");

        let (x, y, _, path) = parse_porcelain_fields(" M b.ts").expect("valid line");
        assert_eq!((x, y), (" ", "M"));
        assert_eq!(path, "b.ts");
    }

    #[test]
    fn skips_short_and_empty_lines() {
        assert!(parse_porcelain_fields("").is_none());
        assert!(parse_porcelain_fields("M").is_none());
    }

    #[test]
    fn paging_args_omit_skip_on_first_page() {
        // skip 为 None：仅 -n limit（首页）。
        assert_eq!(git_log_paging_args(50, None), vec!["-n", "50"]);
        // skip 为 0：等同首页，不产生 --skip。
        assert_eq!(git_log_paging_args(50, Some(0)), vec!["-n", "50"]);
    }

    #[test]
    fn paging_args_emit_skip_for_later_pages() {
        assert_eq!(
            git_log_paging_args(50, Some(100)),
            vec!["-n", "50", "--skip", "100"]
        );
    }
}
