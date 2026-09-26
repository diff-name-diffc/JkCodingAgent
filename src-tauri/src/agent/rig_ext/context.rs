//! 统一上下文整形层（预算 + 配对安全滑窗 + 历史级滚动压缩）。
//!
//! 主对话三条路径（plain_chat / 项目编排 / 架构 agent）发给模型的历史视图
//! 统一经运行循环里的 `compact_history` 整形：总字符超预算时保头（首轮 user
//! 任务意图）保尾（最近完整消息），被裁中段不直接丢弃——经摘要模型折叠为
//! 「【前情摘要】」滚动摘要消息留在内存序列中（摘要模型缺省/失败回退零 LLM
//! 的规则抽取）；下一次裁剪会把旧摘要一并折叠合并，跨轮、跨 run（持久化到
//! `dispatcher_session_summaries`）保持连续。窗口起点只允许落在不破坏
//! tool_call ↔ tool_result 配对的边界上；整形只影响发给模型的视图：落库走
//! 运行循环的 persist 路径，不读内存 messages。
//!
//! 纯函数层：`trim_history`（整形 + 暴露被裁中段）、`shape_history`（整形 +
//! 占位 + 配对修复兜底，阶段 1 行为）。`repair_pairing` 语义对齐
//! `common::message::repair_tool_call_pairing`（作用于 rig `Message`）。
//!
//! 子智能体走同一套 `compact_history_offline`（头部保护 system + 首轮任务）。

use anyhow::{Context, Result};
use rig::completion::{CompletionModel, Message};
use rig::message::{AssistantContent, Text, ToolCall, ToolResult, ToolResultContent, UserContent};
use tokio::sync::watch;

use super::tool_result::RigSummaryModel;
use crate::agent::common::{UNANSWERED_TOOL_RESULT_PLACEHOLDER, UsageTracker};

/// 单张图片的固定估算成本：base64 不直接计全量（一张截图的 base64 可达
/// 数十万字符，按实际字符计会瞬间挤爆预算）。
const IMAGE_FIXED_COST_CHARS: usize = 2000;

/// 占位文案的预算预留量（占位在窗口选定后才按实际裁掉条数渲染，预留一个
/// 富余量级避免边界抖动，组装后不再精确校验）。
const TRIM_PLACEHOLDER_CHARS: usize = 80;

/// 极端超预算时头部 user 消息的文本保留量（优先保尾，头部截断兜底）。
const HEAD_TRUNCATE_KEEP_CHARS: usize = 4000;

/// 上下文整形字符预算：统一容量源（模型库条目 contextWindow，未配置回退
/// `DEFAULT_CONTEXT_WINDOW_CAPACITY_TOKENS` = 1M）× 3.5 字符/token × 0.6
/// 安全系数——比子智能体的 4×1/2 更保守（中文密度更高），并预留 preamble、
/// 输出预算与本轮循环增长的空间。
pub(crate) fn context_budget_chars(context_window_tokens: Option<u64>) -> usize {
    const CHARS_PER_TOKEN_X10: u64 = 35; // 3.5 字符/token
    const SAFETY_PERCENT: u64 = 60;
    let window_tokens =
        context_window_tokens.unwrap_or(crate::agent::db::DEFAULT_CONTEXT_WINDOW_CAPACITY_TOKENS);
    (window_tokens.saturating_mul(CHARS_PER_TOKEN_X10) / 10 * SAFETY_PERCENT / 100) as usize
}

/// 估算单条消息的上下文占用（字符数）：正文/思考计字符数，ToolCall 计
/// name+arguments，ToolResult 计内容，Image 按固定成本估算。
/// 主对话与子智能体共用这一口径。
pub(crate) fn message_chars(message: &Message) -> usize {
    match message {
        Message::System { content } => content.chars().count(),
        Message::Assistant { content, .. } => content
            .iter()
            .map(|item| match item {
                AssistantContent::Text(text) => text.text.chars().count(),
                AssistantContent::Reasoning(reasoning) => reasoning.display_text().chars().count(),
                AssistantContent::ToolCall(call) => {
                    call.function.name.chars().count()
                        + call.function.arguments.to_string().chars().count()
                }
                AssistantContent::Image(_) => IMAGE_FIXED_COST_CHARS,
            })
            .sum(),
        Message::User { content } => content
            .iter()
            .map(|item| match item {
                UserContent::Text(text) => text.text.chars().count(),
                UserContent::ToolResult(result) => result
                    .content
                    .iter()
                    .map(|block| match block {
                        ToolResultContent::Text(text) => text.text.chars().count(),
                        ToolResultContent::Image(_) => IMAGE_FIXED_COST_CHARS,
                        ToolResultContent::Json { value } => value.to_string().chars().count(),
                    })
                    .sum(),
                UserContent::Image(_) => IMAGE_FIXED_COST_CHARS,
                _ => 0,
            })
            .sum(),
    }
}

/// 整形结果：整形后的消息序列 + 被裁掉的中段原文（供滚动压缩复用）。
pub(crate) struct TrimmedHistory {
    pub messages: Vec<Message>,
    /// 被整体移除的中段（保持原 vec 顺序）。空 = 未发生中段裁剪（未超预算，
    /// 或仅剩「头+尾」的极端预算路径）。
    pub dropped: Vec<Message>,
    /// 裁剪占位在 `messages` 中的下标（`dropped` 非空时有效）：压缩路径以
    /// 摘要消息 1:1 替换该占位，不扰动其它下标。
    pub placeholder_index: usize,
}

