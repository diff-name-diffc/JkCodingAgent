//! 工具结果落库 + 压缩管线（迁移自 `common/tool_result.rs`）。
//!
//! 与旧实现的关系：`prepare_rig_tool_result` / 截断 / 内联上限表逐行对齐
//! `prepare_tool_result_with_policy`；摘要走 rig `CompletionModel`（15s 超时，
//! 失败回退零 LLM 的 `extract_structured_summary` 规则抽取——该函数为纯规则
//! 实现，直接复用）。双标签摘要协议（DISPLAY_SUMMARY / CONTEXT_PAYLOAD）在
//! 子模块 `summary`（复制自 `summary/tool_summary.rs`，T4.1 迁移时归一）。
//!
//! 摘要模型缺省（None）时按任务约定跳过压缩、直接截断（完整原文保留在
//! 工具产物中）。

use anyhow::{Context, Result};
use rig::completion::CompletionModel;
use rig::message::ToolCall;
use tauri::ipc::Channel;

use super::llm_usage_from_rig;
use crate::agent::common::{emit, serialize_tool_arguments, UsageTracker};
use crate::agent::db::{DispatcherDb, DispatcherMessageRecord, ToolArtifactDraft};
use crate::agent::run_loop::AgentEvent;
use summary::extract_structured_summary;

pub(crate) mod summary;
#[cfg(test)]
mod tests;

// ─── 结果准备（显式声明 + 阈值双条件驱动，对齐 common/tool_result.rs） ──────────

/// Raw tool results remain inline up to this many characters. Longer results are
/// clipped with an explicit locator while the complete output remains an artifact.
pub const TOOL_RESULT_INLINE_MAX_CHARS: usize = 8_000;

/// 读取类工具未显式分页时的内联上限：读取结果通常是要精读的正文或检索命中，
/// 8000 字符仍可能不够。
pub const TOOL_RESULT_INLINE_MAX_CHARS_READ: usize = 10_000;

/// 显式按行分页读取时的内联上限。分页本身就是模型在主动控制读取粒度，
/// 一页只给 8000 字符会导致接续读取的往返次数过多。
pub const TOOL_RESULT_INLINE_MAX_CHARS_PAGED: usize = 20_000;

/// 内容读取类工具：结果主体是供模型精读的文本。这些工具享有两档内联预算——
/// 默认 READ（10000），显式传入 offset/limit 分页读取时 PAGED（20000）；
/// 其余工具维持 8000。
/// 注意：effective_args 会注入 schema default，列入此处的工具其 offset/limit
/// 参数不得声明 default，否则无法区分显式分页与默认读取。
/// ssh_memo_read 无 offset/limit 参数（备忘录全文有 8000 字符硬上限，
/// 加头部后恰落在 READ 档内），恒走默认 READ 档。
const INLINE_READ_TOOLS: &[&str] = &[
    "read_file",
    "browser_read_text",
    "grep",
    "glob",
    "list_dir",
    "graph_plan_report",
    "ssh_memo_read",
];

/// 按工具与入参决定本次调用的内联字符上限。
fn inline_max_chars(tool_name: &str, args: &serde_json::Value) -> usize {
    if !INLINE_READ_TOOLS.contains(&tool_name) {
        return TOOL_RESULT_INLINE_MAX_CHARS;
    }
    if args.get("offset").is_some() || args.get("limit").is_some() {
        TOOL_RESULT_INLINE_MAX_CHARS_PAGED
    } else {
        TOOL_RESULT_INLINE_MAX_CHARS_READ
    }
}

/// 默认压缩触发阈值（对齐 `tools/spec.rs` 的 DEFAULT_FORCE_COMPRESS_AFTER_CHARS）。
pub const DEFAULT_FORCE_COMPRESS_AFTER_CHARS: usize = 5_000;

/// 单个工具的结果策略（rig 工具面的挂载形态，对应旧 `ToolResultPolicy`；
/// Phase 2 工具迁移时随工具面声明）。
#[derive(Debug, Clone, Copy)]
pub struct RigToolResultPolicy {
    pub default_compress: bool,
    pub force_compress_after_chars: usize,
    pub persist_raw_artifact: bool,
}

