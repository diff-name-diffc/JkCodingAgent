//! 动态模型与工具目录，以及节点运行前的稳定引用解析。

use std::collections::HashSet;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use serde::Serialize;

use super::pi_rpc::discover_extension_tools;
use super::types::{GraphHarnessCatalog, GraphHarnessModel, GraphHarnessTool, GraphNode};
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

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PiModelConfig {
    pub r#ref: String,
    pub url: String,
    pub api_key: String,
    pub model: String,
    pub category: String,
    pub alias: String,
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

    match discover_extension_tools(workspace).await {
        Ok((pi_tools, pi_diagnostics)) => {
            diagnostics.extend(pi_diagnostics);
            tools.extend(pi_tools.into_iter().map(|tool| GraphHarnessTool {
                source: "pi_extension".to_string(),
                name: tool.name,
                description: tool.description,
                provider: "pi".to_string(),
                category: "extension".to_string(),
                readonly: false,
                review_required: false,
            }));
        }
        Err(error) => diagnostics.push(format!("PI 扩展目录发现失败：{error:#}")),
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
    let available = aha_specs(registry, workspace);
    let mut host_tools = Vec::new();
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
        host_tools.push(PiHostToolSpec {
            name: spec.name.clone(),
            runtime_name: format!("aha__{}", sanitize_tool_name(&spec.name)),
            description: spec.description.clone(),
            parameters: spec.parameters.clone(),
        });
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
