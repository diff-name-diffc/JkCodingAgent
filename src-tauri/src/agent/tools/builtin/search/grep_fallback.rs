use super::*;

pub(super) async fn run_grep_fallback(
    pattern: &str,
    workspace: &std::path::Path,
    target: &str,
    options: &GrepOptions,
    stdout_limit: usize,
) -> SearchOutcome {
    // 同主路径：可执行文件探测是同步文件系统 I/O，移入 spawn_blocking。
    let grep_tool = match task::spawn_blocking(|| resolve_tool("grep")).await {
        Ok(tool) => tool,
        Err(error) => {
            return SearchOutcome::error(
                "grep",
                target,
                pattern,
                format!("错误：grep 回退工具探测任务失败：{error}"),
            )
        }
    };
    let mut command = Command::new(grep_tool);
    // grep 只在 TTY 上着色，管道输出无需禁色参数；--no-color 是 ripgrep 专属，BSD/GNU grep 会报退出状态 2。
    // -I 跳二进制、-m 限单文件命中数；以 workspace 为 cwd + 相对 target，输出路径与 rg 主路径一致（相对、省 token）。
    command
        .arg("-rn")
        .arg("-H")
        .arg("-I")
        .arg("-m")
        .arg(options.max_matches_per_file.to_string())
        .current_dir(workspace)
        .kill_on_drop(true);

    match options.case_sensitive {
        Some(true) => {}
        Some(false) => {
            command.arg("-i");
        }
        // grep 没有 smart-case：pattern 不含大写字母时才 -i，近似 rg 行为。
        None => {
            if !pattern.chars().any(|c| c.is_uppercase()) {
                command.arg("-i");
            }
        }
    }
    if options.match_mode == "fixed" {
        command.arg("-F");
    }
    if options.word {
        command.arg("-w");
    }
    if options.context_before == options.context_after && options.context_before > 0 {
        command.arg("-C").arg(options.context_before.to_string());
    } else {
        if options.context_before > 0 {
            command.arg("-B").arg(options.context_before.to_string());
        }
        if options.context_after > 0 {
            command.arg("-A").arg(options.context_after.to_string());
        }
    }
    // 系统 grep 不认 .gitignore、默认遍历隐藏文件与构建产物目录，这里显式排除对齐 rg 默认行为。
    if !options.include_hidden {
        command.arg("--exclude=.*").arg("--exclude-dir=.*");
    }
    if !options.no_ignore {
        for dir in IGNORED_DIRS {
            command.arg(format!("--exclude-dir={dir}"));
        }
    }
    // include/exclude 降级：grep 只支持 basename 级 glob。
    // 含 `/` 的 include 降级成 basename 会放宽为全工作区递归匹配，宽松
    // include 下命中量放大、可能提前触发 max_files 截断，反而掩盖真正目标，
    // 直接跳过并在结果中注明已放宽。含 `/` 的 exclude 同样无法表达：
    // --exclude-dir 只匹配目录 basename，`src/generated/**` 转成
    // `--exclude-dir=src/generated` 永远不命中、形同虚设——与其静默失效，
    // 不如注明放宽。裸名 exclude 在 rg 中匹配任意深度的同名文件与目录，
    // grep 需 `--exclude` + `--exclude-dir` 并用（只用 --exclude 不跳过同名
    // 目录，结果会静默偏宽）。
    // （方向上 include 宁缺毋滥：漏掉过滤好过引入掩盖目标的噪声。）
    let mut relaxed: Vec<String> = Vec::new();
    for glob in &options.include {
        if glob.contains('/') {
            relaxed.push(format!("include `{glob}`"));
            continue;
        }
        if let Some(base) = basename_glob(glob) {
            command.arg(format!("--include={base}"));
        }
    }
    for glob in &options.exclude {
        match grep_fallback_exclude(glob) {
            Ok(flags) => {
                command.args(flags);
            }
            Err(note) => relaxed.push(note),
        }
    }
    let relaxed_note = (!relaxed.is_empty()).then(|| {
        format!(
            "[注意] 本机缺少 ripgrep，已回退到系统 grep：其不支持路径 glob，以下过滤条件未生效（结果可能偏宽）：{}\n",
            relaxed.join("、")
        )
    });

    // 与 rg 主路径同理：`-e` 保护以 `-` 开头的 pattern，`--` 保护 target。
    command.arg("-e").arg(pattern).arg("--").arg(target);

    let output = match run_bounded_search_command(command, stdout_limit).await {
        Ok(output) => output,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return SearchOutcome::error(
                "grep",
                target,
                pattern,
                "错误：未找到 ripgrep (`rg`) 或系统 grep，无法执行搜索".to_string(),
            );
        }
        Err(error) => {
            return SearchOutcome::error(
                "grep",
                target,
                pattern,
                format!("错误：grep 回退搜索失败：{error}"),
            )
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !search_status_is_success(&output) {
        if stderr.is_empty() {
            return SearchOutcome::error(
                "grep",
                target,
                pattern,
                format!("错误：grep 回退搜索失败，退出状态：{}", output.status),
            );
        }
        return SearchOutcome::error(
            "grep",
            target,
            pattern,
            format!(
                "错误：grep 回退搜索失败：{stderr}\n\n[退出状态：{}]",
                output.status
            ),
        );
    }
    if stdout.trim().is_empty() {
        let backend_note = output.truncated.then(|| {
            format!(
                "...（grep 原始输出达到 {stdout_limit} bytes 上限，未得到完整记录；请缩小路径或模式）"
            )
        });
        let display = match backend_note {
            Some(note) => format!("{}{note}", relaxed_note.clone().unwrap_or_default()),
            None => format!(
                "{}未找到匹配内容：{pattern}",
                relaxed_note.clone().unwrap_or_default()
            ),
        };
        return SearchOutcome {
            display,
            data: json!({
                "kind": "grep",
                "path": target,
                "pattern": pattern,
                "files": [],
                "totalMatches": 0,
                "truncated": output.truncated,
                "backend": "grep",
                "relaxedFilters": relaxed,
            }),
        };
    }
    // 渲染含逐候选路径的同步 stat（split_grep_line），移入 spawn_blocking
    // 避免在 async worker 线程上做文件系统 I/O（与 glob 工具的处理一致）。
    let workspace_owned = workspace.to_path_buf();
    let max_files = options.max_files;
    let files_with_matches = options.files_with_matches;
    let backend_truncated = output.truncated;
    match task::spawn_blocking(move || {
        render_grep_fallback_output(&stdout, &workspace_owned, max_files, files_with_matches)
    })
    .await
    {
        Ok(mut rendered) => {
            if backend_truncated {
                mark_backend_truncated(&mut rendered, "grep", stdout_limit);
            }
            let display = match relaxed_note {
                Some(note) => format!("{note}{}", rendered.display),
                None => rendered.display,
            };
            SearchOutcome {
                display,
                data: json!({
                    "kind": "grep",
                    "path": target,
                    "pattern": pattern,
                    "files": rendered.files,
                    "totalMatches": rendered.total_matches,
                    "truncated": rendered.truncated,
                    "backend": "grep",
                    "relaxedFilters": relaxed,
                }),
            }
        }
        Err(error) => SearchOutcome::error(
            "grep",
            target,
            pattern,
            format!("错误：grep 回退结果渲染失败：{error}"),
        ),
    }
}

/// basename 级 glob 的校验（仅对不含 `/` 的模式调用）：过滤掉 `*`/`**`
/// 等等价于「不过滤」的模式。含 `/` 的路径 glob 无法安全降级为 basename
/// 匹配（语义偏差见调用方注释），不会进入本函数。
pub(super) fn basename_glob(glob: &str) -> Option<String> {
    // rsplit 至少产出一项，此处不存在失败分支。
    let base = glob
        .rsplit('/')
        .next()
        .expect("rsplit 至少产出一项")
        .trim_end_matches('/');
    if base.is_empty() || base == "*" || base == "**" {
        return None;
    }
    Some(base.to_string())
}

/// `dir/**`、`dir/*` 形式的排除模式转成 --exclude-dir=dir（grep 无路径 glob 能力）。
pub(super) fn dir_glob(glob: &str) -> Option<String> {
    let trimmed = glob.trim_end_matches('/');
    let dir = trimmed
        .strip_suffix("/**")
        .or_else(|| trimmed.strip_suffix("/*"))?
        .trim_end_matches('/');
    if dir.is_empty() || dir.contains('*') {
        return None;
    }
    Some(dir.to_string())
}

/// grep 回退路径上降级单个 exclude 模式。Ok 为应追加给 grep 的参数；
/// Err 为无法安全降级的模式说明（调用方汇入放宽提示），避免静默偏差。
/// - 裸名模式（`target`、`*.log`、尾斜杠的 `target/`）：rg（gitignore 语义）
///   匹配任意深度的同名文件或目录，grep 需 `--exclude-dir`（目录）与
///   `--exclude`（文件，尾斜杠按 gitignore 语义仅限目录则省略）并用；
/// - `name/**` / `name/*`：以 `--exclude-dir=name` 近似。注意 rg 的 `name/*`
///   与 `name/**` 均剪枝整棵目录树（gitignore 目录剪枝语义），两者等价；
///   且 rg 含 `/` 的 glob 锚定搜索根（仅排除顶层 name 目录），grep 则排除
///   任意深度同名目录，回退结果略窄——对 target/dist 等常见构建产物目录是
///   可接受的近似；
/// - 其余含 `/` 的路径模式：`--exclude-dir` 只匹配目录 basename，
///   `src/generated` 之类永远不命中，与其静默失效不如注明放宽。
pub(super) fn grep_fallback_exclude(glob: &str) -> Result<Vec<String>, String> {
    if let Some(dir) = dir_glob(glob) {
        return if dir.contains('/') {
            Err(format!("exclude `{glob}`"))
        } else {
            Ok(vec![format!("--exclude-dir={dir}")])
        };
    }
    let trimmed = glob.trim_end_matches('/');
    if trimmed.contains('/') {
        return Err(format!("exclude `{glob}`"));
    }
    let Some(base) = basename_glob(trimmed) else {
        // `*`/`**` 等等价于「不排除」：无需参数，也无需放宽提示
        return Ok(Vec::new());
    };
    let mut flags = vec![format!("--exclude-dir={base}")];
    if trimmed.len() == glob.len() {
        flags.push(format!("--exclude={base}"));
    }
    Ok(flags)
}

/// 解析 `path:行: 文本`（命中）与 `path-行- 文本`（上下文）。
/// 两种分隔符都可能出现在路径或文本内部（如 `logs-2024-01/a.rs` 的目录名、
/// 上下文行文本里的 `:5:` 字面量），仅取首个出现位置会误切。grep 以
/// workspace 为 cwd、输出相对路径，因此用「路径段对应真实存在的文件」裁决：
/// 枚举全部 `:行:` 与 `-行-` 候选切分，收集路径可证实的候选——
/// - 恰有一个：直接采用；
/// - 多个同时可证实（路径自身含 `:数字:`/`-数字-`，且某个较短前缀也恰好
///   是文件，如 `a:12:b.rs` 与文件 `a` 并存）：按字节无法区分，取最长路径
///   候选——把更多字符当路径、更少当文本，最接近 grep 的原始输出结构；
/// - 零个：非 grep 格式行。
///
/// 行号解析失败（如溢出 u64）按该候选无效处理，继续尝试后续位置。
/// `path_cache` 缓存已证实/已否定的路径：大输出中同一文件路径反复出现，
/// 无缓存时 stat 次数约为 行数×候选位置数，缓存后收敛为唯一候选路径数。
pub(super) fn split_grep_line<'a>(
    line: &'a str,
    workspace: &std::path::Path,
    path_cache: &mut HashMap<&'a str, bool>,
) -> Option<(&'a str, u64, bool, &'a str)> {
    let bytes = line.as_bytes();
    let mut confirmed: Vec<(&'a str, u64, bool, &'a str)> = Vec::new();
    for sep in [b':', b'-'] {
        for i in 1..bytes.len() {
            if bytes[i] != sep {
                continue;
            }
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j == start || j >= bytes.len() || bytes[j] != sep {
                continue;
            }
            let Some(line_number) = line[start..j].parse::<u64>().ok() else {
                continue;
            };
            let path = &line[..i];
            let is_file = *path_cache
                .entry(path)
                .or_insert_with(|| workspace.join(path).is_file());
            if is_file {
                confirmed.push((path, line_number, sep == b':', &line[j + 1..]));
            }
        }
    }
    match confirmed.len() {
        0 => None,
        1 => confirmed.pop(),
        _ => confirmed
            .into_iter()
            .max_by_key(|(path, _, _, _)| path.len()),
    }
}