/// 上下文整形（纯函数）：总字符 ≤ 预算原样返回；超预算时保头（前
/// `header_len` 条——主对话 1 = 首轮 user 任务意图，子智能体 2 = system +
/// 首轮任务）+ 保尾（最近若干完整消息直到预算用尽），中间整体移除并插入
/// 占位说明。窗口起点只允许落在「不含 ToolResult 的 User」或「不含
/// ToolCall 的 Assistant」之前（配对安全）；头部末条带 ToolCall 时收缩
/// 头部（其结果必落在中段，保护它会造孤儿）。
/// 本函数不做配对修复兜底——运行循环靠迭代边界不变量保证配对完整，
/// 测试辅助与防御场景可追加 `repair_pairing`。任何输入都不得 panic。
pub(crate) fn trim_history(
    mut messages: Vec<Message>,
    budget_chars: usize,
    header_len: usize,
) -> TrimmedHistory {
    let total_chars: usize = messages.iter().map(message_chars).sum();
    if total_chars <= budget_chars {
        return TrimmedHistory {
            messages,
            dropped: Vec::new(),
            placeholder_index: 0,
        };
    }

    // 头部配对安全收缩：头部末条带未应答 ToolCall 时收缩（其结果必随中段
    // 被裁，保护它会留下孤儿调用）。
    let mut head_len = header_len.min(messages.len());
    while head_len > 0 && has_tool_call(&messages[head_len - 1]) {
        head_len -= 1;
    }
    let has_head = head_len > 0;
    let head_chars: usize = messages[..head_len].iter().map(message_chars).sum();
    let tail_budget = budget_chars.saturating_sub(head_chars + TRIM_PLACEHOLDER_CHARS);

    // 保尾：从末尾向前累计。最后一条即使单独超预算也先纳入，随后的配对
    // 边界可以把它并进被裁中段（落单的工具结果不能留在请求里）。
    let mut tail_chars = 0usize;
    let mut start = messages.len();
    for index in (0..messages.len()).rev() {
        let chars = message_chars(&messages[index]);
        if start < messages.len() && tail_chars.saturating_add(chars) > tail_budget {
            break;
        }
        tail_chars = tail_chars.saturating_add(chars);
        start = index;
    }

    // 配对安全边界。保尾的起点有两种合法形态：
    // - 普通 user / 无工具调用的 assistant：直接作为窗口起点；
    // - 带 ToolCall 的 assistant，且它的全部结果都紧随其后：整对留在窗口内
    //   （结果在起点之后，不会被裁掉）。
    // 起点若落在工具结果上，说明对应 assistant 已经在预算外，继续后移把
    // 这些结果一并裁掉。后移到末尾仍没有安全起点时，整段不安全后缀进入
    // 被裁中段——不能回退保留最后一条，否则会发出没有 tool_call 的 tool 结果。
    while start < messages.len() && !is_safe_window_start(&messages[start]) {
        if has_tool_call(&messages[start]) && tool_calls_have_following_results(&messages, start) {
            break;
        }
        start += 1;
    }

    let dropped = start.saturating_sub(head_len);
    if dropped == 0 {
        // 中段无可裁（窗口已覆盖全部或头即尾）：原样保留，交给头部截断兜底。
        // 极端情况：保头 + 保尾最小集仍超预算——优先保尾，头部 user 文本截断。
        if has_head && messages.iter().map(message_chars).sum::<usize>() > budget_chars {
            cap_overlong_head(&mut messages);
        }
        return TrimmedHistory {
            messages,
            dropped: Vec::new(),
            placeholder_index: 0,
        };
    }

    let mut rest = messages.split_off(head_len);
    let tail = rest.split_off(dropped);
    let mut shaped = messages;
    shaped.push(trim_placeholder(dropped));
    shaped.extend(tail);

    // 极端情况：保头 + 保尾最小集仍超预算——优先保尾，头部 user 文本截断。
    if has_head && shaped.iter().map(message_chars).sum::<usize>() > budget_chars {
        cap_overlong_head(&mut shaped);
    }

    TrimmedHistory {
        messages: shaped,
        dropped: rest,
        placeholder_index: head_len,
    }
}

/// 消息是否带工具调用（头部配对安全收缩用）。
fn has_tool_call(message: &Message) -> bool {
    matches!(message, Message::Assistant { content, .. }
        if content.iter().any(|item| matches!(item, AssistantContent::ToolCall(_))))
}

/// 起点处的 assistant 工具调用是否都能在紧随的工具结果消息里找到结果。
/// 结果必须连续出现在后续 user 消息中；遇到普通文本 user / 下一条 assistant
/// 即停止。缺任何一个结果就不能把这条 assistant 当作窗口起点。
fn tool_calls_have_following_results(messages: &[Message], start: usize) -> bool {
    let Message::Assistant { content, .. } = &messages[start] else {
        return false;
    };
    let needed: Vec<String> = content
        .iter()
        .filter_map(|item| match item {
            AssistantContent::ToolCall(call) => Some(call.id.as_str().to_string()),
            _ => None,
        })
        .collect();
    if needed.is_empty() {
        return false;
    }
    let mut found = vec![false; needed.len()];
    for message in &messages[start + 1..] {
        let Message::User { content } = message else {
            break;
        };
        let mut saw_result = false;
        for item in content {
            let UserContent::ToolResult(result) = item else {
                continue;
            };
            saw_result = true;
            if let Some(position) = needed
                .iter()
                .position(|call_id| call_id == result.call.as_str())
            {
                found[position] = true;
            }
        }
        if !saw_result {
            break;
        }
    }
    found.iter().all(|present| *present)
}

/// 窗口起点是否配对安全：不含 ToolResult 的 User、不含 ToolCall 的
/// Assistant、System（本层历史一般无 system，防御性放行）。
/// 带 ToolCall 且结果完整跟随的 assistant 由调用方单独放行。
fn is_safe_window_start(message: &Message) -> bool {
    match message {
        Message::User { content } => !content
            .iter()
            .any(|item| matches!(item, UserContent::ToolResult(_))),
        Message::Assistant { content, .. } => !content
            .iter()
            .any(|item| matches!(item, AssistantContent::ToolCall(_))),
        Message::System { .. } => true,
    }
}

/// 裁剪占位说明（user 角色文本）：让模型知道中间有历史被省略，而非任务刚开始。
/// `trim_history` 的默认填充——运行循环的 `compact_history` 会以滚动摘要
/// 消息 1:1 替换它；占位路径仅服务无摘要需求的调用方（及防御场景）。
fn trim_placeholder(dropped: usize) -> Message {
    Message::User {
        content: vec![UserContent::text(format!(
            "【上下文裁剪】为控制上下文长度，中间的 {dropped} 条历史消息已被省略。如需其中的信息，请重新调用相应工具获取。"
        ))],
    }
}

/// 头部仍超预算时的截断。主对话头部是 user；子智能体头部是 system + 首轮
/// user。只改 `messages[0]` 会在 system 上直接返回，兜底变成空操作。
fn cap_overlong_head(messages: &mut [Message]) {
    for message in messages {
        match message {
            Message::System { content } => cap_system_text(content, HEAD_TRUNCATE_KEEP_CHARS),
            Message::User { .. } => {
                truncate_head_text(message, HEAD_TRUNCATE_KEEP_CHARS);
                break;
            }
            _ => break,
        }
    }
}

fn cap_system_text(content: &mut String, keep_chars: usize) {
    if content.chars().count() <= keep_chars {
        return;
    }
    let kept: String = content.chars().take(keep_chars).collect();
    *content = format!("{kept}\n……（系统提示过长，已按上下文预算截断）");
}

/// 头部 user 消息的文本截断：保留前 `keep_chars` 字符 + 省略标注，
/// 非文本 part（图片等）原样保留。
fn truncate_head_text(message: &mut Message, keep_chars: usize) {
    let Message::User { content } = message else {
        return;
    };
    let mut remaining = keep_chars;
    let mut truncated = false;
    let mut new_content = Vec::with_capacity(content.len());
    for item in std::mem::take(content) {
        match item {
            UserContent::Text(mut text) if !truncated => {
                let len = text.text.chars().count();
                if len <= remaining {
                    remaining -= len;
                    new_content.push(UserContent::Text(text));
                } else {
                    let kept: String = text.text.chars().take(remaining).collect();
                    text.text = format!("{kept}\n……（原始任务描述过长，已按上下文预算截断）");
                    new_content.push(UserContent::Text(text));
                    truncated = true;
                }
            }
            // 截断点之后的其余文本 part 丢弃。
            UserContent::Text(_) => {}
            other => new_content.push(other),
        }
    }
    if new_content.is_empty() {
        new_content.push(UserContent::Text(Text::new(
            "（原始任务描述已按上下文预算省略）",
        )));
    }
    *content = new_content;
}

