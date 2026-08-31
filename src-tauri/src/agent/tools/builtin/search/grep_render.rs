use super::*;

pub(super) fn workspace_relative_target(
    workspace: &std::path::Path,
    target: &std::path::Path,
) -> String {
    if target == workspace {
        ".".to_string()
    } else {
        target
            .strip_prefix(workspace)
            .unwrap_or(target)
            .to_string_lossy()
            .to_string()
    }
}

/// grep 主路径的同步准备结果（在 spawn_blocking 中计算）。
pub(super) struct GrepSearchPrep {
    pub(super) workspace: std::path::PathBuf,
    pub(super) target: String,
    pub(super) rg_tool: std::path::PathBuf,
}

pub(super) fn prepare_grep_search(
    path: &str,
    context: &ToolContext,
) -> Result<GrepSearchPrep, String> {
    let search_path = resolve_path(context, path)?;
    if !search_path.exists() {
        return Err(format!("错误：搜索路径不存在：{path}"));
    }
    let workspace = context
        .workspace
        .canonicalize()
        .map_err(|error| format!("错误：解析工作区路径失败：{error}"))?;
    let target = workspace_relative_target(&workspace, &search_path);
    Ok(GrepSearchPrep {
        workspace,
        target,
        rg_tool: resolve_tool("rg"),
    })
}

pub(super) fn render_grep_stdout(
    stdout: &str,
    max_files: usize,
    files_with_matches: bool,
) -> GrepRendered {
    let mut file_results = Vec::<GrepFileResult>::new();
    let mut file_indexes = HashMap::<String, usize>::new();
    let mut total_matches = 0usize;
    let mut truncated_by_file_limit = false;

    for raw_line in stdout.lines() {
        let Ok(value) = serde_json::from_str::<Value>(raw_line) else {
            continue;
        };
        let Some(kind) = value.get("type").and_then(Value::as_str) else {
            continue;
        };
        if !matches!(kind, "match" | "context") {
            continue;
        }

        let Some(path) = grep_event_path(&value) else {
            continue;
        };
        let index = if let Some(index) = file_indexes.get(&path).copied() {
            index
        } else if file_results.len() < max_files {
            let index = file_results.len();
            file_results.push(GrepFileResult {
                path: path.clone(),
                ..Default::default()
            });
            file_indexes.insert(path.clone(), index);
            index
        } else {
            truncated_by_file_limit = true;
            continue;
        };

        if kind == "match" {
            total_matches += 1;
            file_results[index].match_count += 1;
            if files_with_matches {
                continue;
            }
        } else if files_with_matches {
            continue;
        }

        let Some(line_number) = value
            .get("data")
            .and_then(|data| data.get("line_number"))
            .and_then(Value::as_u64)
        else {
            continue;
        };
        let Some(text) = grep_event_text(&value) else {
            continue;
        };
        file_results[index].lines.push(GrepLine {
            line_number,
            text,
            is_match: kind == "match",
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
        "共 {} 个文件 / {} 处匹配",
        matched_files.len(),
        total_matches
    )];
    if truncated_by_file_limit {
        lines.push(format!(
            "...（只展示前 {} 个命中文件，请继续缩小范围）",
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

pub(super) fn grep_event_path(value: &Value) -> Option<String> {
    let data = value.get("data")?;
    let path = data.get("path")?;
    if let Some(text) = path.get("text").and_then(Value::as_str) {
        return Some(text.to_string());
    }
    let encoded = path.get("bytes").and_then(Value::as_str)?;
    STANDARD
        .decode(encoded)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

pub(super) fn grep_event_text(value: &Value) -> Option<String> {
    let lines = value.get("data")?.get("lines")?;
    if let Some(text) = lines.get("text").and_then(Value::as_str) {
        return Some(text.trim_end_matches('\n').to_string());
    }
    let encoded = lines.get("bytes").and_then(Value::as_str)?;
    STANDARD
        .decode(encoded)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|text| text.trim_end_matches('\n').to_string())
}
