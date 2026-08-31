use super::*;

pub(super) async fn run_grep_query(
    pattern: &str,
    path: &str,
    options: &GrepOptions,
    context: &ToolContext,
    stdout_limit: usize,
) -> SearchOutcome {
    // 路径解析、存在性检查、workspace canonicalize 与 rg 可执行文件探测都是
    // 同步文件系统 I/O，统一移入 spawn_blocking，避免阻塞 Tokio 工作线程。
    let context = context.clone();
    let path_owned = path.to_string();
    let prep = match task::spawn_blocking(move || prepare_grep_search(&path_owned, &context)).await
    {
        Ok(result) => result,
        Err(error) => {
            return SearchOutcome::error(
                "grep",
                path,
                pattern,
                format!("错误：grep 搜索准备任务失败：{error}"),
            )
        }
    };
    let GrepSearchPrep {
        workspace,
        target,
        rg_tool,
    } = match prep {
        Ok(prep) => prep,
        Err(message) => return SearchOutcome::error("grep", path, pattern, message),
    };

    let mut command = Command::new(rg_tool);
    command
        .arg("--json")
        .arg("--line-number")
        .arg("--color")
        .arg("never")
        .arg("--max-count")
        .arg(options.max_matches_per_file.to_string())
        .current_dir(&workspace)
        .kill_on_drop(true);

    match options.case_sensitive {
        Some(true) => {
            command.arg("--case-sensitive");
        }
        Some(false) => {
            command.arg("--ignore-case");
        }
        None => {
            command.arg("--smart-case");
        }
    }

    if options.match_mode == "fixed" {
        command.arg("--fixed-strings");
    }
    if options.word {
        command.arg("--word-regexp");
    }
    if options.include_hidden {
        command.arg("--hidden");
    }
    if options.no_ignore {
        command.arg("--no-ignore");
    }
    if options.context_before == options.context_after && options.context_before > 0 {
        command
            .arg("--context")
            .arg(options.context_before.to_string());
    } else {
        if options.context_before > 0 {
            command
                .arg("--before-context")
                .arg(options.context_before.to_string());
        }
        if options.context_after > 0 {
            command
                .arg("--after-context")
                .arg(options.context_after.to_string());
        }
    }

    for glob in &options.include {
        command.arg("--glob").arg(glob);
    }
    for glob in &options.exclude {
        command.arg("--glob").arg(format!("!{glob}"));
    }

    // `-e` 隔离 pattern 与选项：以 `-` 开头的 pattern（如搜索字面量 `--force`）
    // 否则会被解析为命令行选项；`--` 同样保护以 `-` 开头的 target 路径。
    command.arg("-e").arg(pattern).arg("--").arg(&target);

    let output = match run_bounded_search_command(command, stdout_limit).await {
        Ok(output) => output,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return run_grep_fallback(pattern, &workspace, &target, options, stdout_limit).await;
        }
        Err(error) => {
            return SearchOutcome::error(
                "grep",
                path,
                pattern,
                format!("错误：执行 grep 搜索失败：{error}"),
            )
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !search_status_is_success(&output) {
        if stderr.is_empty() {
            return SearchOutcome::error(
                "grep",
                path,
                pattern,
                format!("错误：grep 搜索失败，退出状态：{}", output.status),
            );
        }
        return SearchOutcome::error(
            "grep",
            path,
            pattern,
            format!(
                "错误：grep 搜索失败：{stderr}\n\n[退出状态：{}]",
                output.status
            ),
        );
    }

    let mut rendered = render_grep_stdout(&stdout, options.max_files, options.files_with_matches);
    if output.truncated {
        mark_backend_truncated(&mut rendered, "ripgrep", stdout_limit);
    }
    if rendered.display.is_empty() {
        let display = format!("未找到匹配内容：{pattern}");
        return SearchOutcome {
            display,
            data: json!({
                "kind": "grep",
                "path": path,
                "pattern": pattern,
                "files": [],
                "totalMatches": 0,
                "truncated": output.truncated,
                "backend": "ripgrep",
            }),
        };
    }
    SearchOutcome {
        display: rendered.display,
        data: json!({
            "kind": "grep",
            "path": path,
            "pattern": pattern,
            "files": rendered.files,
            "totalMatches": rendered.total_matches,
            "truncated": rendered.truncated,
            "backend": "ripgrep",
        }),
    }
}
