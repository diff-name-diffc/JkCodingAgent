//! 工具公共助手：参数提取、路径沙箱、schema 注入。
//! 移植自旧 `agent/tools/builtin/common.rs`；`resolve_path` 的入参由
//! `ToolContext` 改为显式沙箱参数（构造期依赖见 `deps.rs`）。

use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde_json::{json, Value};

pub(crate) use super::super::tool_result::DEFAULT_FORCE_COMPRESS_AFTER_CHARS;
/// 命令执行类工具的压缩触发阈值（高于 8000 内联截断线，截断兜不住才摘要）。
pub(crate) const COMMAND_FORCE_COMPRESS_AFTER_CHARS: usize = 12_000;

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

pub(crate) fn string_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key)?.as_str().map(str::to_string)
}

pub(crate) fn string_array_arg(args: &Value, key: &str) -> Option<Vec<String>> {
    let values = args.get(key)?.as_array()?;
    Some(
        values
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect(),
    )
}

pub(crate) fn non_empty_string_array_arg(args: &Value, key: &str) -> Option<Vec<String>> {
    let values = string_array_arg(args, key)?
        .into_iter()
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>();
    (!values.is_empty()).then_some(values)
}

pub(crate) fn string_list_arg(
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

pub(crate) fn usize_arg(args: &Value, key: &str) -> Option<usize> {
    args.get(key)?.as_u64().map(|value| value as usize)
}

pub(crate) fn u64_arg(args: &Value, key: &str) -> Option<u64> {
    args.get(key)?.as_u64()
}

pub(crate) fn boolish_arg(args: &Value, key: &str) -> Option<bool> {
    let value = args.get(key)?;
    if let Some(flag) = value.as_bool() {
        return Some(flag);
    }
    value
        .as_str()
        .map(|flag| flag.eq_ignore_ascii_case("true"))
}

/// 为工具 schema 注入 `compress` / `compress_intent` 参数。
///
/// `force_compress_after_chars` 必须与 runtime 侧该工具的结果策略
/// （`loop::surface::RigToolSurface::with_policy` 挂载的阈值）一致——文案向
/// 模型声明的阈值与运行时实际阈值出现偏差时，模型无法正确判断何时值得
/// 声明压缩（历史教训：文案写 5000、实际 1000，导致 SSH 输出几乎每次都被压缩）。
pub(crate) fn with_compression_parameters(
    mut schema: Value,
    default_compress: bool,
    force_compress_after_chars: usize,
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
                "是否允许对超长工具结果进行语义压缩。只有 compress=true 且原始结果超过 {force_compress_after_chars} 字符时才会调用摘要模型，按 compress_intent 只提取相关重点；低于该阈值时即使声明压缩也直接返回原文，绝不摘要。compress=false 永不摘要。未摘要的结果超过内联字符上限时会明确标记并截断（普通工具 8000；读取类工具默认 10000，显式 offset/limit 分页读取 20000），完整原文保留在工具产物中。{tool_specific_guidance}"
            ),
            "default": default_compress
        }),
    );
    properties.insert(
        "compress_intent".to_string(),
        json!({
            "type": "string",
            "description": "当 compress=true 时，用一句话具体描述要从结果中确认什么；摘要只返回与该意图直接相关的重点，不会复述全文。意图越具体，摘要越精准。例如：'确认部署是否成功及失败时的报错行'。"
        }),
    );
    schema
}

/// 路径沙箱解析：相对路径以 `workspace` 为基准；`restrict_to_workspace`
/// 开启时拒绝越出 workspace 与 `extra_allowed_dirs` 白名单的路径；
/// 应用托管敏感路径（见 `is_protected_agent_path`）一律拒绝。
pub(crate) fn resolve_path(
    workspace: &Path,
    restrict_to_workspace: bool,
    extra_allowed_dirs: &[PathBuf],
    raw_path: &str,
) -> Result<PathBuf, String> {
    let raw = PathBuf::from(raw_path);
    let joined = if raw.is_absolute() {
        raw
    } else {
        workspace.join(raw)
    };
    let normalized = lexical_normalize(&joined);
    if is_protected_agent_path(&normalized) {
        return Err(protected_path_error(raw_path));
    }

    if restrict_to_workspace {
        let candidate = canonicalize_existing_prefix(&normalized)?;
        // 规范化后再查一次：拦截经符号链接指向敏感目录的别名路径。
        if is_protected_agent_path(&candidate) {
            return Err(protected_path_error(raw_path));
        }

        let workspace = workspace
            .canonicalize()
            .map_err(|error| format!("错误：解析工作区路径失败：{error}"))?;

        let in_workspace = candidate.starts_with(&workspace);
        let in_extra = extra_allowed_dirs.iter().any(|dir| {
            dir.canonicalize()
                .is_ok_and(|canonical| candidate.starts_with(canonical))
        });

        if !in_workspace && !in_extra {
            let extra = extra_allowed_dirs
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join("、");
            return Err(format!(
                "错误：禁止访问工作区之外的路径：{raw_path}。当前工作区：{}；额外授权路径：{}。相对路径以当前工作区为基准。",
                workspace.display(), if extra.is_empty() { "无" } else { &extra }
            ));
        }
        return Ok(candidate);
    }

    // fail-closed 兜底：即便配置关闭了工作区限制，也不裸放行——路径至少经过
    // 词法规范化（抵消 `.`/`..`，前导无法抵消的 `..` 会被保留而非静默吞掉），
    // 并显式记录警告，便于审计该不安全配置实际放行了哪些路径。
    eprintln!(
        "[agent] 警告：restrict_to_workspace=false，未限制路径访问：{raw_path} -> {}",
        normalized.display()
    );
    Ok(normalized)
}

