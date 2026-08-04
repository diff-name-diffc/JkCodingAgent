use std::collections::HashSet;
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
    // Destructive file operations
    "rm -rf /",
    "rm -rf /*",
    "rm -rf ~",
    "rm -rf ~/",
    "rm -rf *",
    // Disk/filesystem destruction
    "mkfs",
    "dd if=/dev/zero",
    "dd if=/dev/random",
    "dd if=/dev/urandom",
    // System shutdown/control
    "shutdown",
    "reboot",
    "halt",
    "poweroff",
    "init 0",
    "init 6",
    // Permission escalation / open permissions
    "chmod 777",
    "chmod -r 777",
    "chown root",
    // Fork bombs
    ":(){:|:&};:",
    "fork bomb",
    // Remote code execution patterns
    "curl | sh",
    "curl | bash",
    "curl | sudo",
    "wget | sh",
    "wget | bash",
    "wget | sudo",
    // Package manager piped install
    "curl | apt",
    "curl | yum",
    // Kernel/module operations
    "rmmod",
    "insmod",
    "modprobe",
    // Network dangerous
    "iptables -f",
    "ip route flush",
    // Overwrite boot/EFI
    "dd of=/dev/sda",
    "dd of=/dev/nvme",
    "dd of=/dev/hda",
    // macOS-specific
    "diskutil erasevolume",
    "diskutil erasedisk",
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

pub(super) fn non_empty_string_array_arg(args: &Value, key: &str) -> Option<Vec<String>> {
    let values = string_array_arg(args, key)?
        .into_iter()
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>();
    (!values.is_empty()).then_some(values)
}

pub(super) fn string_list_arg(
    args: &Value,
    single_key: &str,
    list_key: &str,
) -> Result<Vec<String>, String> {
    let mut values = Vec::new();
    if let Some(value) = string_arg(args, single_key) {
        values.push(value);
    }
    if let Some(items) = string_array_arg(args, list_key) {
        values.extend(items);
    }

    let mut seen = HashSet::new();
    values.retain(|value| !value.trim().is_empty() && seen.insert(value.clone()));

    if values.is_empty() {
        Err(format!("错误：缺少必填参数 {single_key} 或 {list_key}"))
    } else {
        Ok(values)
    }
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

pub(super) fn with_compression_parameters(
    mut schema: Value,
    default_compress: bool,
    tool_specific_guidance: &str,
) -> Value {
    let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) else {
        return schema;
    };
    properties.insert(
        "compress".to_string(),
        json!({
            "type": "boolean",
            "description": format!(
                "是否允许对超长工具结果进行语义压缩。只有 compress=true 且原始结果超过 5000 字符时，才会调用摘要模型根据 compress_intent 提取关键信息；compress=false 绝不会进行摘要。未摘要的结果超过 2000 字符时会明确标记并截断，完整原文保留在工具产物中。{tool_specific_guidance}"
            ),
            "default": default_compress
        }),
    );
    properties.insert(
        "compress_intent".to_string(),
        json!({
            "type": "string",
            "description": "当 compress=true 时，用一句话描述期望从超长结果中提取什么信息。摘要会优先返回可用 path:start-end 精确定位的多段内容。例如：'查找 handleToolResult 函数的实现逻辑和调用链'。"
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
        let candidate = canonicalize_existing_prefix(&normalized)?;

        let workspace = context
            .workspace
            .canonicalize()
            .map_err(|error| format!("解析工作区路径失败：{error}"))?;

        let in_workspace = candidate.starts_with(&workspace);
        let in_extra = context.extra_allowed_dirs.iter().any(|dir| {
            dir.canonicalize()
                .is_ok_and(|canonical| candidate.starts_with(canonical))
        });

        if !in_workspace && !in_extra {
            return Err(format!("错误：禁止访问工作区之外的路径：{raw_path}"));
        }
        return Ok(candidate);
    }

    Ok(normalized)
}

fn canonicalize_existing_prefix(path: &Path) -> Result<PathBuf, String> {
    let mut missing_components = Vec::new();
    let mut cursor = path;

    loop {
        match fs::symlink_metadata(cursor) {
            Ok(_) => {
                let mut resolved = cursor
                    .canonicalize()
                    .map_err(|error| format!("解析路径失败：{error}"))?;
                for component in missing_components.iter().rev() {
                    resolved.push(component);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = cursor.file_name() else {
                    return Err(format!("解析路径失败：{}", path.display()));
                };
                missing_components.push(name.to_os_string());
                cursor = cursor
                    .parent()
                    .ok_or_else(|| format!("解析路径失败：{}", path.display()))?;
            }
            Err(error) => return Err(format!("读取路径元数据失败：{error}")),
        }
    }
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

pub(super) fn render_labeled_sections(sections: Vec<(String, String)>) -> String {
    sections
        .into_iter()
        .map(|(label, body)| {
            let body = body.trim();
            if body.is_empty() {
                format!("## {label}\n[无结果]")
            } else {
                format!("## {label}\n{body}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
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
