use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Output;
use std::time::Duration;

use anyhow::Context;

use crate::project::read_project_config;
use crate::shared::error::{CommandResult, IntoCommandResult};
use crate::shared::truncate_for_display;

type GitResult<T> = std::result::Result<T, GitError>;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("分支名不能为空")]
    RefNameEmpty,
    #[error("分支名过长（{len} 字符），上限 256")]
    RefNameTooLong { len: usize },
    #[error("分支名 `{name}` 包含非法字符，仅允许字母、数字、/、-、_、.、@")]
    RefNameIllegalChars { name: String },
    #[error("分支名 `{name}` 包含非法路径遍历模式")]
    RefNameTraversal { name: String },
    #[error("执行命令失败（cwd={cwd}, args={args:?}）：{source}")]
    CommandIo {
        cwd: PathBuf,
        args: Vec<String>,
        #[source]
        source: std::io::Error,
    },
    #[error("Git 命令线程错误：{0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("Git 命令执行超时（{secs}秒）")]
    Timeout { secs: u64 },
    #[error("Git 命令失败：{0}")]
    CommandFailed(String),
    #[error("没有可用于生成提交信息的已暂存变更。")]
    NoStagedChanges,
    #[error("生成提交信息超时（15秒）")]
    CommitMessageTimeout,
    #[error("智能体执行失败：{0}")]
    AgentFailed(String),
    #[error("智能体返回了空结果。")]
    EmptyAgentResult,
    #[error("读取项目配置失败：{0}")]
    ProjectConfig(String),
}

// ── 辅助函数 ─────────────────────────────────────────────────────────────────