impl RigToolResultPolicy {
    pub fn new(default_compress: bool) -> Self {
        Self {
            default_compress,
            force_compress_after_chars: DEFAULT_FORCE_COMPRESS_AFTER_CHARS,
            persist_raw_artifact: true,
        }
    }

    /// 覆盖压缩触发阈值（命令执行类工具使用更高的 12000）。
    pub fn with_force_compress_after_chars(mut self, chars: usize) -> Self {
        self.force_compress_after_chars = chars;
        self
    }
}

impl Default for RigToolResultPolicy {
    fn default() -> Self {
        Self::new(false)
    }
}

pub struct PreparedRigToolResult {
    pub display_content: String,
    pub context_payload: String,
    pub result_mode: &'static str,
    pub raw_output: String,
    pub needs_summary: bool,
    /// 模型调用工具时声明的信息提取意图（一句话描述期望从结果中提取什么）
    pub compress_intent: Option<String>,
}

/// 对齐 `prepare_tool_result_with_policy`：压缩是「显式声明 + 阈值」双条件
/// 驱动——只有 compress=true 且原文超过阈值才走摘要；其余超内联上限走确定性
/// 截断，完整原文保留在工具产物中。
pub fn prepare_rig_tool_result(
    tool_name: &str,
    args: &serde_json::Value,
    raw_output: &str,
    policy: &RigToolResultPolicy,
) -> PreparedRigToolResult {
    let trimmed = raw_output.trim();
    if trimmed.is_empty() {
        return PreparedRigToolResult {
            display_content: String::new(),
            context_payload: String::new(),
            result_mode: "raw",
            raw_output: String::new(),
            needs_summary: false,
            compress_intent: None,
        };
    }

    let model_compress = args
        .get("compress")
        .and_then(|v| v.as_bool())
        .unwrap_or(policy.default_compress);
    let compress_intent = args
        .get("compress_intent")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let char_count = trimmed.chars().count();
    let max_inline = inline_max_chars(tool_name, args);
    let needs_summary = model_compress && char_count > policy.force_compress_after_chars;

    if needs_summary {
        PreparedRigToolResult {
            display_content: String::new(),
            context_payload: String::new(),
            result_mode: "pending_summary",
            raw_output: trimmed.to_string(),
            needs_summary: true,
            compress_intent,
        }
    } else if char_count > max_inline {
        let truncated = truncate_tool_result(trimmed, char_count, max_inline);
        PreparedRigToolResult {
            display_content: truncated.clone(),
            context_payload: truncated,
            result_mode: "truncated",
            raw_output: trimmed.to_string(),
            needs_summary: false,
            compress_intent,
        }
    } else {
        PreparedRigToolResult {
            display_content: trimmed.to_string(),
            context_payload: trimmed.to_string(),
            result_mode: "raw",
            raw_output: trimmed.to_string(),
            needs_summary: false,
            compress_intent,
        }
    }
}

fn truncate_tool_result(raw_output: &str, char_count: usize, max_chars: usize) -> String {
    let prefix = raw_output.chars().take(max_chars).collect::<String>();
    let truncated_at_output_line = prefix.chars().filter(|ch| *ch == '\n').count() + 1;
    let total_lines = raw_output.lines().count().max(1);
    let source_line_marker = source_line_number_at_cut(&prefix)
        .map(|line| format!("（该行标注的源码/匹配行号为 {line}）"))
        .unwrap_or_default();

    format!(
        "{prefix}\n\n[结果已截断：仅返回前 {max_chars} / {char_count} 字符；截断发生在原始结果第 {truncated_at_output_line} 个输出行{source_line_marker}，原始结果共 {total_lines} 行。完整原始结果见工具产物。]"
    )
}

fn source_line_number_at_cut(prefix: &str) -> Option<usize> {
    let current_line = prefix.rsplit('\n').next()?.trim_start();
    let digit_count = current_line
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .count();
    if digit_count == 0 {
        return None;
    }
    let (digits, suffix) = current_line.split_at(digit_count);
    matches!(suffix.chars().next(), Some('|' | ':' | '-'))
        .then(|| digits.parse::<usize>().ok())
        .flatten()
}

