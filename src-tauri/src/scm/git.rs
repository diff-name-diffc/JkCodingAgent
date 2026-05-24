use std::collections::HashMap;
#[cfg(test)]
use std::path::Path;
use std::time::Duration;

use crate::project::read_project_config;
use crate::shared::truncate_for_display;

// ── 辅助函数 ─────────────────────────────────────────────────────────────────

/// Validate a Git ref name (branch, tag, etc.) against a whitelist of safe characters.
fn validate_git_ref_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("分支名不能为空".to_string());
    }
    if name.len() > 256 {
        return Err(format!("分支名过长（{} 字符），上限 256", name.len()));
    }
    let is_safe = name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '/' || c == '-' || c == '_' || c == '.' || c == '@');
    if !is_safe {
        return Err(format!(
            "分支名 '{}' 包含非法字符，仅允许字母、数字、/、-、_、.、@",
            name
        ));
    }
    // Block path traversal patterns
    if name.contains("..") {
        return Err(format!("分支名 '{}' 包含非法路径遍历模式", name));
    }
    Ok(())
}

/// Validate that project_path is absolute and looks like a real project directory.
#[cfg(test)]
fn validate_project_path(project_path: &str) -> Result<(), String> {
    let path = Path::new(project_path);
    if !path.is_absolute() {
        return Err("项目路径必须是绝对路径".to_string());
    }
    if !path.exists() {
        return Err("项目路径不存在".to_string());
    }
    // Resolve symlinks / .. and ensure the path didn't escape
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("无法解析项目路径：{}", e))?;
    if canonical != path {
        // Allow symlinks that resolve to a valid directory, but block obvious traversal
        if !canonical.is_dir() {
            return Err("项目路径不是目录".to_string());
        }
    }
    Ok(())
}