/// Validate a Git ref name (branch, tag, etc.) against a whitelist of safe characters.
fn validate_git_ref_name(name: &str) -> GitResult<()> {
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
async fn run_git<S: AsRef<std::ffi::OsStr>>(project_path: &str, args: &[S]) -> GitResult<Output> {
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
async fn run_git_with_timeout(
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
async fn run_git_check<S: AsRef<std::ffi::OsStr>>(project_path: &str, args: &[S]) -> GitResult<()> {
    let output = run_git(project_path, args).await?;
    if !output.status.success() {
        return Err(GitError::CommandFailed(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(())
}

// ── Tauri 命令 ───────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn generate_commit_message(project_path: String) -> CommandResult<String> {
    generate_commit_message_impl(project_path.clone())
        .await
        .with_context(|| format!("生成提交信息失败（{}）", project_path))
        .into_command_result()
}

async fn generate_commit_message_impl(project_path: String) -> GitResult<String> {
    use std::process::Command;

    // 1. Get staged diff
    let diff_output = run_git(&project_path, &["diff", "--staged"]).await?;
    let diff = String::from_utf8_lossy(&diff_output.stdout).into_owned();
    if diff.trim().is_empty() {
        return Err(GitError::NoStagedChanges);
    }

    // Truncate diff if too large to avoid CLI arg limits
    let diff = truncate_for_display(&diff, 50_000, "...（diff 已截断）");

    // 2. Read project config for prompt and default agent
    let config = read_project_config(project_path.clone()).map_err(GitError::ProjectConfig)?;
    let commit_prompt = config.git.commit_prompt;
    let agent = config.agent.default;

    // 3. Build full prompt
    let full_prompt = format!(
        "{}\n\n以下是 Git diff：\n```diff\n{}\n```\n\n只输出提交信息正文，不要附加解释。",
        commit_prompt, diff
    );

    // 4. Build PATH with common tool locations
    let home = std::env::var("HOME").unwrap_or_default();
    let current_path = std::env::var("PATH").unwrap_or_default();
    let extra_paths = [
        format!("{home}/.local/bin"),
        format!("{home}/.npm-global/bin"),
        "/opt/homebrew/bin".to_string(),
        "/opt/homebrew/sbin".to_string(),
        "/usr/local/bin".to_string(),
        "/usr/bin".to_string(),
        "/bin".to_string(),
        "/usr/sbin".to_string(),
        "/sbin".to_string(),
    ];
    let mut path_parts: Vec<String> = extra_paths.to_vec();
    for p in current_path.split(':') {
        if !p.is_empty() && !path_parts.contains(&p.to_string()) {
            path_parts.push(p.to_string());
        }
    }
    let full_path = path_parts.join(":");

    // 5. Run agent in non-interactive exec mode with 15 second timeout
    let output = tokio::time::timeout(
        Duration::from_secs(15),
        tokio::task::spawn_blocking(move || {
            if agent == "codex" {
                // codex exec runs in non-interactive mode without requiring a TTY
                Command::new("codex")
                    .args(["exec", &full_prompt])
                    .env("PATH", &full_path)
                    .env("HOME", &home)
                    .current_dir(&project_path)
                    .output()
                    .map_err(|source| GitError::CommandIo {
                        cwd: PathBuf::from(&project_path),
                        args: vec!["codex".into(), "exec".into()],
                        source,
                    })
            } else {
                // claude -p runs in non-interactive print mode; prompt is a positional arg
                Command::new("claude")
                    .args(["-p", &full_prompt, "--output-format", "text"])
                    .env("PATH", &full_path)
                    .env("HOME", &home)
                    .current_dir(&project_path)
                    .output()
                    .map_err(|source| GitError::CommandIo {
                        cwd: PathBuf::from(&project_path),
                        args: vec!["claude".into(), "-p".into()],
                        source,
                    })
            }
        }),
    )
    .await
    .map_err(|_| GitError::CommitMessageTimeout)???;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(GitError::AgentFailed(format!("{}{}", stderr, stdout)));
    }

    let result = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if result.is_empty() {
        return Err(GitError::EmptyAgentResult);
    }
    Ok(result)
}

#[derive(serde::Serialize)]
pub(crate) struct GitFileChange {
    path: String,
    status: String,
    staged: bool,
}

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

#[derive(serde::Serialize, Clone)]
pub(crate) struct GitCommit {
    hash: String,
    short_hash: String,
    author: String,
    date: String,
    message: String,
    refs: Vec<String>,
}

#[derive(serde::Serialize)]
pub(crate) struct GitBranchInfo {
    name: String,
    current: bool,
    remote: Option<String>,
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

#[derive(serde::Serialize)]
pub(crate) struct GitCommitFile {
    path: String,
    status: String,
    additions: i32,
    deletions: i32,
}

#[derive(serde::Serialize)]
pub(crate) struct GitCommitDetail {
    hash: String,
    short_hash: String,
    author: String,
    date: String,
    message: String,
    files: Vec<GitCommitFile>,
    total_additions: i32,
    total_deletions: i32,
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
pub async fn git_show_diff(project_path: String, commit_hash: String) -> CommandResult<String> {
    git_show_diff_impl(project_path.clone(), commit_hash.clone())
        .await
        .with_context(|| {
            format!(
                "读取 Git 提交 diff 失败（{}: {}）",
                project_path, commit_hash
            )
        })
        .into_command_result()
}

async fn git_show_diff_impl(project_path: String, commit_hash: String) -> GitResult<String> {
    let args = vec!["show".to_string(), "--format=".to_string(), commit_hash];
    let output = run_git_with_timeout(project_path, args, Duration::from_secs(10)).await?;
    if !output.status.success() {
        return Err(GitError::CommandFailed(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    let raw = output.stdout;
    let limit = 500 * 1024;
    Ok(String::from_utf8_lossy(if raw.len() > limit {
        &raw[..limit]
    } else {
        &raw
    })
    .into_owned())
}

#[tauri::command]
pub async fn git_file_diff(
    project_path: String,
    file_path: String,
    staged: bool,
) -> CommandResult<String> {
    git_file_diff_impl(project_path.clone(), file_path.clone(), staged)
        .await
        .with_context(|| format!("读取 Git 文件 diff 失败（{}: {}）", project_path, file_path))
        .into_command_result()
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
        let fallback_raw = fallback.stdout;
        let limit = 200 * 1024;
        return Ok(String::from_utf8_lossy(if fallback_raw.len() > limit {
            &fallback_raw[..limit]
        } else {
            &fallback_raw
        })
        .into_owned());
    }

    let limit = 200 * 1024;
    Ok(String::from_utf8_lossy(if raw.len() > limit {
        &raw[..limit]
    } else {
        &raw
    })
    .into_owned())
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
pub async fn git_show_file_diff(
    project_path: String,
    commit_hash: String,
    file_path: String,
) -> CommandResult<String> {
    git_show_file_diff_impl(project_path.clone(), commit_hash.clone(), file_path.clone())
        .await
        .with_context(|| {
            format!(
                "读取 Git 提交文件 diff 失败（{}: {} {}）",
                project_path, commit_hash, file_path
            )
        })
        .into_command_result()
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
    let raw = output.stdout;
    let limit = 500 * 1024;
    Ok(String::from_utf8_lossy(if raw.len() > limit {
        &raw[..limit]
    } else {
        &raw
    })
    .into_owned())
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

#[derive(serde::Serialize)]
pub(crate) struct GitRemoteCounts {
    ahead: i32,
    behind: i32,
    branch: String,
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