/// 应用托管的敏感路径清单：Agent 文件工具一律拒绝访问，优先级高于工作区边界
/// 与 extra_allowed_dirs 白名单。
///
/// - `~/.jkcodingagent/jkbot.sqlite3`（含 -wal/-shm）：全局配置库。SSH 服务器
///   凭据 / 主机密钥 / 模型 apiKey 均存于此，二进制读出即泄漏明文；
/// - `~/.jkcodingagent/ssh-tools/` 与 `<任意目录>/.jkcodingagent/local_env/ssh/`：
///   旧版按项目分键存放的 SSH 明文凭据仓库。现行实现已不再写入这些路径，但
///   旧机器上可能残留，一律拒绝读取兜底；
/// - `~/.jkcodingagent/ssh-memos/`：SSH 运维备忘录（含服务器部署路径、操作
///   方式等敏感运维信息）。直连读写会绕过 memo 模块的字符上限与段落纪律，
///   必须经 ssh_memo_* 工具访问；
/// - `<任意目录>/.jkcodingagent/mcp.json`：MCP server 配置（可能内嵌密钥环境变量）。
///
/// 注意不能扩大到整个 `.jkcodingagent`：`local_env/zsh/` 是 local_zsh 工具声明的
/// 合法产物目录，Agent 需要读写其中的文件。
fn is_protected_agent_path(candidate: &Path) -> bool {
    if let Some(home) = dirs::home_dir() {
        let root = home.join(".jkcodingagent");
        if candidate.starts_with(root.join("jkbot.sqlite3"))
            || candidate.starts_with(root.join("jkbot.sqlite3-wal"))
            || candidate.starts_with(root.join("jkbot.sqlite3-shm"))
            || candidate.starts_with(root.join("ssh-tools"))
            || candidate.starts_with(root.join("ssh-memos"))
        {
            return true;
        }
    }
    let components: Vec<&std::ffi::OsStr> = candidate.iter().collect();
    for (index, component) in components.iter().enumerate() {
        if *component != std::ffi::OsStr::new(".jkcodingagent") {
            continue;
        }
        let rest = &components[index + 1..];
        if rest.first() == Some(&std::ffi::OsStr::new("mcp.json"))
            || has_path_prefix(rest, &["local_env", "ssh"])
        {
            return true;
        }
    }
    false
}

/// `rest` 是否以 `prefix` 给定的路径组件序列开头（含恰好等于前缀本身）。
fn has_path_prefix(rest: &[&std::ffi::OsStr], prefix: &[&str]) -> bool {
    rest.len() >= prefix.len()
        && prefix
            .iter()
            .zip(rest)
            .all(|(name, component)| *name == std::ffi::OsStr::new(component))
}

fn protected_path_error(raw_path: &str) -> String {
    format!("错误：禁止访问应用托管的敏感路径（SSH 凭据 / MCP 配置）：{raw_path}")
}

