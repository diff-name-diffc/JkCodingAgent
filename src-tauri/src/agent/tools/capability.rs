use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::agent::llm::ToolDefinition;

/// 某次执行被明确授予的宿主工具能力。
///
/// 能力集与模型可见的工具定义是两个概念：模型可以只看到一个运行时入口，
/// 而该入口只能代理这里列出的真实工具。集合一经构造即不可变，便于在并发
/// 执行期间共享同一份权限快照。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CapabilitySet {
    names: Arc<HashSet<String>>,
    /// None 表示该调用域未声明资源级写约束；Some(empty) 表示禁止所有
    /// write_file/edit_file。Graph 节点会把 expectedFiles 固化到这里，
    /// 因而模型可见的工具名不再等同于“可写整个工作区”。
    write_scopes: Option<Arc<Vec<PathBuf>>>,
}

impl CapabilitySet {
    pub fn new(names: impl IntoIterator<Item = String>) -> Self {
        Self {
            names: Arc::new(names.into_iter().collect()),
            write_scopes: None,
        }
    }

    /// 将文件写能力收窄到明确的工作区相对路径集合。非法 scope 被丢弃，
    /// 这是 fail-closed 收紧；调用方若传入的列表全部非法，最终效果是禁止写。
    pub fn restrict_writes_to(mut self, scopes: impl IntoIterator<Item = String>) -> Self {
        let scopes = scopes
            .into_iter()
            .filter_map(|scope| normalize_relative_path(&scope))
            .collect::<Vec<_>>();
        self.write_scopes = Some(Arc::new(scopes));
        self
    }

    pub fn has_write_restriction(&self) -> bool {
        self.write_scopes.is_some()
    }

    pub(crate) fn write_scopes_for_review(&self) -> Option<Vec<String>> {
        self.write_scopes.as_ref().map(|scopes| {
            scopes
                .iter()
                .map(|scope| scope.to_string_lossy().into_owned())
                .collect()
        })
    }

    pub fn permits_workspace_write(&self, workspace: &Path, raw_path: &str) -> bool {
        let Some(scopes) = &self.write_scopes else {
            return true;
        };
        let candidate = Path::new(raw_path);
        let relative = if candidate.is_absolute() {
            let Ok(relative) = candidate.strip_prefix(workspace) else {
                return false;
            };
            normalize_relative_path(&relative.to_string_lossy())
        } else {
            normalize_relative_path(raw_path)
        };
        relative.is_some_and(|relative| scopes.iter().any(|scope| scope == &relative))
    }

    pub fn from_definitions(definitions: &[ToolDefinition]) -> Self {
        Self::new(
            definitions
                .iter()
                .map(|definition| definition.function.name.clone()),
        )
    }

    pub fn contains(&self, name: &str) -> bool {
        self.names.contains(name)
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.names.len()
    }
}

impl From<HashSet<String>> for CapabilitySet {
    fn from(names: HashSet<String>) -> Self {
        Self {
            names: Arc::new(names),
            write_scopes: None,
        }
    }
}

fn normalize_relative_path(raw: &str) -> Option<PathBuf> {
    let raw = raw.trim().replace('\\', "/");
    if raw.is_empty() || raw.starts_with('/') || raw.as_bytes().get(1) == Some(&b':') {
        return None;
    }
    let mut normalized = PathBuf::new();
    for component in raw.split('/') {
        match component {
            "" | "." => {}
            ".." => return None,
            component if component.contains('\0') => return None,
            component => normalized.push(component),
        }
    }
    (!normalized.as_os_str().is_empty()).then_some(normalized)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::CapabilitySet;
    use crate::agent::llm::{ToolDefinition, ToolFunctionDefinition};

    fn definition(name: &str) -> ToolDefinition {
        ToolDefinition {
            kind: "function".to_string(),
            function: ToolFunctionDefinition {
                name: name.to_string(),
                description: String::new(),
                parameters: json!({ "type": "object" }),
            },
        }
    }

    #[test]
    fn derives_an_immutable_deduplicated_grant_from_definitions() {
        let capabilities = CapabilitySet::from_definitions(&[
            definition("read_file"),
            definition("grep"),
            definition("read_file"),
        ]);

        assert_eq!(capabilities.len(), 2);
        assert!(capabilities.contains("read_file"));
        assert!(capabilities.contains("grep"));
        assert!(!capabilities.contains("exec"));
    }

    #[test]
    fn write_scopes_are_exact_and_workspace_relative() {
        let workspace = std::path::Path::new("/workspace");
        let capabilities = CapabilitySet::new(["write_file".to_string()])
            .restrict_writes_to(["src/a.rs".to_string(), "./src/b.rs".to_string()]);

        assert!(capabilities.has_write_restriction());
        assert!(capabilities.permits_workspace_write(workspace, "src/a.rs"));
        assert!(capabilities.permits_workspace_write(workspace, "/workspace/src/b.rs"));
        assert!(!capabilities.permits_workspace_write(workspace, "src/c.rs"));
        assert!(!capabilities.permits_workspace_write(workspace, "../outside.rs"));
        assert!(!capabilities.permits_workspace_write(workspace, "/outside/a.rs"));
    }
}
