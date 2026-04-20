use std::fs;
use std::path::{Component, Path, PathBuf};

use serde_json::{json, Value};

use crate::agent::tools::ToolContext;

const NOISE: &[&str] = &[
    ".git",
    ".svn",
    ".hg",
    ".idea",
    ".vscode",
    ".vs",
    "node_modules",
    "__pycache__",
    ".venv",
    "venv",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    "dist",
    "build",
    ".next",
    ".output",
    "target",
];

const DANGEROUS_PATTERNS: &[&str] = &[
    "rm -rf /",
    "rm -rf /*",
    "mkfs",
    "dd if=",
    "shutdown",
    "reboot",
    "halt",
    "poweroff",
    "format",
    ":(){:|:&};:",
];

pub(super) fn string_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key)?.as_str().map(str::to_string)
}

pub(super) fn string_array_arg(args: &Value, key: &str) -> Option<Vec<String>> {
    let values = args.get(key)?.as_array()?;
    Some(
        values
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect(),
    )
}

pub(super) fn usize_arg(args: &Value, key: &str) -> Option<usize> {
    args.get(key)?.as_u64().map(|value| value as usize)
}

pub(super) fn u64_arg(args: &Value, key: &str) -> Option<u64> {
    args.get(key)?.as_u64()
}

pub(super) fn boolish_arg(args: &Value, key: &str) -> Option<bool> {
    let value = args.get(key)?;
    if let Some(flag) = value.as_bool() {
        return Some(flag);
    }
    value.as_str().map(|flag| flag.eq_ignore_ascii_case("true"))
}

pub(super) fn with_result_mode_parameter(
    mut schema: Value,
    default_mode: &str,
    guidance: &str,
) -> Value {
    let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) else {
        return schema;
    };
    properties.insert(
        "result_mode".to_string(),
        json!({
            "type": "string",
            "description": format!(
                "控制本次工具结果写回主调度上下文的方式：auto（按工具类型自动判断）、full（尽量保留完整结果，若过长则做保守压缩）、summary（只压缩写回主调度上下文的内容，前端展示文案与详细结果引用会单独保留）。推荐默认值：{default_mode}。{guidance}"
            ),
            "enum": ["auto", "full", "summary"],
            "default": default_mode
        }),
    );
    schema
}

pub(super) fn resolve_path(context: &ToolContext, raw_path: &str) -> Result<PathBuf, String> {
    let raw = PathBuf::from(raw_path);
    let joined = if raw.is_absolute() {
        raw
    } else {
        context.workspace.join(raw)
    };
    let normalized = lexical_normalize(&joined);

    if context.restrict_to_workspace {
        let workspace = context
            .workspace
            .canonicalize()
            .map_err(|error| format!("解析工作区路径失败：{error}"))?;
        let candidate = if normalized.exists() {
            normalized
                .canonicalize()
                .map_err(|error| format!("解析路径失败：{error}"))?
        } else {
            normalized
        };
        if !candidate.starts_with(&workspace) {
            return Err(format!("错误：禁止访问工作区之外的路径：{raw_path}"));
        }
        return Ok(candidate);
    }

    Ok(normalized)
}

pub(super) fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

pub(super) fn collect_entries(
    root: &Path,
    current: &Path,
    recursive: bool,
    max_entries: usize,
    entries: &mut Vec<String>,
) {
    if entries.len() >= max_entries {
        return;
    }
    let Ok(read_dir) = fs::read_dir(current) else {
        return;
    };
    let mut items = read_dir.filter_map(Result::ok).collect::<Vec<_>>();
    items.sort_by_key(|entry| entry.file_name());

    for item in items {
        if entries.len() >= max_entries {
            entries.push(format!("... ({max_entries} entries shown)"));
            return;
        }
        if is_noise(&item.file_name()) {
            continue;
        }
        let path = item.path();
        if path.is_dir() {
            entries.push(format!("[dir] {}/", rel(&path, root)));
            if recursive {
                collect_entries(root, &path, recursive, max_entries, entries);
            }
        } else {
            entries.push(format!("[file] {}", rel(&path, root)));
        }
    }
}

pub(super) fn is_noise(name: &std::ffi::OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return true;
    };
    NOISE.contains(&name)
        || (name.starts_with('.') && !matches!(name, ".env" | ".gitignore" | ".dockerignore"))
}

pub(super) fn rel(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

pub(super) fn is_dangerous(command: &str) -> bool {
    let lower = command.to_lowercase();
    DANGEROUS_PATTERNS
        .iter()
        .any(|pattern| lower.contains(pattern))
}
