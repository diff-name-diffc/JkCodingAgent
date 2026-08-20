//! 动态模型与工具目录，以及节点运行前的稳定引用解析。

use std::collections::HashSet;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use serde::Serialize;

use super::types::{
    BaseToolGroup, GraphHarnessCatalog, GraphHarnessModel, GraphHarnessTool, GraphNode,
};
use crate::agent::db::settings::ModelLibraryEntry;
use crate::agent::db::AhaSettingsV2;
use crate::agent::tools::{ToolRegistry, ToolSafety, ToolSpec};

const EXCLUDED_AHA_TOOLS: &[&str] = &[
    "read_file",
    "write_file",
    "edit_file",
    "list_dir",
    "glob",
    "grep",
    "exec",
    "local_zsh",
    "message",
    "submit_graph",
    "list_sub_agents",
    "call_sub_agent",
    "notify_user_progress",
];

/// PI 只保留模型熟悉的短工具名；真正的能力名、Schema 与执行策略全部来自
/// Rust ToolRegistry。sidecar 只能把调用数据回传，不能直接持有文件或 shell
/// 能力。数组顺序也是模型看到的稳定工具顺序。
const READ_ONLY_BASE_TOOLS: &[(&str, &str)] = &[
    ("read", "read_file"),
    ("grep", "grep"),
    ("find", "glob"),
    ("ls", "list_dir"),
];
const CODING_BASE_TOOLS: &[(&str, &str)] = &[
    ("bash", "exec"),
    ("edit", "edit_file"),
    ("write", "write_file"),
];

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PiModelConfig {
    pub r#ref: String,
    pub url: String,
    /// 明文 API Key。任何 Serialize 路径（日志、调试输出、DTO 下发前端）都
    /// 跳过该字段；唯一出口是 `sidecar_value`——节点执行时经 stdin 传给 sidecar。
    /// Clone 仅用于运行器内部向节点执行上下文传递密钥，不得用于对外序列化。
    #[serde(skip_serializing)]
    pub api_key: String,
    pub model: String,
    pub category: String,
    pub alias: String,
}