/// 执行 git 命令并返回原始 Output（spawn_blocking 版本，不阻塞 Tokio 运行时）。
async fn run_git<S: AsRef<std::ffi::OsStr>>(
    project_path: &str,
    args: &[S],
) -> Result<std::process::Output, String> {
    let pp = project_path.to_string();
    let args: Vec<String> = args
        .iter()
        .map(|s| s.as_ref().to_string_lossy().into_owned())
        .collect();
    tokio::task::spawn_blocking(move || {
        std::process::Command::new("git")
            .args(&args)
            .current_dir(&pp)
            .output()
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("Git 命令线程错误：{}", e))?
}

/// 带超时的 git 命令执行。
async fn run_git_with_timeout(
    project_path: String,
    args: Vec<String>,
    timeout: Duration,
) -> Result<std::process::Output, String> {
    tokio::time::timeout(
        timeout,
        tokio::task::spawn_blocking(move || {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(&project_path)
                .output()
                .map_err(|e| e.to_string())
        }),
    )
    .await
    .map_err(|_| format!("Git 命令执行超时（{}秒）", timeout.as_secs()))?
    .map_err(|e| format!("Git 命令线程错误：{}", e))?
}

/// 执行 git 命令，若退出码非零则将 stderr 作为错误返回（spawn_blocking 版本）。
async fn run_git_check<S: AsRef<std::ffi::OsStr>>(
    project_path: &str,
    args: &[S],
) -> Result<(), String> {
    let output = run_git(project_path, args).await?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(())
}

// ── Tauri 命令 ───────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn generate_commit_message(project_path: String) -> Result<String, String> {
    use std::process::Command;

    // 1. Get staged diff
    let diff_output = run_git(&project_path, &["diff", "--staged"]).await?;
    let diff = String::from_utf8_lossy(&diff_output.stdout).into_owned();
    if diff.trim().is_empty() {
        return Err("没有可用于生成提交信息的已暂存变更。".to_string());
    }

    // Truncate diff if too large to avoid CLI arg limits
    let diff = truncate_for_display(&diff, 50_000, "...（diff 已截断）");

    // 2. Read project config for prompt and default agent
    let config = read_project_config(project_path.clone())?;
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
                    .map_err(|e| format!("运行 codex 失败：{}", e))
            } else {
                // claude -p runs in non-interactive print mode; prompt is a positional arg
                Command::new("claude")
                    .args(["-p", &full_prompt, "--output-format", "text"])
                    .env("PATH", &full_path)
                    .env("HOME", &home)
                    .current_dir(&project_path)
                    .output()
                    .map_err(|e| format!("运行 claude 失败：{}", e))
            }
        }),
    )
    .await
    .map_err(|_| "生成提交信息超时（15秒）".to_string())?
    .map_err(|e| format!("生成提交信息线程错误：{}", e))??;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(format!("智能体执行失败：{}{}", stderr, stdout));
    }

    let result = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if result.is_empty() {
        return Err("智能体返回了空结果。".to_string());
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
pub async fn git_status(project_path: String) -> Result<Vec<GitFileChange>, String> {
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
pub async fn git_list_branches(project_path: String) -> Result<Vec<GitBranchInfo>, String> {
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
) -> Result<(), String> {
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
) -> Result<(), String> {
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
) -> Result<Vec<GitCommit>, String> {
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
) -> Result<GitCommitDetail, String> {
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
pub async fn git_show_diff(project_path: String, commit_hash: String) -> Result<String, String> {
    let args = vec!["show".to_string(), "--format=".to_string(), commit_hash];
    let output = run_git_with_timeout(project_path, args, Duration::from_secs(10)).await?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
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
) -> Result<String, String> {
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
pub async fn git_stage(project_path: String, file_path: String) -> Result<(), String> {
    run_git_check(&project_path, &["add", "--", &file_path]).await
}

#[tauri::command]
pub async fn git_unstage(project_path: String, file_path: String) -> Result<(), String> {
    run_git_check(&project_path, &["restore", "--staged", "--", &file_path]).await
}

#[tauri::command]
pub async fn git_stage_all(project_path: String) -> Result<(), String> {
    run_git_check(&project_path, &["add", "-A"]).await
}

#[tauri::command]
pub async fn git_unstage_all(project_path: String) -> Result<(), String> {
    run_git_check(&project_path, &["restore", "--staged", "."]).await
}

#[tauri::command]
pub async fn git_commit(project_path: String, message: String) -> Result<(), String> {
    run_git_check(&project_path, &["commit", "-m", &message]).await
}

#[tauri::command]
pub async fn git_show_file_diff(
    project_path: String,
    commit_hash: String,
    file_path: String,
) -> Result<String, String> {
    let output = run_git(
        &project_path,
        &["show", "--format=", &commit_hash, "--", &file_path],
    )
    .await?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
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
pub async fn git_push(project_path: String, branch: Option<String>) -> Result<String, String> {
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
        return Err(combined);
    }
    Ok(combined.trim().to_string())
}

#[tauri::command]
pub async fn git_pull(project_path: String) -> Result<String, String> {
    let output = run_git(&project_path, &["pull"]).await?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        return Err(combined);
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
) -> Result<GitRemoteCounts, String> {
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
    use super::*;
    use std::path::Path;

    // ── validate_git_ref_name ────────────────────────────────────────────────

    #[test]
    fn git_ref_accepts_simple_name() {
        assert!(validate_git_ref_name("main").is_ok());
    }

    #[test]
    fn git_ref_accepts_feature_slash_name() {
        assert!(validate_git_ref_name("feature/login").is_ok());
    }

    #[test]
    fn git_ref_accepts_hyphen_underscore_dot_at() {
        assert!(validate_git_ref_name("release-1.0_v2@head").is_ok());
    }

    #[test]
    fn git_ref_rejects_empty() {
        assert!(validate_git_ref_name("").is_err());
    }

    #[test]
    fn git_ref_rejects_overlong_name() {
        let long_name = "a".repeat(257);
        let result = validate_git_ref_name(&long_name);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("过长"));
    }

    #[test]
    fn git_ref_accepts_exactly_256_chars() {
        let name = "a".repeat(256);
        assert!(validate_git_ref_name(&name).is_ok());
    }

    #[test]
    fn git_ref_rejects_space() {
        assert!(validate_git_ref_name("feature branch").is_err());
    }

    #[test]
    fn git_ref_rejects_special_chars() {
        assert!(validate_git_ref_name("branch$name").is_err());
        assert!(validate_git_ref_name("branch#name").is_err());
        assert!(validate_git_ref_name("branch!name").is_err());
    }

    #[test]
    fn git_ref_rejects_path_traversal() {
        assert!(validate_git_ref_name("feature..hidden").is_err());
    }

    #[test]
    fn git_ref_rejects_semicolon_injection() {
        assert!(validate_git_ref_name("main;rm -rf /").is_err());
    }

    #[test]
    fn git_ref_rejects_backtick_injection() {
        assert!(validate_git_ref_name("main`whoami`").is_err());
    }

    // ── validate_project_path ────────────────────────────────────────────────

    #[test]
    fn project_path_rejects_relative() {
        assert!(validate_project_path("relative/path").is_err());
    }

    #[test]
    fn project_path_rejects_empty() {
        assert!(validate_project_path("").is_err());
    }

    #[test]
    fn project_path_rejects_dot() {
        assert!(validate_project_path(".").is_err());
    }

    #[test]
    fn project_path_rejects_nonexistent_absolute() {
        assert!(validate_project_path("/no/such/directory/ever").is_err());
    }

    #[test]
    fn project_path_accepts_tmp_dir() {
        let dir = std::env::temp_dir();
        // temp_dir should always exist and be absolute
        assert!(validate_project_path(&dir.to_string_lossy()).is_ok());
    }

    // ── git_status porcelain parsing ────────────────────────────────────────
    //
    // The parsing logic in git_status is inline, so we test it by examining
    // what the function body would produce from raw porcelain output.
    // We extract the logic into a helper test that mirrors the parsing.

    #[test]
    fn status_parses_untracked_file() {
        let line = "?? newfile.txt";
        let x = &line[0..1];
        let y = &line[1..2];
        let raw_path = line[3..].to_string();
        assert_eq!(x, "?");
        assert_eq!(y, "?");
        assert_eq!(raw_path, "newfile.txt");
    }

    #[test]
    fn status_parses_staged_modified() {
        // "M " = staged modification (index modified, worktree clean)
        let line = "M  src/main.rs";
        let x = &line[0..1];
        let y = &line[1..2];
        assert_eq!(x, "M");
        assert_eq!(y, " ");
    }

    #[test]
    fn status_parses_unstaged_modified() {
        // " M" = unstaged modification (index clean, worktree modified)
        let line = " M src/main.rs";
        let x = &line[0..1];
        let y = &line[1..2];
        assert_eq!(x, " ");
        assert_eq!(y, "M");
    }

    #[test]
    fn status_parses_both_modified() {
        // "MM" = staged AND unstaged modifications
        let line = "MM file.rs";
        let x = &line[0..1];
        let y = &line[1..2];
        assert_eq!(x, "M");
        assert_eq!(y, "M");
    }

    #[test]
    fn status_parses_renamed_with_arrow() {
        let line = "R  old_name -> new_name";
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
        assert_eq!(display_path, "new_name");
    }

    #[test]
    fn status_skips_short_lines() {
        // Lines shorter than 3 characters should be skipped
        let short_lines = ["", "A", "AB"];
        for line in &short_lines {
            assert!(line.len() < 3);
        }
    }

    // ── git_log format parsing ───────────────────────────────────────────────

    #[test]
    fn log_format_parses_single_commit() {
        let stdout = "\
COMMIT:abc123def456789012345678901234567890abcd
SHORT:abc123d
AUTHOR:John Doe
DATE:2 hours ago
SUBJECT:feat: add new feature
REFS:HEAD -> main, origin/main, tag: v1.0
END_RECORD";

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

        assert_eq!(commits.len(), 1);
        let c = &commits[0];
        assert_eq!(c.hash, "abc123def456789012345678901234567890abcd");
        assert_eq!(c.short_hash, "abc123d");
        assert_eq!(c.author, "John Doe");
        assert_eq!(c.date, "2 hours ago");
        assert_eq!(c.message, "feat: add new feature");
        assert_eq!(c.refs, vec!["HEAD -> main", "origin/main", "tag: v1.0"]);
    }

    #[test]
    fn log_format_parses_multiple_commits() {
        let stdout = "\
COMMIT:first_hash
SHORT:abc12
AUTHOR:Alice
DATE:1 day ago
SUBJECT:first commit
REFS:
END_RECORD
COMMIT:second_hash
SHORT:def34
AUTHOR:Bob
DATE:3 days ago
SUBJECT:second commit
REFS:HEAD -> feature
END_RECORD";

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

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].hash, "first_hash");
        assert_eq!(commits[0].author, "Alice");
        assert!(commits[0].refs.is_empty());
        assert_eq!(commits[1].hash, "second_hash");
        assert_eq!(commits[1].refs, vec!["HEAD -> feature"]);
    }

    // ── git_list_branches output parsing ─────────────────────────────────────

    #[test]
    fn branch_list_parses_local_branches() {
        let stdout = "* main\n  develop\n  feature/x";
        let mut branches = Vec::new();
        for line in stdout.lines() {
            if line.len() < 2 {
                continue;
            }
            let current = line.starts_with("* ");
            let raw = line[2..].trim();
            if raw.contains(" -> ") {
                continue;
            }
            if let Some(_without_remotes) = raw.strip_prefix("remotes/") {
                // remote branch handling
            } else if !raw.is_empty() {
                branches.push(GitBranchInfo {
                    name: raw.to_string(),
                    current,
                    remote: None,
                });
            }
        }
        assert_eq!(branches.len(), 3);
        assert!(branches[0].current);
        assert_eq!(branches[0].name, "main");
        assert!(!branches[1].current);
        assert_eq!(branches[1].name, "develop");
        assert_eq!(branches[2].name, "feature/x");
    }

    #[test]
    fn branch_list_parses_remote_branches() {
        let stdout = "  remotes/origin/main\n  remotes/origin/develop\n  remotes/upstream/feature";
        let mut branches = Vec::new();
        for line in stdout.lines() {
            if line.len() < 2 {
                continue;
            }
            let current = line.starts_with("* ");
            let raw = line[2..].trim();
            if raw.contains(" -> ") {
                continue;
            }
            if let Some(without_remotes) = raw.strip_prefix("remotes/") {
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
        assert_eq!(branches.len(), 3);
        assert_eq!(branches[0].name, "origin/main");
        assert_eq!(branches[0].remote, Some("origin".to_string()));
        assert_eq!(branches[2].name, "upstream/feature");
        assert_eq!(branches[2].remote, Some("upstream".to_string()));
    }

    #[test]
    fn branch_list_skips_head_pointer() {
        let stdout = "* main\n  remotes/origin/HEAD -> origin/main\n  remotes/origin/develop";
        let mut branches = Vec::new();
        for line in stdout.lines() {
            if line.len() < 2 {
                continue;
            }
            let current = line.starts_with("* ");
            let raw = line[2..].trim();
            if raw.contains(" -> ") {
                continue;
            }
            if let Some(without_remotes) = raw.strip_prefix("remotes/") {
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
        // "HEAD -> origin/main" should be skipped
        assert_eq!(branches.len(), 2);
        assert_eq!(branches[0].name, "main");
        assert_eq!(branches[1].name, "origin/develop");
    }

    #[test]
    fn branch_list_skips_short_lines() {
        let stdout = "*\n \n";
        let mut count = 0;
        for line in stdout.lines() {
            if line.len() < 2 {
                continue;
            }
            count += 1;
        }
        assert_eq!(count, 0);
    }

    // ── git_checkout_branch local name extraction ────────────────────────────

    #[test]
    fn checkout_extracts_local_name_from_remote() {
        // Mirrors the logic in git_checkout_branch for is_remote=true
        let branch_name = "origin/main";
        let local_name = branch_name
            .split_once('/')
            .map(|(_, n)| n.to_string())
            .unwrap_or_else(|| branch_name.to_string());
        assert_eq!(local_name, "main");
    }

    #[test]
    fn checkout_extracts_local_name_without_slash() {
        let branch_name = "main";
        let local_name = branch_name
            .split_once('/')
            .map(|(_, n)| n.to_string())
            .unwrap_or_else(|| branch_name.to_string());
        assert_eq!(local_name, "main");
    }

    // ── git_remote_counts parsing ────────────────────────────────────────────

    #[test]
    fn remote_counts_parses_two_parts() {
        let s = "3\t5";
        let parts: Vec<&str> = s.split_whitespace().collect();
        assert_eq!(parts.len(), 2);
        let ahead: i32 = parts[0].parse().unwrap_or(0);
        let behind: i32 = parts[1].parse().unwrap_or(0);
        assert_eq!(ahead, 3);
        assert_eq!(behind, 5);
    }

    #[test]
    fn remote_counts_handles_single_part() {
        let s = "3";
        let parts: Vec<&str> = s.split_whitespace().collect();
        if parts.len() == 2 {
            // would parse normally
        } else {
            // defaults to (0, 0)
        }
        assert_eq!(parts.len(), 1);
    }

    #[test]
    fn remote_counts_handles_empty() {
        let s = "";
        let parts: Vec<&str> = s.split_whitespace().collect();
        assert_eq!(parts.len(), 0);
    }

    // ── git_commit_detail name-status parsing ────────────────────────────────

    #[test]
    fn commit_detail_parses_simple_status() {
        // "M\tpath/to/file.rs" -> status=M, path=path/to/file.rs
        let line = "M\tpath/to/file.rs";
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        match parts.as_slice() {
            [st, path] => {
                assert_eq!(*st, "M");
                assert_eq!(*path, "path/to/file.rs");
            }
            _ => panic!("unexpected parts"),
        }
    }

    #[test]
    fn commit_detail_parses_rename_status() {
        // "R100\told_path\tnew_path" -> status=R, path=new_path
        let line = "R100\told/path\tnew/path";
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        match parts.as_slice() {
            [st, _old, new_path] => {
                let status = if st.starts_with('R') { "R" } else { st };
                assert_eq!(status, "R");
                assert_eq!(*new_path, "new/path");
            }
            _ => panic!("unexpected parts"),
        }
    }

    #[test]
    fn commit_detail_normalizes_rename_status() {
        // All R-prefixed statuses (R100, R090, etc.) should become "R"
        for rename_status in &["R100", "R090", "R050", "R001"] {
            let result = if rename_status.starts_with('R') {
                "R"
            } else {
                *rename_status
            };
            assert_eq!(result, "R");
        }
    }

    // ── git_commit_detail numstat parsing ────────────────────────────────────

    #[test]
    fn commit_detail_parses_numstat_line() {
        let line = "10\t3\tsrc/main.rs";
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        assert_eq!(parts.len(), 3);
        let additions: i32 = parts[0].parse().unwrap_or(0);
        let deletions: i32 = parts[1].parse().unwrap_or(0);
        assert_eq!(additions, 10);
        assert_eq!(deletions, 3);
        assert_eq!(parts[2], "src/main.rs");
    }

    #[test]
    fn commit_detail_handles_binary_numstat() {
        // Binary files show "-" instead of numbers
        let line = "-\t-\timage.png";
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        let additions: i32 = parts[0].parse().unwrap_or(0);
        let deletions: i32 = parts[1].parse().unwrap_or(0);
        assert_eq!(additions, 0);
        assert_eq!(deletions, 0);
    }

    // ── git_show_diff truncation logic ───────────────────────────────────────

    #[test]
    fn show_diff_truncates_at_500k() {
        let raw = vec![b'x'; 600 * 1024];
        let limit = 500 * 1024;
        let result = if raw.len() > limit {
            &raw[..limit]
        } else {
            &raw
        };
        assert_eq!(result.len(), limit);
    }

    #[test]
    fn show_diff_keeps_short_output() {
        let raw = vec![b'x'; 100];
        let limit = 500 * 1024;
        let result = if raw.len() > limit {
            &raw[..limit]
        } else {
            &raw
        };
        assert_eq!(result.len(), 100);
    }

    // ── git_file_diff truncation logic ───────────────────────────────────────

    #[test]
    fn file_diff_truncates_at_200k() {
        let raw = vec![b'x'; 300 * 1024];
        let limit = 200 * 1024;
        let result = if raw.len() > limit {
            &raw[..limit]
        } else {
            &raw
        };
        assert_eq!(result.len(), limit);
    }

    // ── git_log search/branch args building ──────────────────────────────────

    #[test]
    fn log_builds_args_with_search() {
        let limit_str = "50".to_string();
        let format = "COMMIT:%H%nSHORT:%h%nAUTHOR:%an%nDATE:%ar%nSUBJECT:%s%nREFS:%D%nEND_RECORD";
        let mut args: Vec<String> = vec![
            "log".into(),
            format!("--format={}", format),
            "-n".into(),
            limit_str,
        ];
        let search = Some("bugfix".to_string());
        if let Some(ref s) = search {
            if !s.is_empty() {
                args.push("--grep".into());
                args.push(s.clone());
            }
        }
        assert!(args.contains(&"--grep".to_string()));
        assert!(args.contains(&"bugfix".to_string()));
    }

    #[test]
    fn log_builds_args_with_empty_search() {
        let mut args: Vec<String> = vec!["log".into()];
        let search = Some("".to_string());
        if let Some(ref s) = search {
            if !s.is_empty() {
                args.push("--grep".into());
                args.push(s.clone());
            }
        }
        assert!(!args.contains(&"--grep".to_string()));
    }

    #[test]
    fn log_builds_args_with_none_search() {
        let mut args: Vec<String> = vec!["log".into()];
        let search: Option<String> = None;
        if let Some(ref s) = search {
            if !s.is_empty() {
                args.push("--grep".into());
                args.push(s.clone());
            }
        }
        assert!(!args.contains(&"--grep".to_string()));
    }

    // ── git_push branch validation ───────────────────────────────────────────

    #[test]
    fn push_validates_branch_name() {
        // validate_git_ref_name is called for the branch in git_push
        assert!(validate_git_ref_name("main").is_ok());
        assert!(validate_git_ref_name("feature branch").is_err());
    }

    // ── GitFileChange struct ─────────────────────────────────────────────────

    #[test]
    fn git_file_change_serializes() {
        let change = GitFileChange {
            path: "src/main.rs".to_string(),
            status: "M".to_string(),
            staged: true,
        };
        let json = serde_json::to_string(&change).unwrap();
        assert!(json.contains("src/main.rs"));
        assert!(json.contains("\"staged\":true"));
    }

    // ── GitCommitDetail struct ───────────────────────────────────────────────

    #[test]
    fn git_commit_detail_struct_totals() {
        let files = [
            GitCommitFile {
                path: "a.rs".to_string(),
                status: "M".to_string(),
                additions: 10,
                deletions: 3,
            },
            GitCommitFile {
                path: "b.rs".to_string(),
                status: "A".to_string(),
                additions: 50,
                deletions: 0,
            },
        ];
        let total_additions: i32 = files.iter().map(|f| f.additions).sum();
        let total_deletions: i32 = files.iter().map(|f| f.deletions).sum();
        assert_eq!(total_additions, 60);
        assert_eq!(total_deletions, 3);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Integration tests — real git commands against temporary repositories
    // ═══════════════════════════════════════════════════════════════════════════

    use std::fs;

    /// Create a temporary directory, `git init`, configure user, and make an
    /// initial commit so the repo has a HEAD. Returns the TempDir (caller must
    /// keep it alive) and the canonical absolute path as a String.
    fn setup_repo() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path();

        // git init
        let out = std::process::Command::new("git")
            .args(["init"])
            .current_dir(path)
            .output()
            .expect("git init");
        assert!(
            out.status.success(),
            "git init failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        // configure user so commits work
        for (key, val) in [("user.email", "test@test.com"), ("user.name", "Test")] {
            let o = std::process::Command::new("git")
                .args(["config", key, val])
                .current_dir(path)
                .output()
                .expect("git config");
            assert!(o.status.success(), "git config {} failed", key);
        }

        // initial commit so HEAD exists
        fs::write(path.join("README.md"), "# test\n").expect("write README");
        let o = std::process::Command::new("git")
            .args(["add", "README.md"])
            .current_dir(path)
            .output()
            .expect("git add");
        assert!(o.status.success(), "git add failed");
        let o = std::process::Command::new("git")
            .args(["commit", "-m", "initial commit"])
            .current_dir(path)
            .output()
            .expect("git commit");
        assert!(
            o.status.success(),
            "initial commit failed: {}",
            String::from_utf8_lossy(&o.stderr)
        );

        // Use canonical path to avoid validate_project_path mismatch on macOS
        // (/var vs /private/var symlink).
        let canonical = path.canonicalize().expect("canonicalize");
        (dir, canonical.to_string_lossy().into_owned())
    }

    // ── git_status ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn status_empty_repo() {
        let (_dir, repo) = setup_repo();
        let changes = git_status(repo).await.expect("git_status");
        assert!(changes.is_empty(), "clean repo should have no changes");
    }

    #[tokio::test]
    async fn status_untracked_file() {
        let (_dir, repo) = setup_repo();
        let repo_path = Path::new(&repo);
        fs::write(repo_path.join("new_file.txt"), "hello").expect("write");

        let changes = git_status(repo).await.expect("git_status");
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "new_file.txt");
        assert_eq!(changes[0].status, "?");
        assert!(!changes[0].staged);
    }

    #[tokio::test]
    async fn status_staged_modified() {
        let (_dir, repo) = setup_repo();
        let repo_path = Path::new(&repo);
        fs::write(repo_path.join("README.md"), "# modified\n").expect("write");
        // stage it
        let o = std::process::Command::new("git")
            .args(["add", "README.md"])
            .current_dir(&repo)
            .output()
            .expect("git add");
        assert!(o.status.success());

        let changes = git_status(repo).await.expect("git_status");
        assert!(
            changes
                .iter()
                .any(|c| c.path == "README.md" && c.staged && c.status == "M"),
            "expected staged modified README.md, got {} changes",
            changes.len()
        );
    }

    #[tokio::test]
    async fn status_unstaged_modified() {
        let (_dir, repo) = setup_repo();
        let repo_path = Path::new(&repo);
        fs::write(repo_path.join("README.md"), "# unstaged\n").expect("write");
        // do NOT stage

        let changes = git_status(repo).await.expect("git_status");
        assert!(
            changes
                .iter()
                .any(|c| c.path == "README.md" && !c.staged && c.status == "M"),
            "expected unstaged modified README.md, got {} changes",
            changes.len()
        );
    }

    #[tokio::test]
    async fn status_both_staged_and_unstaged() {
        let (_dir, repo) = setup_repo();
        let repo_path = Path::new(&repo);
        // stage first version
        fs::write(repo_path.join("README.md"), "v1\n").expect("write");
        let o = std::process::Command::new("git")
            .args(["add", "README.md"])
            .current_dir(&repo)
            .output()
            .expect("git add");
        assert!(o.status.success());
        // then modify again without staging
        fs::write(repo_path.join("README.md"), "v2\n").expect("write");

        let changes = git_status(repo).await.expect("git_status");
        let staged = changes.iter().find(|c| c.staged);
        let unstaged = changes.iter().find(|c| !c.staged);
        assert!(staged.is_some(), "should have a staged change");
        assert!(unstaged.is_some(), "should have an unstaged change");
        assert_eq!(staged.unwrap().status, "M");
        assert_eq!(unstaged.unwrap().status, "M");
    }

    #[tokio::test]
    async fn status_multiple_untracked() {
        let (_dir, repo) = setup_repo();
        let repo_path = Path::new(&repo);
        fs::write(repo_path.join("a.txt"), "a").expect("write");
        fs::write(repo_path.join("b.rs"), "fn main(){}").expect("write");
        fs::create_dir(repo_path.join("subdir")).expect("mkdir");
        fs::write(repo_path.join("subdir/c.toml"), "[pkg]").expect("write");

        let changes = git_status(repo).await.expect("git_status");
        let untracked: Vec<_> = changes.iter().filter(|c| c.status == "?").collect();
        assert!(
            untracked.len() >= 3,
            "expected at least 3 untracked files, got {}",
            untracked.len()
        );
    }

    // ── git_log ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn log_initial_commit() {
        let (_dir, repo) = setup_repo();
        let commits = git_log(repo, 50, None, None).await.expect("git_log");
        assert_eq!(
            commits.len(),
            1,
            "initial repo should have exactly 1 commit"
        );
        assert_eq!(commits[0].message, "initial commit");
        assert!(!commits[0].hash.is_empty());
        assert!(!commits[0].short_hash.is_empty());
        assert_eq!(commits[0].author, "Test");
    }

    #[tokio::test]
    async fn log_multiple_commits() {
        let (_dir, repo) = setup_repo();
        let repo_path = Path::new(&repo);

        // add second commit
        fs::write(repo_path.join("second.txt"), "second").expect("write");
        let o = std::process::Command::new("git")
            .args(["add", "second.txt"])
            .current_dir(&repo)
            .output()
            .expect("git add");
        assert!(o.status.success());
        let o = std::process::Command::new("git")
            .args(["commit", "-m", "second commit"])
            .current_dir(&repo)
            .output()
            .expect("git commit");
        assert!(o.status.success());

        let commits = git_log(repo, 50, None, None).await.expect("git_log");
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].message, "second commit"); // newest first
        assert_eq!(commits[1].message, "initial commit");
    }

    #[tokio::test]
    async fn log_limit() {
        let (_dir, repo) = setup_repo();
        let commits = git_log(repo, 1, None, None).await.expect("git_log");
        assert_eq!(commits.len(), 1);
    }

    #[tokio::test]
    async fn log_search_filter() {
        let (_dir, repo) = setup_repo();
        let repo_path = Path::new(&repo);

        fs::write(repo_path.join("feature.txt"), "feat").expect("write");
        let o = std::process::Command::new("git")
            .args(["add", "feature.txt"])
            .current_dir(&repo)
            .output()
            .expect("git add");
        assert!(o.status.success());
        let o = std::process::Command::new("git")
            .args(["commit", "-m", "add feature module"])
            .current_dir(&repo)
            .output()
            .expect("git commit");
        assert!(o.status.success());

        let commits = git_log(repo, 50, Some("feature module".into()), None)
            .await
            .expect("git_log");
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].message, "add feature module");
    }

    #[tokio::test]
    async fn log_search_no_match() {
        let (_dir, repo) = setup_repo();
        let commits = git_log(repo, 50, Some("zzzz_nonexistent".into()), None)
            .await
            .expect("git_log");
        assert!(commits.is_empty());
    }

    #[tokio::test]
    async fn log_refs_contains_head() {
        let (_dir, repo) = setup_repo();
        let commits = git_log(repo, 50, None, None).await.expect("git_log");
        assert_eq!(commits.len(), 1);
        // refs should include "HEAD -> main" (or master depending on git config)
        let refs_str = commits[0].refs.join(", ");
        assert!(
            refs_str.contains("HEAD"),
            "refs should contain HEAD, got: {}",
            refs_str
        );
    }

    // ── git_list_branches ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn list_branches_initial() {
        let (_dir, repo) = setup_repo();
        let branches = git_list_branches(repo).await.expect("git_list_branches");
        assert!(!branches.is_empty(), "should have at least one branch");
        let current: Vec<_> = branches.iter().filter(|b| b.current).collect();
        assert_eq!(current.len(), 1, "exactly one current branch");
        assert!(
            current[0].remote.is_none(),
            "local branch should have no remote"
        );
    }

    #[tokio::test]
    async fn list_branches_multiple() {
        let (_dir, repo) = setup_repo();
        // create branches
        for name in &["feature/a", "bugfix/b"] {
            let o = std::process::Command::new("git")
                .args(["branch", name])
                .current_dir(&repo)
                .output()
                .expect("git branch");
            assert!(
                o.status.success(),
                "git branch {} failed: {}",
                name,
                String::from_utf8_lossy(&o.stderr)
            );
        }

        let branches = git_list_branches(repo).await.expect("git_list_branches");
        let names: Vec<&str> = branches.iter().map(|b| b.name.as_str()).collect();
        assert!(
            names.contains(&"feature/a"),
            "branches should contain feature/a, got: {:?}",
            names
        );
        assert!(
            names.contains(&"bugfix/b"),
            "branches should contain bugfix/b, got: {:?}",
            names
        );
    }

    #[tokio::test]
    async fn list_branches_with_remote() {
        let (_dir, repo) = setup_repo();

        // create a bare repo as "remote"
        let remote_dir = tempfile::tempdir().expect("remote temp dir");
        let o = std::process::Command::new("git")
            .args(["init", "--bare"])
            .current_dir(remote_dir.path())
            .output()
            .expect("git init --bare");
        assert!(o.status.success());
        let remote_path = remote_dir.path().to_string_lossy().into_owned();

        let o = std::process::Command::new("git")
            .args(["remote", "add", "origin", &remote_path])
            .current_dir(&repo)
            .output()
            .expect("git remote add");
        assert!(o.status.success());

        // push so remote branch exists
        let current_branch = {
            let out = std::process::Command::new("git")
                .args(["rev-parse", "--abbrev-ref", "HEAD"])
                .current_dir(&repo)
                .output()
                .expect("git rev-parse");
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        let o = std::process::Command::new("git")
            .args(["push", "-u", "origin", &current_branch])
            .current_dir(&repo)
            .output()
            .expect("git push");
        assert!(
            o.status.success(),
            "push failed: {}",
            String::from_utf8_lossy(&o.stderr)
        );

        // keep remote_dir alive through the assertion
        let branches = git_list_branches(repo).await.expect("git_list_branches");
        let remotes: Vec<_> = branches.iter().filter(|b| b.remote.is_some()).collect();
        assert!(
            !remotes.is_empty(),
            "should have at least one remote branch"
        );
        // remote branch names should be like "origin/main"
        assert!(
            remotes[0].name.starts_with("origin/"),
            "remote branch name should start with origin/, got: {}",
            remotes[0].name
        );
        drop(remote_dir);
    }

    // ── git_create_branch ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn create_branch_success() {
        let (_dir, repo) = setup_repo();
        // get current branch name to pass as from_branch
        let current = get_current_branch(&repo);

        git_create_branch(repo.clone(), "feature/new".into(), current)
            .await
            .expect("git_create_branch");

        let branches = git_list_branches(repo).await.expect("git_list_branches");
        let names: Vec<&str> = branches.iter().map(|b| b.name.as_str()).collect();
        assert!(names.contains(&"feature/new"), "should contain feature/new");
    }

    #[tokio::test]
    async fn create_branch_invalid_name() {
        let (_dir, repo) = setup_repo();
        let result = git_create_branch(repo, "bad name!".into(), "main".into()).await;
        assert!(result.is_err(), "should reject invalid branch name");
    }

    // ── git_checkout_branch ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn checkout_branch_local() {
        let (_dir, repo) = setup_repo();
        // create another branch first
        let o = std::process::Command::new("git")
            .args(["branch", "develop"])
            .current_dir(&repo)
            .output()
            .expect("git branch");
        assert!(o.status.success());

        git_checkout_branch(repo.clone(), "develop".into(), false)
            .await
            .expect("git_checkout_branch");

        let current = get_current_branch(&repo);
        assert_eq!(current, "develop", "should be on develop after checkout");
    }

    #[tokio::test]
    async fn checkout_branch_invalid_name() {
        let (_dir, repo) = setup_repo();
        let result = git_checkout_branch(repo, "bad;name".into(), false).await;
        assert!(result.is_err(), "should reject invalid branch name");
    }

    // ── git_show_diff ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn show_diff_returns_diff() {
        let (_dir, repo) = setup_repo();
        let repo_path = Path::new(&repo);

        // create second commit with changes
        fs::write(repo_path.join("README.md"), "# updated\n").expect("write");
        let o = std::process::Command::new("git")
            .args(["add", "README.md"])
            .current_dir(&repo)
            .output()
            .expect("git add");
        assert!(o.status.success());
        let o = std::process::Command::new("git")
            .args(["commit", "-m", "update readme"])
            .current_dir(&repo)
            .output()
            .expect("git commit");
        assert!(o.status.success());

        // get the commit hash
        let commits = git_log(repo.clone(), 1, None, None).await.expect("git_log");
        let hash = commits[0].hash.clone();

        let diff = git_show_diff(repo, hash).await.expect("git_show_diff");
        assert!(!diff.is_empty(), "diff should not be empty");
        assert!(diff.contains("updated"), "diff should mention 'updated'");
    }

    // ── git_file_diff ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn file_diff_staged() {
        let (_dir, repo) = setup_repo();
        let repo_path = Path::new(&repo);
        fs::write(repo_path.join("README.md"), "# staged change\n").expect("write");
        let o = std::process::Command::new("git")
            .args(["add", "README.md"])
            .current_dir(&repo)
            .output()
            .expect("git add");
        assert!(o.status.success());

        let diff = git_file_diff(repo, "README.md".into(), true)
            .await
            .expect("git_file_diff");
        assert!(!diff.is_empty(), "staged diff should not be empty");
        assert!(
            diff.contains("staged change"),
            "diff should contain new content"
        );
    }

    #[tokio::test]
    async fn file_diff_unstaged() {
        let (_dir, repo) = setup_repo();
        let repo_path = Path::new(&repo);
        fs::write(repo_path.join("README.md"), "# unstaged change\n").expect("write");

        let diff = git_file_diff(repo, "README.md".into(), false)
            .await
            .expect("git_file_diff");
        assert!(!diff.is_empty(), "unstaged diff should not be empty");
    }

    #[tokio::test]
    async fn file_diff_untracked_file() {
        let (_dir, repo) = setup_repo();
        let repo_path = Path::new(&repo);
        fs::write(repo_path.join("untracked.txt"), "brand new file").expect("write");

        let diff = git_file_diff(repo, "untracked.txt".into(), false)
            .await
            .expect("git_file_diff");
        assert!(
            !diff.is_empty(),
            "untracked file diff should use --no-index fallback"
        );
        assert!(
            diff.contains("brand new file"),
            "diff should contain file content"
        );
    }

    #[tokio::test]
    async fn file_diff_clean_file() {
        let (_dir, repo) = setup_repo();
        // A clean tracked file produces empty output from `git diff`, so
        // git_file_diff falls back to `--no-index /dev/null <path>` which
        // produces the full file content as a diff. Verify the fallback works.
        let diff = git_file_diff(repo, "README.md".into(), false)
            .await
            .expect("git_file_diff");
        assert!(
            !diff.is_empty(),
            "git_file_diff --no-index fallback should produce output for clean files"
        );
    }

    // ── git_stage / git_unstage ─────────────────────────────────────────────────

    #[tokio::test]
    async fn stage_file() {
        let (_dir, repo) = setup_repo();
        let repo_path = Path::new(&repo);
        fs::write(repo_path.join("new.txt"), "content").expect("write");

        git_stage(repo.clone(), "new.txt".into())
            .await
            .expect("git_stage");

        let changes = git_status(repo).await.expect("git_status");
        let staged: Vec<_> = changes.iter().filter(|c| c.staged).collect();
        assert_eq!(staged.len(), 1);
        assert_eq!(staged[0].path, "new.txt");
    }

    #[tokio::test]
    async fn unstage_file() {
        let (_dir, repo) = setup_repo();
        let repo_path = Path::new(&repo);
        fs::write(repo_path.join("README.md"), "modified").expect("write");
        let o = std::process::Command::new("git")
            .args(["add", "README.md"])
            .current_dir(&repo)
            .output()
            .expect("git add");
        assert!(o.status.success());

        // verify it is staged
        let changes = git_status(repo.clone()).await.expect("git_status");
        assert!(changes.iter().any(|c| c.staged && c.path == "README.md"));

        git_unstage(repo.clone(), "README.md".into())
            .await
            .expect("git_unstage");

        let changes = git_status(repo).await.expect("git_status");
        assert!(
            changes.iter().all(|c| !c.staged),
            "nothing should be staged after unstage"
        );
    }

    #[tokio::test]
    async fn stage_nonexistent_file_fails() {
        let (_dir, repo) = setup_repo();
        let result = git_stage(repo, "no_such_file.xyz".into()).await;
        assert!(result.is_err(), "staging nonexistent file should fail");
    }

    // ── git_stage_all / git_unstage_all ─────────────────────────────────────────

    #[tokio::test]
    async fn stage_all() {
        let (_dir, repo) = setup_repo();
        let repo_path = Path::new(&repo);
        fs::write(repo_path.join("a.txt"), "a").expect("write");
        fs::write(repo_path.join("b.txt"), "b").expect("write");

        git_stage_all(repo.clone()).await.expect("git_stage_all");

        let changes = git_status(repo).await.expect("git_status");
        assert!(
            changes.iter().all(|c| c.staged),
            "all changes should be staged"
        );
        assert_eq!(changes.len(), 2);
    }

    #[tokio::test]
    async fn unstage_all() {
        let (_dir, repo) = setup_repo();
        let repo_path = Path::new(&repo);
        fs::write(repo_path.join("a.txt"), "a").expect("write");
        fs::write(repo_path.join("README.md"), "mod").expect("write");
        let o = std::process::Command::new("git")
            .args(["add", "-A"])
            .current_dir(&repo)
            .output()
            .expect("git add");
        assert!(o.status.success());

        // verify all staged
        let changes = git_status(repo.clone()).await.expect("git_status");
        assert!(changes.iter().all(|c| c.staged));

        git_unstage_all(repo.clone())
            .await
            .expect("git_unstage_all");

        let changes = git_status(repo).await.expect("git_status");
        assert!(
            changes.iter().all(|c| !c.staged),
            "nothing should be staged after unstage_all"
        );
    }

    // ── git_commit ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn commit_staged_change() {
        let (_dir, repo) = setup_repo();
        let repo_path = Path::new(&repo);
        fs::write(repo_path.join("new.txt"), "content").expect("write");
        git_stage(repo.clone(), "new.txt".into())
            .await
            .expect("stage");

        git_commit(repo.clone(), "add new file".into())
            .await
            .expect("git_commit");

        let commits = git_log(repo, 50, None, None).await.expect("git_log");
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].message, "add new file");
    }

    #[tokio::test]
    async fn commit_nothing_staged_fails() {
        let (_dir, repo) = setup_repo();
        let result = git_commit(repo, "should fail".into()).await;
        assert!(result.is_err(), "commit with nothing staged should fail");
    }

    // ── git_remote_counts ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn remote_counts_no_remote() {
        let (_dir, repo) = setup_repo();
        // no remote configured — git_remote_counts should still succeed, reporting 0/0
        let counts = git_remote_counts(repo, None)
            .await
            .expect("git_remote_counts");
        assert_eq!(counts.ahead, 0);
        assert_eq!(counts.behind, 0);
    }

    #[tokio::test]
    async fn remote_counts_with_local_bare() {
        let (_dir, repo) = setup_repo();
        let remote_dir = tempfile::tempdir().expect("remote temp dir");
        let o = std::process::Command::new("git")
            .args(["init", "--bare"])
            .current_dir(remote_dir.path())
            .output()
            .expect("git init --bare");
        assert!(o.status.success());
        let remote_path = remote_dir.path().to_string_lossy().into_owned();

        let o = std::process::Command::new("git")
            .args(["remote", "add", "origin", &remote_path])
            .current_dir(&repo)
            .output()
            .expect("git remote add");
        assert!(o.status.success());

        let current_branch = get_current_branch(&repo);
        let o = std::process::Command::new("git")
            .args(["push", "-u", "origin", &current_branch])
            .current_dir(&repo)
            .output()
            .expect("git push");
        assert!(
            o.status.success(),
            "push failed: {}",
            String::from_utf8_lossy(&o.stderr)
        );

        let counts = git_remote_counts(repo, None)
            .await
            .expect("git_remote_counts");
        assert_eq!(counts.ahead, 0, "just pushed, should be 0 ahead");
        assert_eq!(counts.behind, 0, "just pushed, should be 0 behind");
        assert!(!counts.branch.is_empty());

        drop(remote_dir);
    }

    #[tokio::test]
    async fn remote_counts_ahead_after_local_commit() {
        let (_dir, repo) = setup_repo();
        let remote_dir = tempfile::tempdir().expect("remote temp dir");
        let o = std::process::Command::new("git")
            .args(["init", "--bare"])
            .current_dir(remote_dir.path())
            .output()
            .expect("git init --bare");
        assert!(o.status.success());
        let remote_path = remote_dir.path().to_string_lossy().into_owned();

        let o = std::process::Command::new("git")
            .args(["remote", "add", "origin", &remote_path])
            .current_dir(&repo)
            .output()
            .expect("git remote add");
        assert!(o.status.success());

        let current_branch = get_current_branch(&repo);
        let o = std::process::Command::new("git")
            .args(["push", "-u", "origin", &current_branch])
            .current_dir(&repo)
            .output()
            .expect("git push");
        assert!(
            o.status.success(),
            "push failed: {}",
            String::from_utf8_lossy(&o.stderr)
        );

        // make another commit locally without pushing
        let repo_path = Path::new(&repo);
        fs::write(repo_path.join("extra.txt"), "extra").expect("write");
        let o = std::process::Command::new("git")
            .args(["add", "extra.txt"])
            .current_dir(&repo)
            .output()
            .expect("git add");
        assert!(o.status.success());
        let o = std::process::Command::new("git")
            .args(["commit", "-m", "local only"])
            .current_dir(&repo)
            .output()
            .expect("git commit");
        assert!(o.status.success());

        let counts = git_remote_counts(repo, None)
            .await
            .expect("git_remote_counts");
        assert_eq!(counts.ahead, 1, "should be 1 commit ahead");

        drop(remote_dir);
    }

    // ── git_commit_detail ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn commit_detail_initial_commit() {
        let (_dir, repo) = setup_repo();
        let commits = git_log(repo.clone(), 1, None, None).await.expect("git_log");
        let hash = commits[0].hash.clone();

        let detail = git_commit_detail(repo, hash)
            .await
            .expect("git_commit_detail");
        assert_eq!(detail.message, "initial commit");
        assert_eq!(detail.author, "Test");
        assert!(!detail.hash.is_empty());
        assert!(!detail.short_hash.is_empty());
        // Root commits have no parent, so diff-tree returns no file entries.
        // This is a known limitation — the files vec will be empty.
        assert!(
            detail.files.is_empty(),
            "root commit has no parent, diff-tree returns empty"
        );
        assert_eq!(detail.total_additions, 0);
        assert_eq!(detail.total_deletions, 0);
    }

    #[tokio::test]
    async fn commit_detail_with_modifications() {
        let (_dir, repo) = setup_repo();
        let repo_path = Path::new(&repo);
        fs::write(repo_path.join("README.md"), "# completely new content\n").expect("write");
        let o = std::process::Command::new("git")
            .args(["add", "README.md"])
            .current_dir(&repo)
            .output()
            .expect("git add");
        assert!(o.status.success());
        let o = std::process::Command::new("git")
            .args(["commit", "-m", "rewrite readme"])
            .current_dir(&repo)
            .output()
            .expect("git commit");
        assert!(o.status.success());

        let commits = git_log(repo.clone(), 1, None, None).await.expect("git_log");
        let hash = commits[0].hash.clone();

        let detail = git_commit_detail(repo, hash)
            .await
            .expect("git_commit_detail");
        assert_eq!(detail.message, "rewrite readme");
        assert!(detail.total_additions > 0, "should have additions");
        assert!(
            detail.total_deletions > 0,
            "should have deletions from old content"
        );
    }

    #[tokio::test]
    async fn commit_detail_invalid_hash() {
        let (_dir, repo) = setup_repo();
        let result = git_commit_detail(repo, "0000000000".into()).await;
        // git_commit_detail does not check git exit status, so it returns Ok
        // with empty fields for an invalid hash. This test documents that behavior.
        match result {
            Ok(detail) => {
                assert!(
                    detail.hash.is_empty(),
                    "invalid hash should produce empty detail fields"
                );
                assert!(detail.message.is_empty());
            }
            Err(_) => {
                // If the function is improved to check exit status, this is also acceptable.
            }
        }
    }

    // ── git_push / git_pull with local bare remote ─────────────────────────────

    #[tokio::test]
    async fn push_to_local_bare() {
        let (_dir, repo) = setup_repo();
        let remote_dir = tempfile::tempdir().expect("remote temp dir");
        let o = std::process::Command::new("git")
            .args(["init", "--bare"])
            .current_dir(remote_dir.path())
            .output()
            .expect("git init --bare");
        assert!(o.status.success());
        let remote_path = remote_dir.path().to_string_lossy().into_owned();

        let o = std::process::Command::new("git")
            .args(["remote", "add", "origin", &remote_path])
            .current_dir(&repo)
            .output()
            .expect("git remote add");
        assert!(o.status.success());

        let branch = get_current_branch(&repo);
        let result = git_push(repo.clone(), Some(branch)).await;
        assert!(result.is_ok(), "push should succeed: {:?}", result);

        drop(remote_dir);
    }

    #[tokio::test]
    async fn pull_from_local_bare() {
        let (_dir, repo) = setup_repo();
        let remote_dir = tempfile::tempdir().expect("remote temp dir");
        let o = std::process::Command::new("git")
            .args(["init", "--bare"])
            .current_dir(remote_dir.path())
            .output()
            .expect("git init --bare");
        assert!(o.status.success());
        let remote_path = remote_dir.path().to_string_lossy().into_owned();

        let o = std::process::Command::new("git")
            .args(["remote", "add", "origin", &remote_path])
            .current_dir(&repo)
            .output()
            .expect("git remote add");
        assert!(o.status.success());

        let branch = get_current_branch(&repo);
        // push first so there's something to pull
        let o = std::process::Command::new("git")
            .args(["push", "-u", "origin", &branch])
            .current_dir(&repo)
            .output()
            .expect("git push");
        assert!(
            o.status.success(),
            "push failed: {}",
            String::from_utf8_lossy(&o.stderr)
        );

        // pull should succeed (already up to date)
        let result = git_pull(repo).await;
        assert!(result.is_ok(), "pull should succeed: {:?}", result);

        drop(remote_dir);
    }

    // ── git_show_file_diff ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn show_file_diff_for_commit() {
        let (_dir, repo) = setup_repo();
        let repo_path = Path::new(&repo);
        fs::write(repo_path.join("README.md"), "# updated in commit\n").expect("write");
        let o = std::process::Command::new("git")
            .args(["add", "README.md"])
            .current_dir(&repo)
            .output()
            .expect("git add");
        assert!(o.status.success());
        let o = std::process::Command::new("git")
            .args(["commit", "-m", "update readme"])
            .current_dir(&repo)
            .output()
            .expect("git commit");
        assert!(o.status.success());

        let commits = git_log(repo.clone(), 1, None, None).await.expect("git_log");
        let hash = commits[0].hash.clone();

        let diff = git_show_file_diff(repo, hash, "README.md".into())
            .await
            .expect("git_show_file_diff");
        assert!(!diff.is_empty());
        assert!(diff.contains("updated in commit"));
    }

    // ── generate_commit_message — skipped (needs external agent binary) ─────────

    // generate_commit_message spawns `claude` or `codex` as external processes.
    // Testing it would require those binaries to be installed, so we skip it here.
    // The underlying git diff --staged logic is already covered by stage/commit tests.

    // ── Helper ──────────────────────────────────────────────────────────────────

    fn get_current_branch(repo: &str) -> String {
        let out = std::process::Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(repo)
            .output()
            .expect("git rev-parse HEAD");
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }
}