pub(crate) fn canonicalize_existing_prefix(path: &Path) -> Result<PathBuf, String> {
    let mut missing_components = Vec::new();
    let mut cursor = path;

    loop {
        match fs::symlink_metadata(cursor) {
            Ok(_) => {
                let mut resolved = cursor
                    .canonicalize()
                    .map_err(|error| format!("错误：解析路径失败：{error}"))?;
                for component in missing_components.iter().rev() {
                    resolved.push(component);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = cursor.file_name() else {
                    return Err(format!("错误：解析路径失败：{}", path.display()));
                };
                missing_components.push(name.to_os_string());
                cursor = cursor
                    .parent()
                    .ok_or_else(|| format!("错误：解析路径失败：{}", path.display()))?;
            }
            Err(error) => return Err(format!("错误：读取路径元数据失败：{error}")),
        }
    }
}

pub(crate) fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // 栈顶是普通分量时抵消；栈顶已是 `..` 或为空时保留该分量——
                // 否则 `../foo` 会被静默归一化成 `foo`，路径语义被改变。
                // 绝对路径下栈内仅剩根目录时 pop 为空操作，等价 POSIX `/.. == /`，
                // 因此不需要（也不应该）在根之上再补 `..`。
                let keep = match normalized.components().next_back() {
                    Some(Component::ParentDir) => true,
                    Some(_) => false,
                    None => !normalized.is_absolute(),
                };
                if keep {
                    normalized.push(Component::ParentDir.as_os_str());
                } else {
                    normalized.pop();
                }
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

pub(crate) fn render_labeled_sections(sections: Vec<(String, String)>) -> String {
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

pub(crate) fn is_noise(name: &std::ffi::OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return true;
    };
    NOISE.contains(&name)
        || (name.starts_with('.') && !matches!(name, ".env" | ".gitignore" | ".dockerignore"))
}

pub(crate) fn rel(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

/// 整数尺寸参数校验：未提供返回 None；提供但超出 256..=4096 返回「错误：」报错，
/// 避免 u64→u32 静默截断或把超大尺寸原样传给外部模型。
pub(crate) fn bounded_dimension_arg(args: &Value, key: &str) -> Result<Option<u32>, String> {
    let Some(value) = args.get(key).and_then(Value::as_u64) else {
        return Ok(None);
    };
    if !(256..=4096).contains(&value) {
        return Err(format!("错误：{key} 超出支持范围（256-4096）：{value}"));
    }
    Ok(Some(value as u32))
}

#[cfg(test)]
mod tests {
    use super::{bounded_dimension_arg, is_protected_agent_path, lexical_normalize};
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn lexical_normalize_preserves_leading_parent_dirs() {
        assert_eq!(lexical_normalize(Path::new("../foo")), Path::new("../foo"));
        assert_eq!(
            lexical_normalize(Path::new("../../foo/bar")),
            Path::new("../../foo/bar")
        );
        assert_eq!(lexical_normalize(Path::new("a/../b")), Path::new("b"));
        assert_eq!(lexical_normalize(Path::new("/a/../../b")), Path::new("/b"));
        assert_eq!(lexical_normalize(Path::new("/../foo")), Path::new("/foo"));
        assert_eq!(lexical_normalize(Path::new("./a/./b")), Path::new("a/b"));
    }

    #[test]
    fn protected_agent_paths_are_denied() {
        let home = dirs::home_dir().expect("home");
        assert!(is_protected_agent_path(
            &home.join(".jkcodingagent/jkbot.sqlite3")
        ));
        assert!(is_protected_agent_path(
            &home.join(".jkcodingagent/jkbot.sqlite3-wal")
        ));
        assert!(is_protected_agent_path(
            &home.join(".jkcodingagent/ssh-tools/keys.json")
        ));
        assert!(is_protected_agent_path(
            &home.join(".jkcodingagent/ssh-memos/prod-web.md")
        ));
        assert!(is_protected_agent_path(Path::new(
            "/tmp/ws/.jkcodingagent/local_env/ssh"
        )));
        assert!(is_protected_agent_path(Path::new(
            "/tmp/ws/.jkcodingagent/local_env/ssh/id_rsa"
        )));
        assert!(is_protected_agent_path(Path::new(
            "/tmp/ws/.jkcodingagent/mcp.json"
        )));
    }

    #[test]
    fn local_env_zsh_and_memory_stay_accessible() {
        assert!(!is_protected_agent_path(Path::new(
            "/tmp/ws/.jkcodingagent/local_env/zsh/run-1/out.txt"
        )));
        assert!(!is_protected_agent_path(Path::new(
            "/home/user/.jkcodingagent/memory/notes.md"
        )));
        assert!(!is_protected_agent_path(Path::new(
            "/home/user/.jkcodingagent/skills"
        )));
        assert!(!is_protected_agent_path(Path::new("/tmp/ws/src/main.rs")));
        assert!(!is_protected_agent_path(Path::new(
            "/tmp/ws/ssh-tools/notes.txt"
        )));
    }

    #[test]
    fn bounded_dimension_arg_rejects_out_of_range_values() {
        assert_eq!(bounded_dimension_arg(&json!({}), "width").unwrap(), None);
        assert_eq!(
            bounded_dimension_arg(&json!({"width": 1328})).unwrap(),
            Some(1328)
        );
        assert!(bounded_dimension_arg(&json!({"width": 0})).is_err());
        assert!(bounded_dimension_arg(&json!({"width": 100000})).is_err());
        assert!(bounded_dimension_arg(&json!({"width": 1u64 << 40})).is_err());
        assert!(bounded_dimension_arg(&json!({"width": 256})).is_ok());
        assert!(bounded_dimension_arg(&json!({"width": 4096})).is_ok());
    }
}
