//! 图节点 ACP 权限请求的全局审查 AI（fail-open）。
//!
//! 子智能体（claude-agent-acp）在 bypassPermissions 全权限模式下运行，常规
//! 操作不再产生权限请求；仍浮出到客户端的请求（bypass 免疫的安全检查、
//! 显式 ask 规则等）由本模块结合「全局执行图摘要 + 当前节点任务 + 待审
//! 请求」裁决，输出 ALLOW / DENY / CLARIFY：
//! - DENY 的理由与 CLARIFY 的补充文本仅落节点活动记录展示——ACP 应答只有
//!   option_id，澄清文本无法回传执行器，行为上均按拒绝处理；
//! - 审查模型未配置、初始化/调用失败、超时或输出无法解析 → `Unavailable`，
//!   由调用方 fail-open 放行：无人值守的图节点不应被审查链路卡死
//!   （路径越界等硬规则在进入审查前已由静态策略拦截）。
//!
//! 模型槽复用 `settings.review`（SSH 命令审查同一「全局审查 AI」语义），
//! 但系统提示词是图权限裁决专用文案，不复用 SSH 的命令安全提示词。

use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::schema::v1::RequestPermissionRequest;
use rig::completion::CompletionModel;
use tokio::time::timeout;

use crate::agent::db::settings::SshReviewConfig;
use crate::agent::graph::input::truncate_chars;
use crate::agent::graph::types::{BaseToolGroup, GraphDefinition, GraphNode};

use super::mapping::redact;

/// 审查材料截断后缀（配合 `input::truncate_chars`，字符而非字节口径）。
const REVIEW_TRUNCATE_SUFFIX: &str = "…[已截断]";

/// 审查模型调用超时（对齐 ssh_review）。
const REVIEW_TIMEOUT_SECS: u64 = 30;
/// 用户需求进图摘要的截断上限。
const MAX_REQUIREMENT_CHARS: usize = 1_500;
/// 节点任务送审的截断上限。
const MAX_NODE_TASK_CHARS: usize = 2_000;
/// 待审请求 rawInput 送审的截断上限（脱敏后）。
const MAX_REQUEST_INPUT_CHARS: usize = 2_000;

/// 图权限裁决专用系统提示词：宽松优先（无明确风险即放行），首行协议
/// ALLOW / DENY / CLARIFY，随行给简短理由。
const GRAPH_PERMISSION_REVIEW_SYSTEM_PROMPT: &str = "\
你是执行图节点的权限审查员。图节点由子智能体（Claude Code）以 bypassPermissions \
全权限模式执行，常规操作不会请求权限，只有少数请求（安全检查、显式规则）会浮出\
到你这里裁决。请结合全局执行图与当前节点任务，判断该请求是否与任务一致、是否有\
破坏性风险。

裁决标准（宽松优先）：
- 与当前节点任务相关，且无不可逆破坏（不删库、不强制推送、不大规模改写任务\
外文件、不外泄密钥）→ ALLOW。
- 明确越权或有破坏性 → DENY，首行冒号后给一句理由。
- 信息不足但任务本身允许、风险可控时，倾向 ALLOW 而非 DENY。
- 只有必须补充说明才能安全执行时才输出 CLARIFY，首行冒号后给出建议补充的内容。

输出格式（严格遵守）：首行必须是 ALLOW、DENY 或 CLARIFY 之一（可带冒号与一句\
理由），随后可换行给出简短说明。不要输出 Markdown 代码块或其他格式。";

/// 一次 run 内共享的审查材料（Arc 传递，图摘要只构建一次）。
pub(crate) struct GraphReviewShared {
    pub(crate) config: SshReviewConfig,
    pub(crate) graph_digest: String,
}

/// 节点级审查材料：共享材料 + 当前节点快照。
pub(crate) struct PermissionReviewMaterials {
    pub(crate) shared: Arc<GraphReviewShared>,
    pub(crate) node_id: String,
    pub(crate) node_title: String,
    pub(crate) node_role: String,
    pub(crate) node_task: String,
    pub(crate) read_only: bool,
}