pub(super) fn render_grep_fallback_output(
    stdout: &str,
    workspace: &std::path::Path,
    max_files: usize,
    files_with_matches: bool,
) -> GrepRendered {
    let mut file_results: Vec<GrepFileResult> = Vec::new();
    let mut file_index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut path_cache: HashMap<&str, bool> = HashMap::new();
    let mut total_matches = 0usize;
    let mut truncated_by_file_limit = false;

    for line in stdout.lines() {
        let Some((path, line_number, is_match, text)) =
            split_grep_line(line, workspace, &mut path_cache)
        else {
            continue;
        };
        if is_noise(std::ffi::OsStr::new(
            std::path::Path::new(path).file_name().unwrap_or_default(),
        )) {
            continue;
        }

        let entry = match file_index.get(path) {
            Some(&idx) => Some(idx),
            None if file_results.len() < max_files => {
                let idx = file_results.len();
                file_results.push(GrepFileResult {
                    path: path.to_string(),
                    match_count: 0,
                    lines: Vec::new(),
                });
                file_index.insert(path.to_string(), idx);
                Some(idx)
            }
            None => {
                // 超过 max_files 的命中文件被丢弃：与 rg 主路径一致，标记截断，
                // 避免静默丢弃并在统计中误导模型已覆盖全部结果。
                truncated_by_file_limit = true;
                None
            }
        };
        let Some(idx) = entry else {
            continue;
        };
        if is_match {
            total_matches += 1;
            file_results[idx].match_count += 1;
        }
        if files_with_matches {
            continue;
        }
        file_results[idx].lines.push(GrepLine {
            line_number,
            text: text.to_string(),
            is_match,
        });
    }

    let matched_files = file_results
        .iter()
        .filter(|file| file.match_count > 0)
        .collect::<Vec<_>>();
    if matched_files.is_empty() {
        return GrepRendered {
            display: String::new(),
            files: Vec::new(),
            total_matches: 0,
            truncated: false,
        };
    }

    let mut lines = vec![format!(
        "共 {} 个文件 / {} 处匹配 (grep 回退)",
        matched_files.len(),
        total_matches
    )];
    if truncated_by_file_limit {
        lines.push(format!(
            "...（只展示前 {} 个命中文件，匹配数为部分统计，请继续缩小范围）",
            max_files
        ));
    }

    if files_with_matches {
        lines.extend(matched_files.iter().map(|file| file.path.clone()));
        return GrepRendered {
            display: lines.join("\n"),
            files: matched_files
                .into_iter()
                .map(|file| {
                    json!({
                        "path": file.path,
                        "matchCount": file.match_count,
                        "lines": [],
                    })
                })
                .collect(),
            total_matches,
            truncated: truncated_by_file_limit,
        };
    }

    for file in &matched_files {
        lines.push(String::new());
        lines.push(file.path.clone());
        for entry in &file.lines {
            let separator = if entry.is_match { ':' } else { '-' };
            lines.push(format!("{}{} {}", entry.line_number, separator, entry.text));
        }
    }
    let files = matched_files
        .into_iter()
        .map(|file| {
            json!({
                "path": file.path,
                "matchCount": file.match_count,
                "lines": file.lines.iter().map(|line| json!({
                    "lineNumber": line.line_number,
                    "text": line.text,
                    "isMatch": line.is_match,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    GrepRendered {
        display: lines.join("\n"),
        files,
        total_matches,
        truncated: truncated_by_file_limit,
    }
}