/// 测试用入口：不跟踪落库 id 的配对修复。生产路径用 `repair_pairing_aligned`。
#[cfg(test)]
pub(crate) fn repair_pairing(messages: &mut Vec<Message>) {
    let mut ids = vec![None; messages.len()];
    repair_pairing_aligned(messages, &mut ids);
}

/// 剔除孤儿 tool result，并为缺失结果的 tool call 补占位。语义对齐
/// `common::message::repair_tool_call_pairing`。`ids` 与消息下标保持对齐；
/// 多条工具结果收成一条时，锚点取这批里最后一条已知 id。
pub(crate) fn repair_pairing_aligned(messages: &mut Vec<Message>, ids: &mut Vec<Option<String>>) {
    if ids.len() != messages.len() {
        *ids = vec![None; messages.len()];
    }
    let mut repaired: Vec<Message> = Vec::with_capacity(messages.len());
    let mut repaired_ids: Vec<Option<String>> = Vec::with_capacity(messages.len());
    // 最近一条带 ToolCall 的 assistant 尚未应答的调用（按声明顺序）。
    let mut pending: Vec<ToolCall> = Vec::new();

    let incoming: Vec<(Message, Option<String>)> = std::mem::take(messages)
        .into_iter()
        .zip(std::mem::take(ids))
        .collect();
    let mut iter = incoming.into_iter().peekable();
    while let Some((message, id)) = iter.next() {
        match message {
            Message::Assistant {
                id: message_id,
                content,
            } => {
                // 进入新 assistant 前，上一条 assistant 的未应答调用先补占位
                //（其结果消息已被裁掉或从未产生）。
                flush_pending_results(&mut repaired, &mut repaired_ids, &mut pending);
                pending = content
                    .iter()
                    .filter_map(|item| match item {
                        AssistantContent::ToolCall(call) => Some(call.clone()),
                        _ => None,
                    })
                    .collect();
                repaired.push(Message::Assistant {
                    id: message_id,
                    content,
                });
                repaired_ids.push(id);
            }
            Message::User { content } => {
                let (mut results, mut others) = partition_tool_results(content);
                if results.is_empty() {
                    // 普通 user 消息：工具结果必须紧随其 assistant；user 消息
                    // 插入意味着此前 pending 的调用已无结果——先补占位再放行。
                    flush_pending_results(&mut repaired, &mut repaired_ids, &mut pending);
                    repaired.push(Message::User { content: others });
                    repaired_ids.push(id);
                    continue;
                }
                // DB 把每个 role=tool 行转成独立 User。同一批调用的结果会连着
                // 出现，必须收齐再匹配；否则第一条就把其余调用补成占位，后面的
                // 真实结果变成孤儿被丢掉。
                let mut group_ids = vec![id];
                while iter
                    .peek()
                    .is_some_and(|(message, _)| is_pure_tool_result_message(message))
                {
                    let Some((Message::User { content }, next_id)) = iter.next() else {
                        break;
                    };
                    group_ids.push(next_id);
                    let (more, extra) = partition_tool_results(content);
                    results.extend(more);
                    others.extend(extra);
                }
                let merged_id = group_ids.into_iter().rev().find_map(|item| item);
                let mut kept: Vec<UserContent> = Vec::with_capacity(pending.len() + others.len());
                for call in pending.drain(..) {
                    let position = results.iter().position(|item| match item {
                        UserContent::ToolResult(result) => result.call.as_str() == call.id.as_str(),
                        _ => false,
                    });
                    match position {
                        Some(index) => kept.push(results.remove(index)),
                        None => kept.push(UserContent::ToolResult(unanswered_tool_result(&call))),
                    }
                }
                kept.extend(others);
                if !kept.is_empty() {
                    repaired.push(Message::User { content: kept });
                    repaired_ids.push(merged_id);
                }
            }
            system @ Message::System { .. } => {
                flush_pending_results(&mut repaired, &mut repaired_ids, &mut pending);
                repaired.push(system);
                repaired_ids.push(id);
            }
        }
    }
    flush_pending_results(&mut repaired, &mut repaired_ids, &mut pending);
    *messages = repaired;
    *ids = repaired_ids;
}

fn partition_tool_results(content: Vec<UserContent>) -> (Vec<UserContent>, Vec<UserContent>) {
    content
        .into_iter()
        .partition(|item| matches!(item, UserContent::ToolResult(_)))
}

/// 只含工具结果的 user 消息。混有文本的 user 是人类消息，不能并进结果批。
fn is_pure_tool_result_message(message: &Message) -> bool {
    let Message::User { content } = message else {
        return false;
    };
    !content.is_empty()
        && content
            .iter()
            .all(|item| matches!(item, UserContent::ToolResult(_)))
}

/// 为仍 pending 的调用补占位结果消息（紧随其 assistant）。
fn flush_pending_results(
    repaired: &mut Vec<Message>,
    ids: &mut Vec<Option<String>>,
    pending: &mut Vec<ToolCall>,
) {
    if pending.is_empty() {
        return;
    }
    let content = pending
        .drain(..)
        .map(|call| UserContent::ToolResult(unanswered_tool_result(&call)))
        .collect();
    repaired.push(Message::User { content });
    ids.push(None);
}

/// 补齐用的占位工具结果：只承载「未产生结果」这一事实，不伪装成执行错误
///（文案与 DB 读侧 `repair_tool_call_pairing` 一致）。
fn unanswered_tool_result(call: &ToolCall) -> ToolResult {
    ToolResult {
        call: call.id.clone(),
        provider: call.provider.clone(),
        name: call.function.name.clone(),
        content: vec![ToolResultContent::text(UNANSWERED_TOOL_RESULT_PLACEHOLDER)],
    }
}

// ─── 历史级滚动压缩（阶段 2） ─────────────────────────────────────────────────

/// 滚动摘要消息与持久化摘要的共同前缀标记：渲染回摘要输入时凭它识别
/// 「这是更早历史的摘要」，滚动合并不得丢失其中的任务目标与关键决策。
pub(crate) const HISTORY_SUMMARY_MARKER: &str = "【前情摘要】";

/// 摘要正文上限（字符）：摘要是长期驻留上下文的开销，必须远小于它替代的原文。
const HISTORY_SUMMARY_MAX_CHARS: usize = 3_000;

/// 送入摘要模型的渲染历史上限（对齐工具结果摘要的 24K 口径）。
const HISTORY_SUMMARY_INPUT_MAX_CHARS: usize = 24_000;

/// 单条消息渲染上限（头 1000 + 尾 500）：单条巨型工具结果不得挤占其它消息。
const RENDER_MESSAGE_MAX_CHARS: usize = 1_500;

/// 历史摘要与工具结果摘要同为串行附加步骤，超时必须短（对齐 15s 口径）。
const HISTORY_SUMMARY_TIMEOUT_SECS: u64 = 15;