impl PermissionReviewMaterials {
    pub(crate) fn new(shared: Arc<GraphReviewShared>, node: &GraphNode) -> Self {
        Self {
            shared,
            node_id: node.id.clone(),
            node_title: node.title.clone(),
            node_role: node.role.trim().to_string(),
            node_task: truncate_chars(
                node.task.trim(),
                MAX_NODE_TASK_CHARS,
                REVIEW_TRUNCATE_SUFFIX,
            ),
            read_only: matches!(node.base_tool_group, BaseToolGroup::ReadOnly),
        }
    }

    /// 审计展示用的材料摘要（不含图摘要全文）。
    pub(crate) fn audit_label(&self) -> String {
        format!("节点 {}「{}」", self.node_id, self.node_title)
    }
}

/// 审查结论。`Unavailable` 是 fail-open 通道：调用方放行并留痕。
#[derive(Debug, Clone)]
pub(super) enum ReviewOutcome {
    Allowed,
    Denied { reason: String },
    Clarify { text: String },
    Unavailable { reason: String },
}

/// 构建全局图摘要：标题 + 摘要 + 用户需求（截断）+ 节点单行清单。
/// 每个节点一行，只保留审查感知必需的字段，控制审查请求体量。
pub(crate) fn build_graph_digest(definition: &GraphDefinition, user_requirement: &str) -> String {
    let mut lines = vec![
        format!("标题：{}", definition.title),
        format!("摘要：{}", definition.summary.trim()),
        format!(
            "用户需求：{}",
            truncate_chars(
                user_requirement.trim(),
                MAX_REQUIREMENT_CHARS,
                REVIEW_TRUNCATE_SUFFIX
            )
        ),
        String::from("节点："),
    ];
    for node in &definition.nodes {
        let group = node.base_tool_group.as_str();
        let plan = if node.use_plan_mode { "yes" } else { "no" };
        let deps = if node.depends_on.is_empty() {
            "无".to_string()
        } else {
            node.depends_on.join(",")
        };
        lines.push(format!(
            "- {}「{}」 工具组={} 计划模式={} 依赖=[{}]",
            node.id, node.title, group, plan, deps
        ));
    }
    lines.join("\n")
}