impl PiModelConfig {
    /// 明文密钥的唯一出口：sidecar start 消息的 model 字段。
    /// 手工构建而非复用派生 Serialize，避免派生实现把密钥带到
    /// 其他序列化点（派生侧 api_key 已 skip_serializing）。
    pub(crate) fn sidecar_value(&self) -> serde_json::Value {
        serde_json::json!({
            "ref": self.r#ref,
            "url": self.url,
            "apiKey": self.api_key,
            "model": self.model,
            "category": self.category,
            "alias": self.alias,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PiHostToolSpec {
    pub name: String,
    pub runtime_name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Clone)]
pub(crate) struct ResolvedNodeHarness {
    pub model: PiModelConfig,
    pub model_label: String,
    pub host_tools: Vec<PiHostToolSpec>,
}

pub(crate) async fn build_harness_catalog(
    workspace: &Path,
    settings: &AhaSettingsV2,
    registry: &ToolRegistry,
) -> GraphHarnessCatalog {
    let mut diagnostics = Vec::new();
    for entry in settings
        .model_library
        .iter()
        .filter(|entry| is_supported_graph_model(entry))
    {
        if entry.id.trim().is_empty()
            || entry.model.trim().is_empty()
            || entry.url.trim().is_empty()
        {
            diagnostics.push(format!(
                "模型 '{}' 配置不完整，缺少稳定 ID、模型名或 URL，已从执行图目录排除",
                if entry.alias.trim().is_empty() {
                    entry.model.as_str()
                } else {
                    entry.alias.as_str()
                }
            ));
        }
    }
    let models = settings
        .model_library
        .iter()
        .filter(|entry| is_complete_graph_model(entry))
        .map(|entry| GraphHarnessModel {
            id: entry.id.clone(),
            label: if entry.alias.trim().is_empty() {
                entry.model.clone()
            } else {
                entry.alias.clone()
            },
            model: entry.model.clone(),
            category: entry.category.clone(),
            capabilities: model_capabilities(&entry.model, &entry.category),
        })
        .collect::<Vec<_>>();

    if models.is_empty() {
        diagnostics.push("没有可用于执行图的已启用对话/视觉模型".to_string());
    }

    // Aha 工具的安全元数据直接来自注册表 spec：readonly 取 access.readonly，
    // review_required 按 spec.safety 重建（非 Safe 一律需审查）。
    let mut tools = aha_specs(registry, workspace)
        .into_iter()
        .map(|spec| GraphHarnessTool {
            source: "aha".to_string(),
            name: spec.name,
            description: spec.description,
            provider: spec.provider,
            category: spec.category.as_str().to_string(),
            readonly: spec.access.readonly,
            review_required: spec.safety != ToolSafety::Safe,
        })
        .collect::<Vec<_>>();

    if let Some(diagnostic) = disabled_project_extension_diagnostic(workspace).await {
        diagnostics.push(diagnostic);
    }
    tools.sort_by(|left, right| {
        (left.source.as_str(), left.name.as_str())
            .cmp(&(right.source.as_str(), right.name.as_str()))
    });
    let mut seen = HashSet::new();
    tools.retain(|tool| {
        if seen.insert((tool.source.clone(), tool.name.clone())) {
            true
        } else {
            diagnostics.push(format!("工具重名已排除：{}:{}", tool.source, tool.name));
            false
        }
    });
    GraphHarnessCatalog {
        models,
        tools,
        diagnostics,
    }
}

pub(crate) fn resolve_node_harness(
    node: &GraphNode,
    settings: &AhaSettingsV2,
    registry: &ToolRegistry,
    workspace: &Path,
) -> Result<ResolvedNodeHarness> {
    let entry = settings
        .model_library
        .iter()
        .find(|entry| entry.id == node.model_ref && is_supported_graph_model(entry))
        .ok_or_else(|| {
            anyhow!(
                "节点 '{}' 的模型 '{}' 不存在、已禁用或分类不受支持",
                node.id,
                node.model_ref
            )
        })?;
    if entry.url.trim().is_empty() || entry.model.trim().is_empty() {
        return Err(anyhow!("节点 '{}' 的模型配置缺少 URL 或模型名", node.id));
    }
    reject_pi_extensions(node)?;

    let available = aha_specs(registry, workspace);
    let mut host_tools = Vec::new();
    let mut runtime_names = HashSet::new();

    for (runtime_name, capability_name) in base_tool_aliases(node.base_tool_group) {
        let spec = registry
            .spec_by_name(workspace, capability_name, false)
            .with_context(|| {
                format!(
                    "节点 '{}' 所需的基础宿主能力 '{}' 未注册",
                    node.id, capability_name
                )
            })?;
        push_host_tool(
            &node.id,
            &mut host_tools,
            &mut runtime_names,
            spec,
            runtime_name.to_string(),
        )?;
    }

    for selected in node
        .special_tools
        .iter()
        .filter(|tool| tool.source == "aha")
    {
        let spec = available
            .iter()
            .find(|spec| spec.name == selected.name)
            .with_context(|| {
                format!(
                    "节点 '{}' 的 Aha 工具 '{}' 已不可用",
                    node.id, selected.name
                )
            })?;
        let runtime_name = format!("aha__{}", sanitize_tool_name(&spec.name));
        push_host_tool(
            &node.id,
            &mut host_tools,
            &mut runtime_names,
            spec.clone(),
            runtime_name,
        )?;
    }
    let label = if entry.alias.trim().is_empty() {
        entry.model.clone()
    } else {
        entry.alias.clone()
    };
    Ok(ResolvedNodeHarness {
        model: PiModelConfig {
            r#ref: entry.id.clone(),
            url: entry.url.clone(),
            api_key: entry.api_key.clone(),
            model: entry.model.clone(),
            category: entry.category.clone(),
            alias: entry.alias.clone(),
        },
        model_label: label,
        host_tools,
    })
}

fn base_tool_aliases(group: BaseToolGroup) -> impl Iterator<Item = (&'static str, &'static str)> {
    READ_ONLY_BASE_TOOLS.iter().copied().chain(
        (group == BaseToolGroup::Coding)
            .then_some(CODING_BASE_TOOLS)
            .into_iter()
            .flatten()
            .copied(),
    )
}

fn push_host_tool(
    node_id: &str,
    host_tools: &mut Vec<PiHostToolSpec>,
    runtime_names: &mut HashSet<String>,
    spec: ToolSpec,
    runtime_name: String,
) -> Result<()> {
    // fail-closed：不同工具名清洗后可能塌缩为同一 runtime_name
    // （如 foo-bar 与 foo_bar），或扩展工具试图占用 read/bash 等基础别名。
    if !runtime_names.insert(runtime_name.clone()) {
        return Err(anyhow!(
            "节点 '{node_id}' 的宿主工具运行名冲突：{runtime_name}（能力 '{}'）",
            spec.name
        ));
    }
    host_tools.push(PiHostToolSpec {
        name: spec.name,
        runtime_name,
        description: spec.description,
        parameters: spec.parameters,
    });
    Ok(())
}

fn reject_pi_extensions(node: &GraphNode) -> Result<()> {
    if let Some(extension) = node
        .special_tools
        .iter()
        .find(|tool| tool.source == "pi_extension")
    {
        return Err(anyhow!(
            "节点 '{}' 仍引用 PI 扩展工具 '{}'；执行图已禁用可执行扩展，请迁移为经 CapabilityBroker 托管的 Aha 工具",
            node.id,
            extension.name
        ));
    }
    Ok(())
}

async fn disabled_project_extension_diagnostic(workspace: &Path) -> Option<String> {
    let extensions = workspace.join(".jkcodingagent/pi-agent/extensions");
    let display = extensions.display().to_string();
    match tokio::task::spawn_blocking(move || std::fs::symlink_metadata(extensions)).await {
        Ok(Ok(_)) => Some(format!(
            "检测到项目 PI 扩展目录 {display}；为保证所有工具调用都经过 CapabilityBroker，执行图已禁用这些可执行扩展，请迁移为 Aha 工具"
        )),
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => None,
        Ok(Err(error)) => Some(format!(
            "无法检查项目 PI 扩展目录 {display}，已按 fail-closed 禁用扩展：{error}"
        )),
        Err(error) => Some(format!(
            "检查项目 PI 扩展目录的任务失败，已按 fail-closed 禁用扩展：{error}"
        )),
    }
}

fn is_supported_graph_model(entry: &ModelLibraryEntry) -> bool {
    entry.enabled && matches!(entry.category.as_str(), "text" | "vision")
}

fn is_complete_graph_model(entry: &ModelLibraryEntry) -> bool {
    is_supported_graph_model(entry)
        && !entry.id.trim().is_empty()
        && !entry.model.trim().is_empty()
        && !entry.url.trim().is_empty()
}

fn aha_specs(registry: &ToolRegistry, workspace: &Path) -> Vec<ToolSpec> {
    registry
        .specs_for_workspace(workspace, Option::<std::iter::Empty<&str>>::None, true)
        .into_iter()
        .filter(|spec| !EXCLUDED_AHA_TOOLS.contains(&spec.name.as_str()))
        .filter(|spec| {
            !matches!(
                spec.category.as_str(),
                "filesystem" | "search" | "shell" | "sub_agent"
            )
        })
        .collect()
}

fn sanitize_tool_name(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn model_capabilities(model: &str, category: &str) -> Vec<String> {
    let lower = model.to_ascii_lowercase();
    let mut result = Vec::new();
    if category == "vision" {
        result.push("vision".to_string());
    }
    if ["reason", "thinking", "o1", "o3", "r1", "gpt-5"]
        .iter()
        .any(|tag| lower.contains(tag))
    {
        result.push("reasoning".to_string());
    }
    if ["128k", "200k", "256k", "long", "kimi", "gemini", "claude"]
        .iter()
        .any(|tag| lower.contains(tag))
    {
        result.push("long_context".to_string());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_tool_groups_map_only_to_broker_capabilities() {
        assert_eq!(
            base_tool_aliases(BaseToolGroup::ReadOnly).collect::<Vec<_>>(),
            vec![
                ("read", "read_file"),
                ("grep", "grep"),
                ("find", "glob"),
                ("ls", "list_dir"),
            ]
        );
        assert_eq!(
            base_tool_aliases(BaseToolGroup::Coding).collect::<Vec<_>>(),
            vec![
                ("read", "read_file"),
                ("grep", "grep"),
                ("find", "glob"),
                ("ls", "list_dir"),
                ("bash", "exec"),
                ("edit", "edit_file"),
                ("write", "write_file"),
            ]
        );
    }

    #[test]
    fn persisted_pi_extension_selection_is_rejected_loudly() {
        let node = GraphNode {
            id: "node".into(),
            title: "node".into(),
            role: String::new(),
            model_ref: "model".into(),
            base_tool_group: BaseToolGroup::ReadOnly,
            special_tools: vec![super::super::types::GraphToolRef {
                source: "pi_extension".into(),
                name: "unsafe".into(),
            }],
            task: "task".into(),
            depends_on: Vec::new(),
            inject_state_keys: Vec::new(),
            output_key: "output".into(),
            expected_files: Vec::new(),
            export_policy: Default::default(),
        };

        let error = reject_pi_extensions(&node).unwrap_err().to_string();
        assert!(error.contains("已禁用可执行扩展"));
        assert!(error.contains("unsafe"));
    }

    #[tokio::test]
    async fn project_extension_directory_is_only_reported_never_loaded() {
        let workspace = std::env::temp_dir().join(format!(
            "aha-graph-extension-policy-{}",
            uuid::Uuid::new_v4()
        ));
        let extension_dir = workspace.join(".jkcodingagent/pi-agent/extensions");
        std::fs::create_dir_all(&extension_dir).unwrap();
        std::fs::write(
            extension_dir.join("must-not-run.ts"),
            "throw new Error('must not execute');",
        )
        .unwrap();

        let diagnostic = disabled_project_extension_diagnostic(&workspace)
            .await
            .expect("检测到扩展目录时必须给出诊断");
        assert!(diagnostic.contains("已禁用这些可执行扩展"));
        assert!(diagnostic.contains("CapabilityBroker"));

        std::fs::remove_dir_all(workspace).unwrap();
    }
}
