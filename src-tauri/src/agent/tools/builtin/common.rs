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
                "是否对工具结果进行语义压缩：调用摘要模型根据 compress_intent 描述的目的，从原始结果中提取关键信息并返回给主模型。结果超过 1000 字符时系统会强制压缩。{tool_specific_guidance}"
            ),
            "default": default_compress
        }),
    );
    properties.insert(
        "compress_intent".to_string(),
        json!({
            "type": "string",
            "description": "当 compress=true 或系统强制压缩时，用一句话描述本次工具调用期望从结果中提取什么信息；任何时候都不应为空。例如：'查找 handleToolResult 函数的实现逻辑和调用链'、'确认 pnpm test 是否全部通过以及失败项'、'获取配置文件中的端口和数据库连接信息'。"
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

    use serde_json::json;

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
            sub_agent_tool_registry: None,
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

    // --- Argument parsing tests ---

    #[test]
    fn string_arg_returns_value_when_present() {
        let args = json!({"name": "test-value"});
        assert_eq!(
            super::string_arg(&args, "name"),
            Some("test-value".to_string())
        );
    }

    #[test]
    fn string_arg_returns_none_when_missing() {
        let args = json!({"other": "value"});
        assert_eq!(super::string_arg(&args, "name"), None);
    }

    #[test]
    fn string_arg_returns_none_for_non_string() {
        let args = json!({"name": 42});
        assert_eq!(super::string_arg(&args, "name"), None);
    }

    #[test]
    fn string_arg_returns_none_for_null() {
        let args = json!({"name": null});
        assert_eq!(super::string_arg(&args, "name"), None);
    }

    #[test]
    fn string_array_arg_returns_vec_when_present() {
        let args = json!({"paths": ["/a", "/b", "/c"]});
        assert_eq!(
            super::string_array_arg(&args, "paths"),
            Some(vec!["/a".to_string(), "/b".to_string(), "/c".to_string()])
        );
    }

    #[test]
    fn string_array_arg_skips_non_string_elements() {
        let args = json!({"paths": ["/a", 42, "/c"]});
        assert_eq!(
            super::string_array_arg(&args, "paths"),
            Some(vec!["/a".to_string(), "/c".to_string()])
        );
    }

    #[test]
    fn string_array_arg_returns_none_for_non_array() {
        let args = json!({"paths": "single"});
        assert_eq!(super::string_array_arg(&args, "paths"), None);
    }

    #[test]
    fn string_array_arg_returns_none_when_missing() {
        let args = json!({});
        assert_eq!(super::string_array_arg(&args, "paths"), None);
    }

    #[test]
    fn non_empty_string_array_arg_filters_empty_strings() {
        let args = json!({"items": ["a", "", "  ", "b"]});
        assert_eq!(
            super::non_empty_string_array_arg(&args, "items"),
            Some(vec!["a".to_string(), "b".to_string()])
        );
    }

    #[test]
    fn non_empty_string_array_arg_returns_none_when_all_empty() {
        let args = json!({"items": ["", "  ", "   "]});
        assert_eq!(super::non_empty_string_array_arg(&args, "items"), None);
    }

    #[test]
    fn non_empty_string_array_arg_returns_none_when_missing() {
        let args = json!({});
        assert_eq!(super::non_empty_string_array_arg(&args, "items"), None);
    }

    #[test]
    fn string_list_arg_combines_single_and_list() {
        let args = json!({"path": "/single", "paths": ["/a", "/b"]});
        let result = super::string_list_arg(&args, "path", "paths").unwrap();
        assert_eq!(
            result,
            vec!["/single".to_string(), "/a".to_string(), "/b".to_string()]
        );
    }

    #[test]
    fn string_list_arg_works_with_single_only() {
        let args = json!({"path": "/single"});
        let result = super::string_list_arg(&args, "path", "paths").unwrap();
        assert_eq!(result, vec!["/single".to_string()]);
    }

    #[test]
    fn string_list_arg_works_with_list_only() {
        let args = json!({"paths": ["/a", "/b"]});
        let result = super::string_list_arg(&args, "path", "paths").unwrap();
        assert_eq!(result, vec!["/a".to_string(), "/b".to_string()]);
    }

    #[test]
    fn string_list_arg_deduplicates() {
        let args = json!({"path": "/a", "paths": ["/a", "/b"]});
        let result = super::string_list_arg(&args, "path", "paths").unwrap();
        assert_eq!(result, vec!["/a".to_string(), "/b".to_string()]);
    }

    #[test]
    fn string_list_arg_filters_empty() {
        let args = json!({"paths": ["", "  ", "/a"]});
        let result = super::string_list_arg(&args, "path", "paths").unwrap();
        assert_eq!(result, vec!["/a".to_string()]);
    }

    #[test]
    fn string_list_arg_errors_when_both_missing() {
        let args = json!({});
        let error = super::string_list_arg(&args, "path", "paths").unwrap_err();
        assert!(error.contains("缺少必填参数"));
    }

    #[test]
    fn usize_arg_returns_value_when_present() {
        let args = json!({"count": 42});
        assert_eq!(super::usize_arg(&args, "count"), Some(42));
    }

    #[test]
    fn usize_arg_returns_none_for_non_number() {
        let args = json!({"count": "not a number"});
        assert_eq!(super::usize_arg(&args, "count"), None);
    }

    #[test]
    fn usize_arg_returns_none_when_missing() {
        let args = json!({});
        assert_eq!(super::usize_arg(&args, "count"), None);
    }

    #[test]
    fn u64_arg_returns_value_when_present() {
        let args = json!({"timeout": 300});
        assert_eq!(super::u64_arg(&args, "timeout"), Some(300));
    }

    #[test]
    fn u64_arg_returns_none_for_float() {
        let args = json!({"timeout": 2.5});
        assert_eq!(super::u64_arg(&args, "timeout"), None);
    }

    #[test]
    fn boolish_arg_accepts_bool_true() {
        let args = json!({"flag": true});
        assert_eq!(super::boolish_arg(&args, "flag"), Some(true));
    }

    #[test]
    fn boolish_arg_accepts_bool_false() {
        let args = json!({"flag": false});
        assert_eq!(super::boolish_arg(&args, "flag"), Some(false));
    }

    #[test]
    fn boolish_arg_accepts_string_true() {
        let args = json!({"flag": "true"});
        assert_eq!(super::boolish_arg(&args, "flag"), Some(true));
    }

    #[test]
    fn boolish_arg_accepts_string_true_case_insensitive() {
        let args = json!({"flag": "TRUE"});
        assert_eq!(super::boolish_arg(&args, "flag"), Some(true));
    }

    #[test]
    fn boolish_arg_string_false_yields_some_false() {
        let args = json!({"flag": "false"});
        // "false".eq_ignore_ascii_case("true") is false => .map returns Some(false)
        assert_eq!(super::boolish_arg(&args, "flag"), Some(false));
    }

    #[test]
    fn boolish_arg_returns_none_for_non_bool_non_string() {
        let args = json!({"flag": 42});
        assert_eq!(super::boolish_arg(&args, "flag"), None);
    }

    // --- with_compression_parameters tests ---

    #[test]
    fn with_compression_parameters_adds_compress_and_intent_properties() {
        let schema = json!({"type": "object", "properties": {}});
        let result = super::with_compression_parameters(schema, false, "tool-specific guidance");
        let props = result.get("properties").unwrap().as_object().unwrap();
        assert!(props.contains_key("compress"));
        assert!(props.contains_key("compress_intent"));
        let compress = &props["compress"];
        assert_eq!(compress["default"], false);
        assert_eq!(compress["type"], "boolean");
        assert!(compress["description"]
            .as_str()
            .unwrap()
            .contains("tool-specific guidance"));
    }

    #[test]
    fn with_compression_parameters_preserves_existing_properties() {
        let schema = json!({"type": "object", "properties": {"path": {"type": "string"}}});
        let result = super::with_compression_parameters(schema, true, "");
        let props = result.get("properties").unwrap().as_object().unwrap();
        assert!(props.contains_key("path"));
        assert!(props.contains_key("compress"));
        assert!(props.contains_key("compress_intent"));
    }

    #[test]
    fn with_compression_parameters_returns_unchanged_without_properties() {
        let schema = json!({"type": "object"});
        let result = super::with_compression_parameters(schema, false, "");
        assert!(result.get("properties").is_none());
    }

    // --- is_noise tests ---

    #[test]
    fn is_noise_detects_known_noise_dirs() {
        assert!(super::is_noise(std::ffi::OsStr::new("node_modules")));
        assert!(super::is_noise(std::ffi::OsStr::new(".git")));
        assert!(super::is_noise(std::ffi::OsStr::new("__pycache__")));
        assert!(super::is_noise(std::ffi::OsStr::new("target")));
        assert!(super::is_noise(std::ffi::OsStr::new("dist")));
        assert!(super::is_noise(std::ffi::OsStr::new("build")));
        assert!(super::is_noise(std::ffi::OsStr::new(".venv")));
    }

    #[test]
    fn is_noise_allows_normal_names() {
        assert!(!super::is_noise(std::ffi::OsStr::new("src")));
        assert!(!super::is_noise(std::ffi::OsStr::new("lib")));
        assert!(!super::is_noise(std::ffi::OsStr::new("tests")));
        assert!(!super::is_noise(std::ffi::OsStr::new("README.md")));
    }

    #[test]
    fn is_noise_allows_dotfiles_with_exceptions() {
        assert!(!super::is_noise(std::ffi::OsStr::new(".env")));
        assert!(!super::is_noise(std::ffi::OsStr::new(".gitignore")));
        assert!(!super::is_noise(std::ffi::OsStr::new(".dockerignore")));
    }

    #[test]
    fn is_noise_rejects_hidden_files() {
        assert!(super::is_noise(std::ffi::OsStr::new(".DS_Store")));
        assert!(super::is_noise(std::ffi::OsStr::new(".cache")));
    }

    // --- is_dangerous tests ---

    #[test]
    fn is_dangerous_detects_rm_rf_root() {
        assert!(super::is_dangerous("rm -rf /"));
        assert!(super::is_dangerous("rm -rf /*"));
    }

    #[test]
    fn is_dangerous_detects_dd_patterns() {
        assert!(super::is_dangerous("dd if=/dev/zero of=/dev/sda"));
        assert!(super::is_dangerous("dd if=/dev/random of=disk.img"));
    }

    #[test]
    fn is_dangerous_detects_shutdown_commands() {
        assert!(super::is_dangerous("shutdown now"));
        assert!(super::is_dangerous("reboot"));
    }

    #[test]
    fn is_dangerous_detects_piped_remote_execution() {
        // The pattern is substring match on "curl | sh" (with spaces as-is)
        assert!(super::is_dangerous("curl | sh"));
        assert!(super::is_dangerous("wget | bash"));
        assert!(super::is_dangerous("curl | sudo"));
    }

    #[test]
    fn is_dangerous_is_case_insensitive() {
        assert!(super::is_dangerous("RM -RF /"));
        assert!(super::is_dangerous("Reboot"));
    }

    #[test]
    fn is_dangerous_allows_safe_commands() {
        assert!(!super::is_dangerous("ls -la"));
        assert!(!super::is_dangerous("cargo build"));
        assert!(!super::is_dangerous("git status"));
        assert!(!super::is_dangerous("echo hello"));
    }

    // --- lexical_normalize tests ---

    #[test]
    fn lexical_normalize_removes_dot_components() {
        let path = std::path::Path::new("/home/user/./project/./file.txt");
        let normalized = super::lexical_normalize(path);
        assert_eq!(
            normalized,
            std::path::PathBuf::from("/home/user/project/file.txt")
        );
    }

    #[test]
    fn lexical_normalize_resolves_parent_dir() {
        let path = std::path::Path::new("/home/user/project/../other/file.txt");
        let normalized = super::lexical_normalize(path);
        assert_eq!(
            normalized,
            std::path::PathBuf::from("/home/user/other/file.txt")
        );
    }

    #[test]
    fn lexical_normalize_handles_mixed_dots() {
        let path = std::path::Path::new("/a/b/../c/./d");
        let normalized = super::lexical_normalize(path);
        assert_eq!(normalized, std::path::PathBuf::from("/a/c/d"));
    }

    #[test]
    fn lexical_normalize_preserves_clean_path() {
        let path = std::path::Path::new("/home/user/project");
        let normalized = super::lexical_normalize(path);
        assert_eq!(normalized, std::path::PathBuf::from("/home/user/project"));
    }

    #[test]
    fn lexical_normalize_empty_path() {
        let path = std::path::Path::new("");
        let normalized = super::lexical_normalize(path);
        assert_eq!(normalized, std::path::PathBuf::new());
    }

    // --- render_labeled_sections tests ---

    #[test]
    fn render_labeled_sections_formats_single_section() {
        let result =
            super::render_labeled_sections(vec![("Files".to_string(), "a.rs\nb.rs".to_string())]);
        assert_eq!(result, "## Files\na.rs\nb.rs");
    }

    #[test]
    fn render_labeled_sections_joins_multiple_sections() {
        let result = super::render_labeled_sections(vec![
            ("Section A".to_string(), "content a".to_string()),
            ("Section B".to_string(), "content b".to_string()),
        ]);
        assert!(result.contains("## Section A"));
        assert!(result.contains("content a"));
        assert!(result.contains("## Section B"));
        assert!(result.contains("content b"));
    }

    #[test]
    fn render_labeled_sections_shows_no_result_for_empty_body() {
        let result = super::render_labeled_sections(vec![("Empty".to_string(), "  ".to_string())]);
        assert!(result.contains("[无结果]"));
    }

    // --- rel helper tests ---

    #[test]
    fn rel_strips_prefix_when_matching() {
        let path = std::path::Path::new("/home/user/project/src/main.rs");
        let root = std::path::Path::new("/home/user/project");
        assert_eq!(super::rel(path, root), "src/main.rs");
    }

    #[test]
    fn rel_returns_full_path_when_prefix_does_not_match() {
        let path = std::path::Path::new("/other/path/file.txt");
        let root = std::path::Path::new("/home/user/project");
        assert_eq!(super::rel(path, root), "/other/path/file.txt");
    }

    // --- resolve_path tests ---

    #[test]
    fn resolve_path_allows_absolute_path_inside_workspace() {
        let workspace = temp_path("jkcodingagent-common-abs-path");
        fs::create_dir_all(&workspace).expect("create workspace");
        let context = tool_context(workspace.clone());

        let resolved =
            resolve_path(&context, workspace.to_string_lossy().as_ref()).expect("resolve");
        assert!(resolved.starts_with(workspace.canonicalize().expect("canonicalize")));

        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn resolve_path_rejects_absolute_path_outside_workspace() {
        let workspace = temp_path("jkcodingagent-common-abs-outside");
        fs::create_dir_all(&workspace).expect("create workspace");
        let context = tool_context(workspace.clone());

        let error = resolve_path(&context, "/etc/passwd").expect_err("reject outside");
        assert!(error.contains("禁止访问工作区之外"));

        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn resolve_path_allows_relative_path_inside_workspace() {
        let workspace = temp_path("jkcodingagent-common-rel-path");
        fs::create_dir_all(&workspace).expect("create workspace");
        let context = tool_context(workspace.clone());

        let resolved = resolve_path(&context, "src/main.rs").expect("resolve");
        assert!(resolved.starts_with(workspace.canonicalize().expect("canonicalize")));
        assert!(resolved.to_string_lossy().ends_with("src/main.rs"));

        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn resolve_path_unrestricted_allows_outside() {
        let workspace = temp_path("jkcodingagent-common-unrestricted");
        fs::create_dir_all(&workspace).expect("create workspace");
        let mut context = tool_context(workspace.clone());
        context.restrict_to_workspace = false;

        let resolved = resolve_path(&context, "/tmp/some-file.txt").expect("resolve");
        assert_eq!(resolved, std::path::PathBuf::from("/tmp/some-file.txt"));

        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn resolve_path_allows_extra_dir() {
        let workspace = temp_path("jkcodingagent-common-extra-workspace");
        let extra = temp_path("jkcodingagent-common-extra-dir");
        fs::create_dir_all(&workspace).expect("create workspace");
        fs::create_dir_all(&extra).expect("create extra dir");
        let mut context = tool_context(workspace.clone());
        context.extra_allowed_dirs = vec![extra.clone()];

        let resolved = resolve_path(&context, extra.to_string_lossy().as_ref()).expect("resolve");
        assert!(resolved.starts_with(extra.canonicalize().expect("canonicalize")));

        let _ = fs::remove_dir_all(workspace);
        let _ = fs::remove_dir_all(extra);
    }

    // --- collect_entries tests ---

    #[test]
    fn collect_entries_lists_files_and_dirs() {
        let dir = temp_path("jkcodingagent-common-collect");
        let sub = dir.join("subdir");
        fs::create_dir_all(&sub).expect("create subdir");
        fs::write(dir.join("file.txt"), "hello").expect("write file");
        fs::write(sub.join("nested.txt"), "world").expect("write nested");

        let mut entries = Vec::new();
        super::collect_entries(&dir, &dir, false, 100, &mut entries);

        assert!(entries.iter().any(|e| e.contains("[dir] subdir/")));
        assert!(entries.iter().any(|e| e.contains("[file] file.txt")));
        // Non-recursive should not include nested file
        assert!(!entries.iter().any(|e| e.contains("nested.txt")));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn collect_entries_recursive_lists_nested() {
        let dir = temp_path("jkcodingagent-common-collect-recursive");
        let sub = dir.join("subdir");
        fs::create_dir_all(&sub).expect("create subdir");
        fs::write(sub.join("nested.txt"), "world").expect("write nested");

        let mut entries = Vec::new();
        super::collect_entries(&dir, &dir, true, 100, &mut entries);

        assert!(entries.iter().any(|e| e.contains("nested.txt")));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn collect_entries_respects_max_entries() {
        let dir = temp_path("jkcodingagent-common-collect-max");
        fs::create_dir_all(&dir).expect("create dir");
        for i in 0..20 {
            fs::write(dir.join(format!("file{i}.txt")), "").expect("write file");
        }

        let mut entries = Vec::new();
        super::collect_entries(&dir, &dir, false, 5, &mut entries);

        // Should have at most 5 regular entries + 1 "... (5 entries shown)" marker
        assert!(entries.len() <= 6);
        assert!(entries.iter().any(|e| e.contains("entries shown")));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn collect_entries_skips_noise_dirs() {
        let dir = temp_path("jkcodingagent-common-collect-noise");
        let node_modules = dir.join("node_modules");
        let src = dir.join("src");
        fs::create_dir_all(&node_modules).expect("create node_modules");
        fs::create_dir_all(&src).expect("create src");
        fs::write(node_modules.join("lib.js"), "").expect("write lib");
        fs::write(src.join("main.rs"), "").expect("write main");

        let mut entries = Vec::new();
        super::collect_entries(&dir, &dir, true, 100, &mut entries);

        assert!(!entries.iter().any(|e| e.contains("node_modules")));
        assert!(entries.iter().any(|e| e.contains("src")));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn collect_entries_handles_nonexistent_dir() {
        let mut entries = Vec::new();
        super::collect_entries(
            std::path::Path::new("/nonexistent/path"),
            std::path::Path::new("/nonexistent/path"),
            false,
            100,
            &mut entries,
        );
        assert!(entries.is_empty());
    }
}