/// 审查一次权限请求。任何链路问题都以 `Unavailable` 返回（调用方 fail-open）。
pub(super) async fn review_permission(
    materials: &PermissionReviewMaterials,
    request: &RequestPermissionRequest,
) -> ReviewOutcome {
    if !materials.shared.config.is_configured() {
        return ReviewOutcome::Unavailable {
            reason: "审查模型未配置（设置 → 模型用途/审查）".to_string(),
        };
    }
    let config = &materials.shared.config;
    let model_name = config.model_config.model.trim().to_string();

    // 与 ssh_review 同口径：不带 max_tokens（交服务端默认预算）、关思考，
    // 避免思考链耗尽输出预算导致结论为空。
    let spec = crate::agent::rig_ext::model::PurposeModelSpec {
        api_key: config.model_config.api_key.clone(),
        api_base: config.model_config.url.clone(),
        model: config.model_config.model.clone(),
        max_tokens: None,
        context_window: None,
        temperature: 0.0,
        enable_thinking: false,
    };
    let model = match crate::agent::rig_ext::model::completions_model(&spec) {
        Ok(model) => model,
        Err(error) => {
            return ReviewOutcome::Unavailable {
                reason: format!("审查模型 `{model_name}` 初始化失败：{error}"),
            }
        }
    };

    let completion = crate::agent::rig_ext::model::build_completion_request(
        Some(GRAPH_PERMISSION_REVIEW_SYSTEM_PROMPT.to_string()),
        vec![rig::completion::Message::user(build_user_prompt(
            materials, request,
        ))],
        Vec::new(),
        None,
        0.0,
        false,
    );

    let inner = match timeout(
        Duration::from_secs(REVIEW_TIMEOUT_SECS),
        model.completion(completion),
    )
    .await
    {
        Ok(inner) => inner,
        Err(_) => {
            return ReviewOutcome::Unavailable {
                reason: format!("审查模型 `{model_name}` 调用超时（>{REVIEW_TIMEOUT_SECS}s）"),
            }
        }
    };
    let response = match inner {
        Ok(response) => response,
        Err(error) => {
            return ReviewOutcome::Unavailable {
                reason: format!("审查模型 `{model_name}` 调用失败：{error}"),
            }
        }
    };

    let content = response
        .choice
        .iter()
        .filter_map(|item| match item {
            rig::message::AssistantContent::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
        .trim()
        .to_string();
    if content.is_empty() {
        return ReviewOutcome::Unavailable {
            reason: format!("审查模型 `{model_name}` 返回空内容"),
        };
    }
    parse_outcome(&content)
}

/// 审查用户提示：全局图摘要 + 当前节点 + 待审请求（rawInput 先脱敏再截断）。
fn build_user_prompt(
    materials: &PermissionReviewMaterials,
    request: &RequestPermissionRequest,
) -> String {
    let fields = &request.tool_call.fields;
    let tool_name = fields.name.clone().unwrap_or_else(|| "未知工具".into());
    let title = fields.title.clone().unwrap_or_default();
    let locations = fields
        .locations
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|location| location.path.display().to_string())
        .collect::<Vec<_>>()
        .join("、");
    let raw_input = fields
        .raw_input
        .clone()
        .map(redact)
        .map(|value| value.to_string())
        .map(|text| truncate_chars(text.trim(), MAX_REQUEST_INPUT_CHARS, REVIEW_TRUNCATE_SUFFIX))
        .unwrap_or_else(|| "（无）".to_string());

    let role_line = if materials.node_role.is_empty() {
        String::new()
    } else {
        format!("角色：{}\n", materials.node_role)
    };
    let discipline = if materials.read_only {
        "（只读节点：约定不得写文件/执行副作用命令）"
    } else {
        ""
    };

    format!(
        "【全局执行图】\n{graph_digest}\n\n【当前节点】{discipline}\nid：{id} 标题：{title}\n{role_line}任务：\n{task}\n\n【待审查请求】\n工具：{tool}\n标题：{req_title}\n位置：{locations}\n输入：\n{raw_input}\n\n请裁决。",
        graph_digest = materials.shared.graph_digest,
        discipline = discipline,
        id = materials.node_id,
        title = materials.node_title,
        role_line = role_line,
        task = materials.node_task,
        tool = tool_name,
        req_title = if title.is_empty() { "（无）" } else { &title },
        locations = if locations.is_empty() { "（未声明）" } else { &locations },
        raw_input = raw_input,
    )
}

/// 解析审查输出：首行关键词（大小写不敏感，兼容中英文表达）。
///
/// 判定分两档：首行以协议英文词（deny/clarify/allow）或「允许/通过/拒绝」
/// **开头**时直接按该词裁决——理由文本里的否定表述（「无需补充」「不构成
/// 拒绝」）不再干扰判定；未按协议开头时才退回 contains 启发，此时保持
/// deny > clarify > allow 的优先级，避免「确认后才允许」类表述被误读为放行。
/// 无法识别 → `Unavailable`（调用方 fail-open 放行并留痕）。
fn parse_outcome(content: &str) -> ReviewOutcome {
    let trimmed = content.trim();
    let first_line = trimmed.lines().next().unwrap_or("").trim();
    let lower = first_line.to_lowercase();

    // 理由提取：优先取首行冒号后的文本，为空则取后续行。
    let extract_reason = || -> String {
        let after_colon = first_line
            .split_once(['：', ':'])
            .map(|(_, rest)| rest.trim().to_string())
            .filter(|rest| !rest.is_empty());
        after_colon.unwrap_or_else(|| {
            trimmed
                .lines()
                .skip(1)
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_string()
        })
    };
    let reason_or = |fallback: &str| -> String {
        let reason = extract_reason();
        if reason.is_empty() {
            fallback.to_string()
        } else {
            truncate_chars(&reason, 500, REVIEW_TRUNCATE_SUFFIX)
        }
    };

    // 协议词开头：直接裁决，理由文本不参与判定。
    if lower.starts_with("deny") || lower.starts_with("拒绝") {
        return ReviewOutcome::Denied {
            reason: reason_or("审查模型判定拒绝"),
        };
    }
    if lower.starts_with("clarify") || lower.starts_with("澄清") {
        return ReviewOutcome::Clarify {
            text: reason_or("需要补充信息后才能判断"),
        };
    }
    if lower.starts_with("allow") || lower.starts_with("允许") || lower.starts_with("通过") {
        return ReviewOutcome::Allowed;
    }
    // contains 启发回退（首行未按协议开头）。
    if lower.contains("拒绝") || lower.contains("不允许") {
        return ReviewOutcome::Denied {
            reason: reason_or("审查模型判定拒绝"),
        };
    }
    if lower.contains("澄清") || (lower.contains("需") && lower.contains("补充")) {
        return ReviewOutcome::Clarify {
            text: reason_or("需要补充信息后才能判断"),
        };
    }
    if lower.contains("允许") || lower.contains("通过") {
        return ReviewOutcome::Allowed;
    }
    ReviewOutcome::Unavailable {
        reason: format!(
            "审查输出无法解析：{}",
            truncate_chars(trimmed, 200, REVIEW_TRUNCATE_SUFFIX)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_outcome_reads_verdict_keywords() {
        assert!(matches!(parse_outcome("ALLOW"), ReviewOutcome::Allowed));
        assert!(matches!(
            parse_outcome("allow: 命令与任务一致"),
            ReviewOutcome::Allowed
        ));
        assert!(matches!(parse_outcome("允许"), ReviewOutcome::Allowed));
        assert!(matches!(
            parse_outcome("DENY: 会删除任务外文件"),
            ReviewOutcome::Denied { reason } if reason.contains("删除任务外文件")
        ));
        assert!(matches!(
            parse_outcome("拒绝：越权"),
            ReviewOutcome::Denied { .. }
        ));
        assert!(matches!(
            parse_outcome("CLARIFY: 需要确认目标分支"),
            ReviewOutcome::Clarify { text } if text.contains("目标分支")
        ));
    }

    #[test]
    fn parse_outcome_denies_take_priority_and_reason_falls_back_to_next_lines() {
        // 「确认后才允许」不得被误读为放行
        assert!(matches!(
            parse_outcome("DENY：需用户确认后才允许"),
            ReviewOutcome::Denied { .. }
        ));
        // 首行只有关键词时，理由取后续行
        assert!(matches!(
            parse_outcome("DENY\n该命令会强制推送到主分支"),
            ReviewOutcome::Denied { reason } if reason.contains("强制推送")
        ));
    }

    #[test]
    fn parse_outcome_verdict_prefix_beats_negated_keywords_in_reason() {
        // 协议词开头时，理由里的否定表述不得翻转结论（曾误判 CLARIFY/DENY）。
        assert!(matches!(
            parse_outcome("ALLOW：无需补充信息，任务一致，直接执行"),
            ReviewOutcome::Allowed
        ));
        assert!(matches!(
            parse_outcome("allow：该命令不构成拒绝理由"),
            ReviewOutcome::Allowed
        ));
        assert!(matches!(
            parse_outcome("允许：无需补充说明"),
            ReviewOutcome::Allowed
        ));
    }

    #[test]
    fn parse_outcome_unrecognized_content_is_unavailable() {
        assert!(matches!(
            parse_outcome("我认为这个请求问题不大，可以直接执行。"),
            ReviewOutcome::Unavailable { reason } if reason.contains("无法解析")
        ));
    }

    #[test]
    fn graph_digest_lists_nodes_with_plan_flag_and_dependencies() {
        let definition = serde_json::from_value(serde_json::json!({
            "version": 4,
            "title": "重构登录",
            "summary": "先调研后改造",
            "nodes": [
                { "id": "n1", "title": "调研", "modelRef": "sonnet",
                  "baseToolGroup": "read_only", "task": "调研", "outputKey": "o1" },
                { "id": "n2", "title": "改造", "modelRef": "opus",
                  "baseToolGroup": "coding", "task": "改造", "outputKey": "o2",
                  "dependsOn": ["n1"], "usePlanMode": true }
            ]
        }))
        .expect("反序列化图定义");
        let digest = build_graph_digest(&definition, "重构登录流程");
        assert!(digest.contains("标题：重构登录"));
        assert!(digest.contains("用户需求：重构登录流程"));
        assert!(digest.contains("- n1「调研」 工具组=read_only 计划模式=no 依赖=[无]"));
        assert!(digest.contains("- n2「改造」 工具组=coding 计划模式=yes 依赖=[n1]"));
    }
}
