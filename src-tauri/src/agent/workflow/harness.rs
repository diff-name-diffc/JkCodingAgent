//! ACP 执行器（claude-agent-acp）的静态模型目录，以及节点运行前的稳定引用解析。
//!
//! 工作流定义 v4 起模型目录不再引用应用模型库：节点执行器是 claude-agent-acp
//! 子进程，模型选择走 ACP 会话的 configOptions（`default` 不设，继承 Claude
//! 配置）。目录为静态四值表，校验（validate）与编排器提示词共用同一来源。

use anyhow::{anyhow, Result};

use super::types::{WorkflowHarnessCatalog, WorkflowHarnessModel, WorkflowNode};

/// ACP 会话权限模式（映射 Claude Code 权限模式的 mode id）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PermissionMode {
    /// bypassPermissions：Agent 全权限运行，常规操作不再产生权限请求；
    /// 仍浮出到客户端的请求（bypass 免疫的安全检查、显式 ask 规则）由全局
    /// 权限审查 AI 裁决。所有节点的默认模式（执行器以非 root 运行时
    /// adapter 默认放开 bypass，无需额外启动参数）。
    BypassPermissions,
    /// plan：先计划后执行。复杂改造类节点由编排器标记 usePlanMode 启用；
    /// 计划完成后客户端批准退出并统一切回 bypassPermissions。
    Plan,
}

impl PermissionMode {
    pub(crate) fn mode_id(self) -> &'static str {
        match self {
            Self::BypassPermissions => "bypassPermissions",
            Self::Plan => "plan",
        }
    }
}

/// 节点运行前解析出的稳定执行配置。
#[derive(Debug, Clone)]
pub(crate) struct ResolvedNodeHarness {
    /// ACP 模型目录 id（default/sonnet/opus/haiku）；`default` 表示不下发模型
    /// 配置项，继承 Claude 登录态的默认模型。
    pub model_id: String,
    pub model_label: String,
    pub permission_mode: PermissionMode,
}

/// ACP 模型目录条目（静态表）。`id` 同时作为会话 configOptions 里匹配用的
/// 关键字（`default`：不设置模型选项）。
struct AcpModelSpec {
    id: &'static str,
    label: &'static str,
    description: &'static str,
}

const ACP_MODELS: &[AcpModelSpec] = &[
    AcpModelSpec {
        id: "default",
        label: "默认（继承 Claude 配置）",
        description: "不设置模型选项，沿用 Claude 登录态的默认模型",
    },
    AcpModelSpec {
        id: "sonnet",
        label: "Claude Sonnet",
        description: "均衡型，适合大多数编码与调研节点",
    },
    AcpModelSpec {
        id: "opus",
        label: "Claude Opus",
        description: "最强推理，适合复杂改造与验收节点",
    },
    AcpModelSpec {
        id: "haiku",
        label: "Claude Haiku",
        description: "轻量快速，适合简单只读节点",
    },
];

fn acp_model_spec(model_id: &str) -> Option<&'static AcpModelSpec> {
    ACP_MODELS.iter().find(|spec| spec.id == model_id)
}

/// 构建 Harness 目录：静态 ACP 模型表。
pub(crate) fn build_harness_catalog() -> WorkflowHarnessCatalog {
    WorkflowHarnessCatalog {
        models: ACP_MODELS
            .iter()
            .map(|spec| WorkflowHarnessModel {
                id: spec.id.to_string(),
                label: spec.label.to_string(),
                model: spec.description.to_string(),
                category: "acp".to_string(),
                capabilities: vec!["agent".to_string()],
            })
            .collect(),
        diagnostics: Vec::new(),
    }
}

/// 解析节点 Harness：modelRef → ACP 目录条目 + usePlanMode → 权限模式。
/// baseToolGroup 不再决定权限模式（read_only 的写约束由输入软提示承载）。
pub(crate) fn resolve_node_harness(node: &WorkflowNode) -> Result<ResolvedNodeHarness> {
    let spec = acp_model_spec(node.model_ref.trim()).ok_or_else(|| {
        anyhow!(
            "节点 '{}' 的模型 '{}' 不在 ACP 目录中（可选：{}）",
            node.id,
            node.model_ref,
            ACP_MODELS
                .iter()
                .map(|spec| spec.id)
                .collect::<Vec<_>>()
                .join(" / ")
        )
    })?;
    Ok(ResolvedNodeHarness {
        model_id: spec.id.to_string(),
        model_label: spec.label.to_string(),
        permission_mode: if node.use_plan_mode {
            PermissionMode::Plan
        } else {
            PermissionMode::BypassPermissions
        },
    })
}

#[cfg(test)]
mod tests {
    use super::super::types::BaseToolGroup;
    use super::*;

    fn node(model_ref: &str, group: BaseToolGroup) -> WorkflowNode {
        WorkflowNode {
            id: "n1".into(),
            title: "n1".into(),
            role: String::new(),
            model_ref: model_ref.into(),
            base_tool_group: group,
            task: "task".into(),
            depends_on: vec![],
            inject_state_keys: vec![],
            output_key: "out".into(),
            expected_files: vec![],
            export_policy: Default::default(),
            use_plan_mode: false,
        }
    }

    #[test]
    fn catalog_lists_four_acp_models() {
        let catalog = build_harness_catalog();
        let ids: Vec<&str> = catalog.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["default", "sonnet", "opus", "haiku"]);
    }

    #[test]
    fn resolves_permission_mode_from_use_plan_mode() {
        let mut planned = node("sonnet", BaseToolGroup::Coding);
        planned.use_plan_mode = true;
        assert_eq!(
            resolve_node_harness(&planned).unwrap().permission_mode,
            PermissionMode::Plan
        );
        // 默认 bypassPermissions 全权限；read_only 工具组只影响输入软提示。
        let read_only = resolve_node_harness(&node("sonnet", BaseToolGroup::ReadOnly)).unwrap();
        assert_eq!(read_only.permission_mode, PermissionMode::BypassPermissions);
        assert_eq!(read_only.model_id, "sonnet");
        let coding = resolve_node_harness(&node("opus", BaseToolGroup::Coding)).unwrap();
        assert_eq!(coding.permission_mode, PermissionMode::BypassPermissions);
    }

    #[test]
    fn rejects_unknown_model_ref() {
        let error = resolve_node_harness(&node("gpt-4o", BaseToolGroup::ReadOnly)).unwrap_err();
        assert!(format!("{error:#}").contains("不在 ACP 目录中"));
    }

    #[test]
    fn permission_mode_ids_match_claude_modes() {
        assert_eq!(
            PermissionMode::BypassPermissions.mode_id(),
            "bypassPermissions"
        );
        assert_eq!(PermissionMode::Plan.mode_id(), "plan");
    }
}