/// 一次滚动压缩的结果。摘要消息已就位于 `messages`（替换裁剪占位）。
pub(crate) struct CompactionOutcome {
    /// 合并后的滚动摘要消息全文（含标记前缀；持久化与线格式共用同一文本）。
    pub summary: String,
    /// 被折叠中段在压缩前 vec 的起始下标与长度（供调用方同步平行索引结构，
    /// 如运行循环的消息 id 跟踪）。
    pub splice_start: usize,
    pub dropped_len: usize,
}

/// 判断消息是否为滚动摘要（折叠头部时用：摘要头不保护，并入新摘要）。
fn is_summary_message(message: &Message) -> bool {
    matches!(message, Message::User { content }
        if content.iter().any(|item| matches!(item, UserContent::Text(text)
            if text.text.starts_with(HISTORY_SUMMARY_MARKER))))
}

/// 历史级滚动压缩：超预算时把被裁中段折叠为「【前情摘要】」摘要消息留在
/// 内存序列中（而非占位丢弃）。摘要首选压缩用途槽位模型（15s 超时），
/// 缺省/失败回退零 LLM 的规则抽取（头尾保留），压缩链路绝不因 LLM 不可用
/// 而中断。下一次裁剪会把旧摘要一并渲染进输入，由摘要模型合并——
/// rolling summary 语义（对齐 rig-memory `CompactingMemory`）。
///
/// `header_len`：保护头部条数——主对话 1（首轮任务意图）、子智能体 2
/// （system + 首轮任务，恒不折叠）；头部恰是上一轮滚动摘要时不再保护
/// （并入新摘要，保证跨轮/跨 run 连续性）。
///
/// 调用方不变量：迭代边界的内存序列配对完整（运行循环保证）；本函数内部
/// 不做 repair——窗口边界由 `trim_history` 的配对安全规则保证。
///
/// 返回 None = 未发生中段裁剪（未超预算，或极端预算下「头+尾」已占满、
/// 无中段可摘要——此时仍应用整形结果，如头部截断）。
pub(crate) async fn compact_history<M: CompletionModel>(
    messages: &mut Vec<Message>,
    budget_chars: usize,
    header_len: usize,
    summary_model: Option<&RigSummaryModel<'_, M>>,
    usage_tracker: &mut UsageTracker,
    cancel_rx: Option<&watch::Receiver<bool>>,
) -> Option<CompactionOutcome> {
    let (trimmed, rendered) = prepare_fold(messages, budget_chars, header_len)?;
    let summary_body = match summary_model {
        Some(model) if !cancellation_requested(cancel_rx) => {
            tokio::select! {
                biased;
                _ = wait_for_summary_cancel(cancel_rx) => fallback_history_summary(&rendered),
                result = summarize_history_segment(model, &rendered) => match result {
                    Ok((text, usage)) => {
                        if usage.has_values() {
                            usage_tracker.record(&super::llm_usage_from_rig(&usage));
                        }
                        text
                    }
                    Err(error) => {
                        eprintln!("历史摘要模型调用失败，回退规则抽取：{error:#}");
                        fallback_history_summary(&rendered)
                    }
                },
            }
        }
        Some(_) | None => fallback_history_summary(&rendered),
    };
    Some(finalize_fold(messages, trimmed, &summary_body))
}

fn cancellation_requested(cancel_rx: Option<&watch::Receiver<bool>>) -> bool {
    cancel_rx.is_some_and(|cancel_rx| *cancel_rx.borrow() || cancel_rx.has_changed().is_err())
}

async fn wait_for_summary_cancel(cancel_rx: Option<&watch::Receiver<bool>>) {
    let Some(cancel_rx) = cancel_rx else {
        std::future::pending::<()>().await;
        return;
    };
    if cancellation_requested(Some(cancel_rx)) {
        return;
    }
    let mut cancel_rx = cancel_rx.clone();
    loop {
        if cancel_rx.changed().await.is_err() || *cancel_rx.borrow() {
            return;
        }
    }
}

/// 无摘要模型的压缩入口（子智能体等不额外消耗 LLM 调用的路径）：零 LLM
/// 规则抽取（头尾保留）。与 `compact_history` 同一整形/折叠管线。
pub(crate) fn compact_history_offline(
    messages: &mut Vec<Message>,
    budget_chars: usize,
    header_len: usize,
) -> Option<CompactionOutcome> {
    let (trimmed, rendered) = prepare_fold(messages, budget_chars, header_len)?;
    Some(finalize_fold(
        messages,
        trimmed,
        &fallback_history_summary(&rendered),
    ))
}

/// 折叠准备：未超预算或无中段可裁时返回 None（后者仍应用整形结果，如头部
/// 截断）；否则返回整形结果（中段已替换为占位）与被裁中段的渲染文本。
/// 滚动摘要头折叠在此判定：保护头部恰是上一轮摘要时不再保护，并入新摘要。
fn compaction_trim_budget(budget_chars: usize) -> usize {
    const SUMMARY_OVERSHOOT: usize = HISTORY_SUMMARY_MAX_CHARS + 200;
    budget_chars.saturating_sub(SUMMARY_OVERSHOOT.saturating_sub(TRIM_PLACEHOLDER_CHARS))
}

fn prepare_fold(
    messages: &mut Vec<Message>,
    budget_chars: usize,
    header_len: usize,
) -> Option<(TrimmedHistory, String)> {
    if messages.iter().map(message_chars).sum::<usize>() <= budget_chars {
        return None;
    }
    let mut header_len = header_len.min(messages.len());
    if header_len > 0 && is_summary_message(&messages[header_len - 1]) {
        header_len -= 1;
    }
    let owned = std::mem::take(messages);
    // 占位只有几十个字，换上的摘要最长约 3000。先把差额从预算里扣掉，
    // 否则贴着预算裁完再换摘要会重新超预算。
    let trimmed = trim_history(owned, compaction_trim_budget(budget_chars), header_len);
    if trimmed.dropped.is_empty() {
        *messages = trimmed.messages;
        return None;
    }
    let rendered = render_messages_for_summary(&trimmed.dropped);
    Some((trimmed, rendered))
}

/// 折叠收尾：摘要正文 → 摘要消息 1:1 替换占位，写回调用方 vec。
fn finalize_fold(
    messages: &mut Vec<Message>,
    trimmed: TrimmedHistory,
    summary_body: &str,
) -> CompactionOutcome {
    let dropped_len = trimmed.dropped.len();
    let summary_body = truncate_middle_chars(summary_body, HISTORY_SUMMARY_MAX_CHARS);
    let summary_text = format!(
        "{HISTORY_SUMMARY_MARKER}较早的 {dropped_len} 条历史消息已折叠为以下摘要（原文已不在上下文中，如需细节请重新调用工具获取）：\n{summary_body}"
    );
    let splice_start = trimmed.placeholder_index;
    let mut shaped = trimmed.messages;
    shaped[splice_start] = Message::User {
        content: vec![UserContent::text(summary_text.clone())],
    };
    *messages = shaped;
    CompactionOutcome {
        summary: summary_text,
        splice_start,
        dropped_len,
    }
}