pub(super) fn bound_inline_tool_result(content: String) -> String {
    // 摘要结果的展示上限保持紧凑值：压缩后的内容本就该足够精炼。
    let char_count = content.chars().count();
    if char_count > TOOL_RESULT_INLINE_MAX_CHARS {
        truncate_tool_result(&content, char_count, TOOL_RESULT_INLINE_MAX_CHARS)
    } else {
        content
    }
}

// ─── 摘要模型（rig 形态） ─────────────────────────────────────────────────────

/// 摘要模型调用配置（压缩用途槽位的运行形态）。
pub struct RigSummaryModel<'a, M: CompletionModel> {
    pub model: &'a M,
    pub model_name: &'a str,
    pub max_tokens: Option<u64>,
    pub temperature: f64,
}

/// 工具结果摘要是夹在「工具执行完成 → 主模型下一轮」之间的串行步骤，
/// 超时必须短：压缩是锦上添花，超时即回退零 LLM 的规则抽取
/// （`extract_structured_summary`），绝不能让它成为工具调用的主要时延来源。
const SUMMARY_TIMEOUT_SECS: u64 = 15;

pub(super) struct RigToolSummary {
    pub display_content: String,
    pub context_payload: String,
}

/// 以 rig 模型执行双标签摘要。成功返回摘要与该次用量（用量由调用方并入
/// UsageTracker，对齐旧 on_usage 回调语义）。
async fn summarize_with_rig_model<M: CompletionModel>(
    summary: &RigSummaryModel<'_, M>,
    tool_name: &str,
    raw_output: &str,
    user_question: Option<&str>,
    compress_intent: Option<&str>,
) -> Result<(RigToolSummary, rig::completion::Usage)> {
    let system = summary::build_summary_system_prompt(tool_name, compress_intent.is_some());
    let user = summary::build_summary_user_message(tool_name, raw_output, user_question, compress_intent);
    let request = super::model::build_completion_request(
        Some(system),
        vec![rig::completion::Message::user(user)],
        Vec::new(),
        summary.max_tokens,
        summary.temperature,
        true,
    );
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(SUMMARY_TIMEOUT_SECS),
        summary.model.completion(request),
    )
    .await
    .context("工具结果摘要超时")?
    .map_err(|error| anyhow::anyhow!("工具结果摘要请求失败：{error}"))?;

    let text = response
        .choice
        .iter()
        .filter_map(|content| match content {
            rig::message::AssistantContent::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    let (context_payload, display_content) = summary::parse_dual_tool_summary(&text);
    if context_payload.is_empty() && display_content.is_empty() {
        anyhow::bail!("工具结果摘要返回空内容：{tool_name}");
    }
    Ok((
        RigToolSummary {
            display_content,
            context_payload,
        },
        response.usage,
    ))
}

// ─── 落库入口 ─────────────────────────────────────────────────────────────────

/// 工具结果落库 + 可选压缩 + `ToolFinished` 事件（rig 版
/// `persist_tool_result_with_compression`）。返回落库消息；回灌给模型的
/// 内容取 `context_payload`（压缩模式下比 display 更详细，与旧契约一致）。
#[allow(clippy::too_many_arguments)]
pub async fn persist_rig_tool_result<M: CompletionModel>(
    db: &DispatcherDb,
    workspace_id: &str,
    on_event: &Channel<AgentEvent>,
    tool_call: &ToolCall,
    policy: &RigToolResultPolicy,
    result: &str,
    summary: Option<&RigSummaryModel<'_, M>>,
    usage_tracker: &mut UsageTracker,
) -> Result<DispatcherMessageRecord> {
    let tool_call_id = tool_call.wire_call_id().to_string();
    let tool_name = &tool_call.function.name;
    // G9-14：序列化失败不再静默降级为 `{}`——错误上抛，由运行循环以 Failed 收口。
    let arguments_json = serialize_tool_arguments(tool_name, &tool_call.function.arguments)?;
    let prepared = prepare_rig_tool_result(tool_name, &tool_call.function.arguments, result, policy);

    if !prepared.needs_summary {
        let artifacts = policy
            .persist_raw_artifact
            .then(|| ToolArtifactDraft::raw_tool_output(tool_name, result))
            .into_iter()
            .collect::<Vec<_>>();
        let tool_message = db
            .add_visible_tool_result_async(
                workspace_id,
                &prepared.display_content,
                &prepared.context_payload,
                Some(&tool_call_id),
                Some(tool_name),
                Some(prepared.result_mode),
                &artifacts,
            )
            .await?;
        emit(
            on_event,
            AgentEvent::ToolFinished {
                tool_call_id,
                name: tool_name.clone(),
                arguments: arguments_json,
                display_text: tool_message.plain_text(),
                context_payload: prepared.context_payload.clone(),
                result_mode: prepared.result_mode.to_string(),
                detail_refs: tool_message.tool_artifacts.clone(),
            },
        );
        return Ok(tool_message);
    }

    // 摘要模型缺省：跳过压缩，直接截断（完整原文保留在工具产物中）。
    let Some(summary) = summary else {
        let char_count = prepared.raw_output.chars().count();
        let truncated = truncate_tool_result(
            &prepared.raw_output,
            char_count,
            inline_max_chars(tool_name, &tool_call.function.arguments),
        );
        return persist_with_presentation(
            db,
            workspace_id,
            on_event,
            tool_call,
            &tool_call_id,
            &arguments_json,
            result,
            truncated.clone(),
            truncated,
            "truncated",
            policy.persist_raw_artifact,
        )
        .await;
    };

    let user_question = db
        .get_latest_user_message_content_async(workspace_id)
        .await
        .ok()
        .flatten();

    match summarize_with_rig_model(
        summary,
        tool_name,
        &prepared.raw_output,
        user_question.as_deref(),
        prepared.compress_intent.as_deref(),
    )
    .await
    {
        Ok((rig_summary, usage)) => {
            if usage.has_values() {
                usage_tracker.record(&llm_usage_from_rig(&usage));
            }
            let mode = if prepared.compress_intent.is_some() {
                "intent_compressed"
            } else {
                "conservative_summary"
            };
            persist_with_presentation(
                db,
                workspace_id,
                on_event,
                tool_call,
                &tool_call_id,
                &arguments_json,
                result,
                rig_summary.display_content,
                rig_summary.context_payload,
                mode,
                policy.persist_raw_artifact,
            )
            .await
        }
        Err(error) => {
            eprintln!(
                "summarize_tool_result failed for {}: {:#}, falling back to structured extraction",
                tool_name, error
            );
            let structured = extract_structured_summary(tool_name, &prepared.raw_output);
            persist_with_presentation(
                db,
                workspace_id,
                on_event,
                tool_call,
                &tool_call_id,
                &arguments_json,
                result,
                structured.clone(),
                structured,
                "structured_fallback",
                policy.persist_raw_artifact,
            )
            .await
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn persist_with_presentation(
    db: &DispatcherDb,
    workspace_id: &str,
    on_event: &Channel<AgentEvent>,
    tool_call: &ToolCall,
    tool_call_id: &str,
    arguments_json: &str,
    raw_result: &str,
    display_content: String,
    context_payload: String,
    result_mode: &'static str,
    persist_raw_artifact: bool,
) -> Result<DispatcherMessageRecord> {
    let display_content = bound_inline_tool_result(display_content);
    let context_payload = bound_inline_tool_result(context_payload);
    let artifacts = persist_raw_artifact
        .then(|| ToolArtifactDraft::raw_tool_output(&tool_call.function.name, raw_result))
        .into_iter()
        .collect::<Vec<_>>();
    let tool_message = db
        .add_visible_tool_result_async(
            workspace_id,
            &display_content,
            &context_payload,
            Some(tool_call_id),
            Some(&tool_call.function.name),
            Some(result_mode),
            &artifacts,
        )
        .await?;

    emit(
        on_event,
        AgentEvent::ToolFinished {
            tool_call_id: tool_call_id.to_string(),
            name: tool_call.function.name.clone(),
            arguments: arguments_json.to_string(),
            display_text: tool_message.plain_text(),
            context_payload: tool_message
                .context_payload
                .clone()
                .unwrap_or_else(|| tool_message.plain_text()),
            result_mode: result_mode.to_string(),
            detail_refs: tool_message.tool_artifacts.clone(),
        },
    );

    Ok(tool_message)
}
