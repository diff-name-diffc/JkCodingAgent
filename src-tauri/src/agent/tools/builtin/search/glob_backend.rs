use super::*;

pub(super) fn safe_glob_entries(
    root: &Path,
    max_depth: Option<usize>,
) -> impl Iterator<Item = Result<walkdir::DirEntry, walkdir::Error>> {
    let mut builder = WalkDir::new(root).follow_links(false).min_depth(1);
    if let Some(max_depth) = max_depth {
        builder = builder.max_depth(max_depth);
    }
    builder
        .into_iter()
        .filter_entry(|entry| !entry.file_type().is_symlink() && !is_noise(entry.file_name()))
}

pub(super) fn run_glob_query(
    path: &str,
    pattern: &str,
    max_results: usize,
    context: &ToolContext,
) -> SearchOutcome {
    let dir_path = match resolve_path(context, path) {
        Ok(path) => path,
        Err(message) => return SearchOutcome::error("glob", path, pattern, message),
    };
    let search_pattern = match glob_search_pattern(context, &dir_path, pattern) {
        Ok(search_pattern) => search_pattern,
        Err(message) => return SearchOutcome::error("glob", path, pattern, message),
    };
    let relative_pattern = match Path::new(&search_pattern).strip_prefix(&dir_path) {
        Ok(pattern) => pattern,
        Err(_) => {
            return SearchOutcome::error(
                "glob",
                path,
                pattern,
                "错误：glob 模式无法转换为工作区相对路径".to_string(),
            )
        }
    };
    let matcher = match Pattern::new(&relative_pattern.to_string_lossy()) {
        Ok(matcher) => matcher,
        Err(error) => {
            return SearchOutcome::error(
                "glob",
                path,
                pattern,
                format!("错误：glob 模式无效：{error}"),
            )
        }
    };
    let max_depth = relative_pattern
        .components()
        .all(|component| component.as_os_str() != "**")
        .then(|| relative_pattern.components().count());
    let match_options = MatchOptions {
        case_sensitive: true,
        require_literal_separator: true,
        require_literal_leading_dot: false,
    };

    // 只保留修改时间最新的 max_results 项；总命中数单独计数。旧实现先把
    // 整个工作区所有匹配路径与 metadata 全量收进 Vec 后才截断，`**/*`
    // 可轻易把单次调用变成无界内存增长。
    let mut newest: BinaryHeap<Reverse<(Option<std::time::SystemTime>, std::path::PathBuf)>> =
        BinaryHeap::with_capacity(max_results.saturating_add(1));
    let mut total_matches = 0usize;
    let mut scanned_entries = 0usize;
    let mut scan_truncated = false;
    for entry in safe_glob_entries(&dir_path, max_depth) {
        if scanned_entries >= MAX_GLOB_SCAN_ENTRIES {
            scan_truncated = true;
            break;
        }
        scanned_entries += 1;
        match entry {
            Ok(entry) => {
                let path = entry.into_path();
                let relative = path.strip_prefix(&dir_path).unwrap_or(&path);
                if !matcher.matches_path_with(relative, match_options) {
                    continue;
                }
                total_matches = total_matches.saturating_add(1);
                let modified = fs::symlink_metadata(&path)
                    .and_then(|metadata| metadata.modified())
                    .ok();
                let candidate = (modified, path);
                if newest.len() < max_results {
                    newest.push(Reverse(candidate));
                } else if newest
                    .peek()
                    .is_some_and(|Reverse(oldest)| candidate > *oldest)
                {
                    newest.pop();
                    newest.push(Reverse(candidate));
                }
            }
            Err(error) => {
                return SearchOutcome::error(
                    "glob",
                    path,
                    pattern,
                    format!("错误：glob 搜索失败：{error}"),
                )
            }
        }
    }
    let mut matches_with_metadata = newest
        .into_iter()
        .map(|Reverse((modified, path))| (path, modified))
        .collect::<Vec<_>>();
    matches_with_metadata.sort_by_key(|(_, modified)| *modified);
    matches_with_metadata.reverse();

    if matches_with_metadata.is_empty() {
        let display = if scan_truncated {
            format!(
                "在前 {MAX_GLOB_SCAN_ENTRIES} 个目录项中未找到匹配文件；扫描已达上限，请缩小路径或模式：{}",
                dir_path.display()
            )
        } else {
            format!("未找到匹配文件：{}", dir_path.display())
        };
        return SearchOutcome {
            display,
            data: json!({
                "kind": "glob",
                "path": path,
                "pattern": pattern,
                "matches": [],
                "total": 0,
                "truncated": scan_truncated,
                "scanTruncated": scan_truncated,
                "scannedEntries": scanned_entries,
            }),
        };
    }

    let matches = matches_with_metadata
        .iter()
        .map(|(path, _)| rel(path, &dir_path))
        .collect::<Vec<_>>();
    let mut lines = matches.clone();
    let result_truncated = total_matches > matches.len();
    if result_truncated {
        lines.push(format!(
            "...（已显示 {} / {}）",
            matches.len(),
            total_matches
        ));
    }
    if scan_truncated {
        lines.push(format!(
            "...（扫描达到 {MAX_GLOB_SCAN_ENTRIES} 个目录项上限；命中总数仅代表已扫描范围，请缩小路径或模式）"
        ));
    }
    SearchOutcome {
        display: lines.join("\n"),
        data: json!({
            "kind": "glob",
            "path": path,
            "pattern": pattern,
            "matches": matches,
            "total": total_matches,
            "truncated": result_truncated || scan_truncated,
            "scanTruncated": scan_truncated,
            "scannedEntries": scanned_entries,
        }),
    }
}

/// 将起始目录与用户传入的 glob 模式拼接后做归一化与工作区校验，防止
/// 「pattern 为绝对路径」或「pattern 含 `..`」绕过目录校验遍历工作区之外的文件。
/// glob 通配符无法 canonicalize，因此用词法归一化 + 前缀包含判断（fail-closed）。
pub(super) fn glob_search_pattern(
    context: &ToolContext,
    dir_path: &Path,
    pattern: &str,
) -> Result<String, String> {
    let joined = lexical_normalize(&dir_path.join(pattern));

    if context.restrict_to_workspace {
        let mut allowed_roots = vec![context.workspace.clone()];
        allowed_roots.extend(context.extra_allowed_dirs.iter().cloned());
        if !path_within_allowed_roots(&joined, &allowed_roots) {
            return Err(format!(
                "错误：glob 模式越界，禁止搜索工作区之外的路径：{pattern}"
            ));
        }
    }

    joined
        .to_str()
        .map(str::to_string)
        .ok_or_else(|| "错误：glob 模式不是有效的 UTF-8".to_string())
}

/// 判断（词法归一化后的）路径是否位于任一允许根目录之内。
/// 允许根目录无法 canonicalize 时按不包含处理（fail-closed）。
pub(super) fn path_within_allowed_roots(
    joined: &Path,
    allowed_roots: &[std::path::PathBuf],
) -> bool {
    allowed_roots.iter().any(|root| {
        root.canonicalize()
            .map(|canonical| joined.starts_with(&canonical))
            .unwrap_or(false)
    })
}
