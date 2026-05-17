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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::resolve_path;
    use crate::agent::tools::ToolContext;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("{name}-{}", uuid::Uuid::new_v4()))
    }

    fn tool_context(workspace: PathBuf) -> ToolContext {
        ToolContext {
            workspace_id: "test-workspace".to_string(),
            workspace,
            session_title: "test-session".to_string(),
            exec_timeout_secs: 30,
            restrict_to_workspace: true,
            extra_allowed_dirs: vec![],
            app_handle: None,
            llm_provider: None,
            vision_model: String::new(),
            image_model_url: String::new(),
            image_model_api_key: String::new(),
            image_model: String::new(),
            image_edit_model: String::new(),
        }
    }

    #[test]
    fn resolve_path_allows_missing_child_inside_workspace() {
        let workspace = temp_path("jkcodingagent-common-path-test");
        fs::create_dir_all(&workspace).expect("create workspace");
        let context = tool_context(workspace.clone());

        let resolved = resolve_path(&context, "new/nested/file.txt").expect("resolve path");

        assert!(resolved.starts_with(workspace.canonicalize().expect("canonical workspace")));
        let _ = fs::remove_dir_all(workspace);
    }

    #[cfg(unix)]
    #[test]
    fn resolve_path_rejects_missing_child_below_symlinked_parent() {
        let workspace = temp_path("jkcodingagent-common-workspace-test");
        let outside = temp_path("jkcodingagent-common-outside-test");
        fs::create_dir_all(&workspace).expect("create workspace");
        fs::create_dir_all(&outside).expect("create outside");
        std::os::unix::fs::symlink(&outside, workspace.join("escape")).expect("create symlink");
        let context = tool_context(workspace.clone());

        let error = resolve_path(&context, "escape/passwd").expect_err("reject escaped path");

        assert!(error.contains("禁止访问工作区之外"));
        let _ = fs::remove_dir_all(workspace);
        let _ = fs::remove_dir_all(outside);
    }
}