/// 以 rig 模型生成历史滚动摘要（单块纯文本输出，无双标签协议——摘要只
/// 回灌模型上下文，不直接展示前端）。返回摘要与该次用量（调用方并入
/// UsageTracker，对齐工具结果摘要的用量语义）。
async fn summarize_history_segment<M: CompletionModel>(
    summary: &RigSummaryModel<'_, M>,
    rendered: &str,
) -> Result<(String, rig::completion::Usage)> {
    let request = super::model::build_completion_request(
        Some(history_summary_system_prompt()),
        vec![Message::user(rendered)],
        Vec::new(),
        summary.max_tokens,
        summary.temperature,
        true,
    );
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(HISTORY_SUMMARY_TIMEOUT_SECS),
        summary.model.completion(request),
    )
    .await
    .context("历史摘要超时")?
    .map_err(|error| anyhow::anyhow!("历史摘要请求失败：{error}"))?;

    let text = response
        .choice
        .iter()
        .filter_map(|content| match content {
            AssistantContent::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    let trimmed = text.trim();
    if trimmed.is_empty() {
        anyhow::bail!("历史摘要返回空内容");
    }
    Ok((trimmed.to_string(), response.usage))
}

fn history_summary_system_prompt() -> String {
    format!(
        "你是调度 Agent 的会话历史压缩器。一段对话历史即将从模型上下文中移除，你的任务是把它压缩成一份滚动摘要，\
         让看不到原文的模型仍能保持上下文连续、准确继续任务。\n\
         规则：\n\
         - 必须保留：用户的原始任务目标与明确约束；已做出的关键决策及原因；已完成的操作及结果（创建/修改的文件路径、\
         执行的关键命令、重要结论）；尚未解决的问题、报错与当前进行中的工作。\n\
         - 只保留原文明确出现的事实，严禁猜测或补全；文件路径、符号名、配置键、错误文本保持原文，不得改写含义。\n\
         - 输入中若含「{HISTORY_SUMMARY_MARKER}」段落，那是更早历史的摘要：其要点必须并入新摘要，\
         不得丢失其中的任务目标与关键决策。\n\
         - 直接输出摘要正文（可分条），不要输出标题、客套话或解释，总量控制在 {HISTORY_SUMMARY_MAX_CHARS} 字符以内。"
    )
}

/// 摘要模型缺省/失败时的零 LLM 兜底：头（任务背景）+ 尾（最近进展）保留，
/// 中间省略。渲染文本中更早的【前情摘要】通常位于头部，随之保留。
fn fallback_history_summary(rendered: &str) -> String {
    truncate_middle_chars(rendered, HISTORY_SUMMARY_MAX_CHARS)
}

/// 把被裁中段渲染为「角色: 文本」行序列（供摘要模型与规则兜底共用）。
/// reasoning 是瞬态思考，不进入摘要；图片以占位行表示。
fn render_messages_for_summary(messages: &[Message]) -> String {
    let mut rendered = String::new();
    for message in messages {
        match message {
            Message::System { content } => push_rendered(&mut rendered, "系统", content),
            Message::User { content } => {
                for item in content {
                    match item {
                        UserContent::Text(text) => push_rendered(&mut rendered, "用户", &text.text),
                        UserContent::ToolResult(result) => {
                            let body = result
                                .content
                                .iter()
                                .filter_map(|block| block.as_text())
                                .collect::<Vec<_>>()
                                .join("\n");
                            push_rendered(
                                &mut rendered,
                                &format!("工具结果({})", result.name),
                                &body,
                            );
                        }
                        UserContent::Image(_) => rendered.push_str("用户: [图片]\n"),
                        _ => {}
                    }
                }
            }
            Message::Assistant { content, .. } => {
                for item in content {
                    match item {
                        AssistantContent::Text(text) => {
                            push_rendered(&mut rendered, "助手", &text.text)
                        }
                        AssistantContent::ToolCall(call) => push_rendered(
                            &mut rendered,
                            &format!("助手调用工具({})", call.function.name),
                            &call.function.arguments.to_string(),
                        ),
                        // 瞬态思考不进入摘要（与线格式不回灌 reasoning 的口径一致）。
                        AssistantContent::Reasoning(_) => {}
                        AssistantContent::Image(_) => rendered.push_str("助手: [图片]\n"),
                    }
                }
            }
        }
    }
    truncate_middle_chars(&rendered, HISTORY_SUMMARY_INPUT_MAX_CHARS)
}

fn push_rendered(out: &mut String, role: &str, text: &str) {
    let body = truncate_middle_chars(text, RENDER_MESSAGE_MAX_CHARS);
    out.push_str(role);
    out.push_str(": ");
    out.push_str(&body);
    out.push('\n');
}

/// 头 60% + 尾 40% 的中段省略截断（头承载任务背景，尾承载最近进展）。
fn truncate_middle_chars(text: &str, max_chars: usize) -> String {
    let chars = text.chars().count();
    if chars <= max_chars {
        return text.to_string();
    }
    let head_chars = max_chars * 3 / 5;
    let tail_chars = max_chars - head_chars;
    let head: String = text.chars().take(head_chars).collect();
    let tail: String = text.chars().skip(chars - tail_chars).collect();
    format!("{head}\n[...省略 {} 字符...]\n{tail}", chars - max_chars)
}

/// 请求级上下文超限错误识别：服务商文案无统一标准，按主流文案子串匹配
/// （小写化后）。命中即触发运行循环的预算收缩重试；误判的代价只是一次
/// 带压缩的重试，漏判才会整轮报废——匹配从宽。
pub(crate) fn is_context_overflow_error(error_text: &str) -> bool {
    let text = error_text.to_lowercase();
    const PATTERNS: &[&str] = &[
        "context_length_exceeded",
        "maximum context length",
        "context length",
        "context window",
        "prompt is too long",
        "input is too long",
        // dashscope：「Range of input length should be [1, N]」
        "input length should be",
        "too many tokens",
        "request entity too large",
        "reduce the length",
    ];
    PATTERNS.iter().any(|pattern| text.contains(pattern))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig::message::{ProviderCallId, ToolCallId, ToolFunction};

    /// 阶段 1 管线的测试形态：trim + 配对修复兜底（占位裁剪路径）。
    fn shape_history(messages: Vec<Message>, budget_chars: usize) -> Vec<Message> {
        let mut trimmed = trim_history(messages, budget_chars, 1);
        repair_pairing(&mut trimmed.messages);
        trimmed.messages
    }

    /// 测试用摘要模型：completion 返回固定文案或错误；stream 不会被压缩
    /// 路径调用（unreachable）。
    struct StubSummaryModel {
        response: std::result::Result<String, String>,
    }

    impl StubSummaryModel {
        fn rig_summary_model(&self) -> RigSummaryModel<'_, Self> {
            RigSummaryModel {
                model: self,
                max_tokens: None,
                temperature: 0.3,
            }
        }
    }

    impl CompletionModel for StubSummaryModel {
        fn completion(
            &self,
            _request: rig::completion::CompletionRequest,
        ) -> impl std::future::Future<
            Output = std::result::Result<
                rig::completion::CompletionResponse,
                rig::completion::CompletionError,
            >,
        > + rig::wasm_compat::WasmCompatSend {
            let response = self.response.clone();
            async move {
                match response {
                    Ok(text) => Ok(rig::completion::CompletionResponse::new(
                        vec![AssistantContent::Text(Text::new(text))],
                        rig::completion::Usage::new(),
                        "stub",
                    )),
                    Err(error) => Err(rig::completion::CompletionError::ProviderError(error)),
                }
            }
        }

        fn stream(
            &self,
            _request: rig::completion::CompletionRequest,
        ) -> impl std::future::Future<
            Output = std::result::Result<
                rig::streaming::StreamingCompletionResponse,
                rig::completion::CompletionError,
            >,
        > + rig::wasm_compat::WasmCompatSend {
            async move { unreachable!("压缩路径不调用 stream") }
        }
    }

    fn failing_summary_model() -> StubSummaryModel {
        StubSummaryModel {
            response: Err("模拟摘要模型失败".to_string()),
        }
    }

    fn user(text: &str) -> Message {
        Message::user(text)
    }

    fn assistant_text(text: &str) -> Message {
        Message::Assistant {
            id: None,
            content: vec![AssistantContent::Text(Text::new(text))],
        }
    }

    fn assistant_toolcall(call_id: &str, arguments: serde_json::Value) -> Message {
        Message::Assistant {
            id: None,
            content: vec![AssistantContent::ToolCall(ToolCall::from_wire(
                call_id,
                ToolFunction::new("t".to_string(), arguments),
            ))],
        }
    }

    fn tool_result(call_id: &str, text: &str) -> Message {
        let provider = ProviderCallId::new(call_id.to_string());
        Message::User {
            content: vec![UserContent::ToolResult(ToolResult {
                call: ToolCallId::for_provider(provider.as_ref()),
                provider,
                name: "t".to_string(),
                content: vec![ToolResultContent::text(text)],
            })],
        }
    }

    fn repeat(ch: char, count: usize) -> String {
        std::iter::repeat_n(ch, count).collect()
    }

    fn has_tool_result(message: &Message) -> bool {
        matches!(message, Message::User { content }
            if content.iter().any(|item| matches!(item, UserContent::ToolResult(_))))
    }

    fn has_tool_call(message: &Message) -> bool {
        matches!(message, Message::Assistant { content, .. }
            if content.iter().any(|item| matches!(item, AssistantContent::ToolCall(_))))
    }

    // ① 未超预算：原样返回。
    #[test]
    fn under_budget_returns_history_unchanged() {
        let messages = vec![user("任务"), assistant_text("回复"), user("追问")];
        let original = messages.clone();
        let shaped = shape_history(messages, usize::MAX);
        assert_eq!(shaped, original);
    }

    // ② 超预算：保头保尾 + 占位说明。
    #[test]
    fn over_budget_keeps_head_and_tail_with_placeholder() {
        let head = user(&repeat('头', 100));
        let middle_user = user(&repeat('中', 300));
        let middle_assistant = assistant_text(&repeat('答', 300));
        let tail = user(&repeat('尾', 100));
        let messages = vec![head.clone(), middle_user, middle_assistant, tail.clone()];
        // 预算 = 头 100 + 占位预留 80 + 尾 100：中段两条必须整体移除。
        let shaped = shape_history(messages, 280);
        assert_eq!(shaped.len(), 3);
        assert_eq!(shaped[0], head);
        let Message::User { content } = &shaped[1] else {
            panic!("应插入占位说明");
        };
        let placeholder_text = format!("{content:?}");
        assert!(placeholder_text.contains("【上下文裁剪】"));
        assert!(placeholder_text.contains('2'));
        assert_eq!(shaped[2], tail);
    }

    // ③ 裁剪边界落在 tool_call / tool_result 中间时自动后移：起点不得落在
    // assistant-toolcall 上或其结果消息上，整对同进同出。
    #[test]
    fn boundary_moves_forward_off_tool_call_result_pair() {
        let head = user(&repeat('头', 100));
        // assistant 工具调用：name 1 字符 + arguments JSON 100 字符 = 101。
        let call = assistant_toolcall("c1", serde_json::json!({ "a": repeat('参', 92) }));
        assert_eq!(message_chars(&call), 101);
        let result = tool_result("c1", &repeat('果', 100));
        let tail = user(&repeat('尾', 100));
        let messages = vec![head.clone(), call, result, tail.clone()];
        // 预算 = 头 100 + 占位预留 80 + 尾 100 + 结果 100 = 380：
        // 尾部累计可容纳 tail+result，再加上 call（101）则超限——起点先落在
        // result 上（孤儿结果边界），自动后移到 tail，call/result 整对裁掉。
        let shaped = shape_history(messages, 380);
        assert_eq!(shaped.len(), 3);
        assert_eq!(shaped[0], head);
        assert_eq!(shaped[2], tail);
        assert!(!shaped.iter().any(has_tool_call));
        assert!(!shaped.iter().any(has_tool_result));
    }

    // ④ 窗口首条孤儿 tool result 被剔除：极端预算下回退保留的最后一条是
    // 工具结果（其 assistant 已被裁掉），repair 必须把它剔除而不是发孤儿结果。
    #[test]
    fn orphan_tool_result_at_window_start_is_dropped() {
        let head = user(&repeat('头', 100));
        let call = assistant_toolcall("c1", serde_json::json!({ "a": repeat('参', 95) }));
        let result = tool_result("c1", &repeat('果', 100));
        let messages = vec![head.clone(), call, result];
        // 预算只够尾部一条工具结果：没有安全起点，结果并入被裁中段，不留孤儿。
        let shaped = shape_history(messages, 280);
        assert_eq!(shaped.len(), 2);
        assert_eq!(shaped[0], head);
        assert!(!shaped.iter().any(has_tool_result));
        assert!(!shaped.iter().any(has_tool_call));
    }

    // ⑤ 尾部是没有结果的 tool call：不能把它单独留在窗口里（生产路径
    // compact_history 不跑 repair）。整段并入被裁中段，只留头和占位。
    #[test]
    fn unanswered_trailing_tool_call_is_folded_away() {
        let head = user(&repeat('头', 100));
        let big_call = assistant_toolcall("c1", serde_json::json!({ "a": repeat('参', 500) }));
        let last_call = assistant_toolcall("c9", serde_json::json!({}));
        let messages = vec![head.clone(), big_call, last_call];
        let shaped = shape_history(messages, 280);
        assert_eq!(shaped.len(), 2);
        assert_eq!(shaped[0], head);
        assert!(!shaped.iter().any(has_tool_call));
        assert!(!shaped.iter().any(has_tool_result));
    }

    // ⑤b repair 本身仍为缺失结果补占位（循环入口的防御路径）。
    #[test]
    fn repair_pairing_keeps_results_split_across_user_messages() {
        let assistant = Message::Assistant {
            id: None,
            content: vec![
                AssistantContent::ToolCall(ToolCall::from_wire(
                    "c1",
                    ToolFunction::new("t".to_string(), serde_json::json!({})),
                )),
                AssistantContent::ToolCall(ToolCall::from_wire(
                    "c2",
                    ToolFunction::new("t".to_string(), serde_json::json!({"n": 2})),
                )),
            ],
        };
        let mut messages = vec![
            assistant,
            tool_result("c1", "第一结果"),
            tool_result("c2", "第二结果"),
        ];
        repair_pairing(&mut messages);
        assert_eq!(messages.len(), 2);
        let Message::User { content } = &messages[1] else {
            panic!("同一批调用的结果应收成一条 user");
        };
        let texts: Vec<&str> = content
            .iter()
            .filter_map(|item| match item {
                UserContent::ToolResult(result) => {
                    result.content.first().and_then(|block| block.as_text())
                }
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["第一结果", "第二结果"]);
    }

    #[test]
    fn repair_pairing_supplies_missing_tool_result() {
        let call = assistant_toolcall("c9", serde_json::json!({}));
        let mut messages = vec![call];
        repair_pairing(&mut messages);
        assert_eq!(messages.len(), 2);
        assert!(has_tool_call(&messages[0]));
        let Message::User { content } = &messages[1] else {
            panic!("缺失结果的 tool call 应补占位结果");
        };
        let Some(UserContent::ToolResult(result)) = content.first() else {
            panic!("占位应为工具结果");
        };
        assert_eq!(result.call.as_str(), "c9");
        let text = result.content[0].as_text().unwrap_or_default();
        assert!(text.contains("没有产生结果"));
    }

    // ⑤c 工具对本身装得进尾部预算、且结果就是最后一条（工具迭代边界的常态）
    // 时，必须整对保留，不能把 assistant 裁掉只留孤儿结果。
    #[test]
    fn trailing_tool_pair_that_fits_stays_intact() {
        let head = user(&repeat('头', 100));
        let middle = user(&repeat('中', 400));
        let call = assistant_toolcall("c1", serde_json::json!({ "a": "x" }));
        let result = tool_result("c1", &repeat('果', 80));
        let call_chars = message_chars(&call);
        let result_chars = message_chars(&result);
        // 头 100 + 占位 80 + 整对：中段装不下，工具对装得下。
        let budget = 100 + 80 + call_chars + result_chars;
        let shaped = shape_history(
            vec![head.clone(), middle, call.clone(), result.clone()],
            budget,
        );
        assert_eq!(shaped.len(), 4);
        assert_eq!(shaped[0], head);
        assert_eq!(shaped[2], call);
        assert_eq!(shaped[3], result);
    }

    // ⑥ 极端超预算：保头 + 保尾最小集仍超预算——优先保尾，头部文本截断，
    // 任何输入不得 panic。
    #[test]
    fn extreme_over_budget_truncates_head_and_never_panics() {
        let head = user(&repeat('头', 10_000));
        let tail = user(&repeat('尾', 10_000));
        let shaped = shape_history(vec![head, tail.clone()], 100);
        assert_eq!(shaped.len(), 2);
        let Message::User { content } = &shaped[0] else {
            panic!("头部应为 user 消息");
        };
        let Some(UserContent::Text(text)) = content.first() else {
            panic!("头部应为文本");
        };
        let head_chars = text.text.chars().count();
        assert!(head_chars < 10_000, "头部应被截断：{head_chars}");
        assert!(head_chars >= HEAD_TRUNCATE_KEEP_CHARS);
        assert!(text.text.contains("（原始任务描述过长"));
        assert_eq!(shaped[1], tail);

        // 单条巨型消息、空历史、全工具结果历史：均不得 panic。
        let _ = shape_history(vec![user(&repeat('独', 100_000))], 10);
        let _ = shape_history(Vec::new(), 10);
        let _ = shape_history(vec![tool_result("c1", &repeat('果', 10_000))], 10);
    }

    #[test]
    fn extreme_budget_caps_system_prompt_and_task() {
        let tail = user("尾");
        let trimmed = trim_history(
            vec![
                Message::system(&repeat('系', 8_000)),
                user(&repeat('任', 8_000)),
                tail.clone(),
            ],
            100,
            2,
        );
        assert_eq!(trimmed.messages.len(), 3);
        let Message::System { content } = &trimmed.messages[0] else {
            panic!("头部应为 system");
        };
        assert!(content.contains("系统提示过长"));
        assert!(content.chars().count() < 8_000);
        let Message::User { content } = &trimmed.messages[1] else {
            panic!("第二条应为任务");
        };
        let Some(UserContent::Text(text)) = content.first() else {
            panic!("任务应为文本");
        };
        assert!(text.text.contains("原始任务描述过长"));
        assert_eq!(trimmed.messages[2], tail);
    }

    #[test]
    fn budget_derives_from_context_window() {
        // 1M × 3.5 字符/token × 0.6 安全系数。
        assert_eq!(context_budget_chars(Some(1_000_000)), 2_100_000);
        assert_eq!(context_budget_chars(None), 2_100_000);
        assert_eq!(context_budget_chars(Some(128_000)), 268_800);
    }

    #[test]
    fn message_chars_counts_parts_and_image_fixed_cost() {
        let call = assistant_toolcall("c1", serde_json::json!({ "a": "bc" }));
        // name 1 + arguments {"a":"bc"} 序列化 10 字符。
        assert_eq!(message_chars(&call), 1 + "{\"a\":\"bc\"}".chars().count());
        assert_eq!(message_chars(&tool_result("c1", "结果")), 2);
        assert_eq!(message_chars(&user("你好")), 2);
        assert_eq!(message_chars(&Message::system("系统")), 2);
    }

    // ⑦ 滚动压缩：摘要模型失败时回退规则抽取——被裁中段折叠为【前情摘要】
    // 消息而非占位丢弃；头尾保持，摘要内容来自中段渲染文本。
    #[tokio::test]
    async fn compact_history_folds_middle_into_summary_via_fallback() {
        let head = user("原始任务：重构上下文管理");
        let middle_a = user(&repeat('中', 400));
        let middle_b = assistant_text(&repeat('答', 400));
        let tail = user(&repeat('尾', 100));
        let mut messages = vec![head.clone(), middle_a, middle_b, tail.clone()];
        let mut tracker = UsageTracker::new();
        // 预算 300：头 11 + 占位预留 80 → 尾预算 209，只装得下 tail（100），
        // 中段两条整体折叠。
        let stub = failing_summary_model();
        let summary_model = stub.rig_summary_model();
        let outcome = compact_history(
            &mut messages,
            300,
            1,
            Some(&summary_model),
            &mut tracker,
            None,
        )
        .await
        .expect("应发生中段压缩");
        assert_eq!(outcome.splice_start, 1);
        assert_eq!(outcome.dropped_len, 2);
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0], head);
        assert_eq!(messages[2], tail);
        let Message::User { content } = &messages[1] else {
            panic!("应为摘要消息");
        };
        let Some(UserContent::Text(text)) = content.first() else {
            panic!("摘要应为文本");
        };
        assert!(text.text.starts_with(HISTORY_SUMMARY_MARKER));
        assert!(outcome.summary.starts_with(HISTORY_SUMMARY_MARKER));
        // 规则兜底保留渲染文本头部 → 中段内容进入摘要。
        assert!(text.text.contains('中'));
    }

    // ⑧ 滚动合并：头部是上一轮【前情摘要】时不再保护，并入新摘要（跨轮/
    // 跨 run 的摘要连续性：旧摘要要点不得丢失）。
    #[tokio::test]
    async fn compact_history_folds_previous_summary_head() {
        let old_summary = user(&format!("{HISTORY_SUMMARY_MARKER}早期摘要：任务目标是 X。"));
        let middle = assistant_text(&repeat('答', 400));
        let tail = user(&repeat('尾', 100));
        let mut messages = vec![old_summary, middle, tail.clone()];
        let mut tracker = UsageTracker::new();
        let stub = failing_summary_model();
        let summary_model = stub.rig_summary_model();
        let outcome = compact_history(
            &mut messages,
            300,
            1,
            Some(&summary_model),
            &mut tracker,
            None,
        )
        .await
        .expect("应发生中段压缩");
        // 旧摘要头并入：占位下标 0，dropped = 旧摘要 + 中段。
        assert_eq!(outcome.splice_start, 0);
        assert_eq!(outcome.dropped_len, 2);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1], tail);
        let Message::User { content } = &messages[0] else {
            panic!("新摘要应在头部");
        };
        let Some(UserContent::Text(text)) = content.first() else {
            panic!("摘要应为文本");
        };
        assert!(text.text.contains("任务目标是 X"));
    }

    // ⑨ 未超预算：压缩是 no-op（内存序列原样，无摘要消息注入）。
    #[tokio::test]
    async fn compact_history_cancelled_before_summary_uses_fallback() {
        let head = user("原始任务");
        let middle = user(&repeat('中', 400));
        let tail = user(&repeat('尾', 100));
        let mut messages = vec![head, middle, tail];
        let mut tracker = UsageTracker::new();
        let (_cancel_tx, cancel_rx) = watch::channel(true);
        let stub = StubSummaryModel {
            response: Ok("不应调用模型".to_string()),
        };
        let summary_model = stub.rig_summary_model();
        let outcome = compact_history(
            &mut messages,
            300,
            1,
            Some(&summary_model),
            &mut tracker,
            Some(&cancel_rx),
        )
        .await
        .expect("应发生中段压缩");
        assert!(!outcome.summary.contains("不应调用模型"));
        assert!(outcome.summary.contains('中'));
    }

    #[tokio::test]
    async fn compact_history_under_budget_is_noop() {
        let mut messages = vec![user("任务"), assistant_text("回复")];
        let original = messages.clone();
        let mut tracker = UsageTracker::new();
        let stub = failing_summary_model();
        let summary_model = stub.rig_summary_model();
        assert!(
            compact_history(
                &mut messages,
                usize::MAX,
                1,
                Some(&summary_model),
                &mut tracker,
                None,
            )
            .await
            .is_none()
        );
        assert_eq!(messages, original);
    }

    // ⑩ 摘要模型成功：LLM 产出文本原样进入摘要消息（而非规则抽取）。
    #[tokio::test]
    async fn compact_history_uses_model_output_when_available() {
        let head = user("原始任务");
        let middle_a = user(&repeat('中', 400));
        let middle_b = assistant_text(&repeat('答', 400));
        let tail = user(&repeat('尾', 100));
        let mut messages = vec![head, middle_a, middle_b, tail];
        let mut tracker = UsageTracker::new();
        let stub = StubSummaryModel {
            response: Ok("模型产出的滚动摘要".to_string()),
        };
        let summary_model = stub.rig_summary_model();
        let outcome = compact_history(
            &mut messages,
            300,
            1,
            Some(&summary_model),
            &mut tracker,
            None,
        )
        .await
        .expect("应发生中段压缩");
        assert!(outcome.summary.contains("模型产出的滚动摘要"));
        let Message::User { content } = &messages[1] else {
            panic!("应为摘要消息");
        };
        let Some(UserContent::Text(text)) = content.first() else {
            panic!("摘要应为文本");
        };
        assert!(text.text.contains("模型产出的滚动摘要"));
    }

    // ⑩b 工具迭代边界：结果是最后一条，对应调用装不进尾部预算。
    // 调用和结果都折进摘要，窗口里不得留下孤儿 tool 结果。
    #[tokio::test]
    async fn compact_history_folds_orphan_tool_result_instead_of_emitting_it() {
        let head = user(&repeat('头', 100));
        let call = assistant_toolcall("c1", serde_json::json!({ "a": repeat('参', 200) }));
        let result = tool_result("c1", "工具结果正文-必须进摘要");
        let mut messages = vec![head.clone(), call, result];
        let mut tracker = UsageTracker::new();
        let stub = failing_summary_model();
        let summary_model = stub.rig_summary_model();
        // 头 100 + 占位 80 + 结果（远小于调用）装得下，调用装不下。
        let outcome = compact_history(
            &mut messages,
            280,
            1,
            Some(&summary_model),
            &mut tracker,
            None,
        )
        .await
        .expect("应把装不下的工具对折进摘要");
        assert!(outcome.dropped_len >= 2);
        assert!(!messages.iter().any(has_tool_call));
        assert!(!messages.iter().any(has_tool_result));
        assert_eq!(messages[0], head);
        assert!(outcome.summary.contains("工具结果正文-必须进摘要"));
    }

    // ⑪ 摘要渲染：reasoning 不进入摘要；单条超长消息按头尾截断。
    #[test]
    fn render_for_summary_skips_reasoning_and_caps_long_messages() {
        let mut message = assistant_text(&repeat('文', 3_000));
        let Message::Assistant { content, .. } = &mut message else {
            panic!();
        };
        content.insert(
            0,
            AssistantContent::Reasoning(rig::message::Reasoning::new("思考过程内容")),
        );
        let rendered = render_messages_for_summary(&[message]);
        assert!(!rendered.contains("思考过程内容"));
        assert!(rendered.contains("助手: "));
        assert!(rendered.contains("省略"));
        assert!(rendered.chars().count() <= RENDER_MESSAGE_MAX_CHARS + 64);
    }

    #[test]
    fn context_overflow_error_patterns() {
        assert!(is_context_overflow_error(
            "HTTP 400: context_length_exceeded - This model's maximum context length is 131072"
        ));
        assert!(is_context_overflow_error(
            "Range of input length should be [1, 1000000]"
        ));
        assert!(is_context_overflow_error(
            "prompt is too long: 200000 tokens"
        ));
        assert!(!is_context_overflow_error("HTTP 401 unauthorized"));
        assert!(!is_context_overflow_error("网络连接失败"));
    }
}
