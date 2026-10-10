use anyhow::Result;

use super::super::db::{DispatcherDb, DispatcherMessageRecord, DispatcherMessageUsageStats};
use crate::agent::db::{ChatMessage, OutboundToolCall};

// ─── Assistant Message Persistence ───────────────────────────────────────────────

pub async fn persist_assistant_message(
    db: &DispatcherDb,
    workspace_id: &str,
    content: &str,
    usage_stats: &DispatcherMessageUsageStats,
) -> Result<DispatcherMessageRecord> {
    db.add_visible_message_with_usage_async(workspace_id, "assistant", content, usage_stats)
        .await
}

pub async fn persist_tool_calls_message(
    db: &DispatcherDb,
    workspace_id: &str,
    content: &str,
    tool_calls: &[OutboundToolCall],
    thinking_content: &str,
    thinking_elapsed_ms: Option<u64>,
) -> Result<DispatcherMessageRecord> {
    db.add_visible_message_with_tools_and_thinking_async(
        workspace_id,
        "assistant",
        content,
        None,
        None,
        None,
        Some(tool_calls),
        if thinking_content.is_empty() {
            None
        } else {
            Some(thinking_content)
        },
        thinking_elapsed_ms.unwrap_or(0),
    )
    .await
}

/// 序列化工具参数供模型/前端展示。
///
/// G9-14：失败（如非有限浮点数）不再记日志降级为空对象 `{}`，而是返回错误上抛——
/// 静默降级会让模型/前端看到的参数与工具实际执行所用的 effective_args 不一致
/// 且无线索可查。调用方（run loop 的工具执行入口）以 `?` 透传，运行以 Failed
/// 事件收口；错误消息以「错误：」开头，符合前端展示与 `is_tool_error_message`
/// 的既有契约。实践中 LLM 响应经 JSON 解析得到的参数不可能含非有限浮点数，
/// 该分支是防御性兜底。
pub(crate) fn serialize_tool_arguments(
    tool_name: &str,
    arguments: &serde_json::Value,
) -> Result<String> {
    serde_json::to_string(arguments)
        .map_err(|error| anyhow::anyhow!("错误：工具 '{tool_name}' 参数序列化失败：{error}"))
}

// ─── LLM Context Filtering ─────────────────────────────────────────────────────

/// `wait_for_tools` 控制工具名：等待语义的唯一出处是 `rig_ext::loop::wait`
///（re-export 本常量），历史装配过滤也按本常量识别控制对。
pub(crate) const WAIT_TOOL_NAME: &str = "wait_for_tools";

/// 纯调度 plumbing 工具名：其 assistant/tool 消息不进入 LLM 上下文。
/// 本常量是 LLM 上下文过滤的唯一口径来源；DB 加载路径
/// （`db::messages::queries::load_llm_history`）直接委托 `should_keep_llm_message`。
/// 这些工具名只存在于老库的历史行（dispatch 子进程系统已下线），保留过滤
/// 是为了不让旧会话的 plumbing 消息重新灌进上下文。
const DISPATCH_PLUMBING_TOOL_NAMES: [&str; 6] = [
    "dispatch_claude",
    "dispatch_codex",
    "continue_claude_session",
    "continue_codex_session",
    "exit_claude_session",
    "exit_codex_session",
];

/// 消息是否应保留在 LLM 上下文中（G9-05）。
///
/// 过滤纯调度 plumbing 工具（dispatch_claude 等）的工具结果，以及仅承载
/// 流程状态、对模型决策无意义的 process-only assistant 消息。
/// DB 加载路径（`db::messages::queries::load_llm_history`）直接委托本函数，
/// 「新 run 从 DB 重新加载」因而不存在第二份同口径实现。
/// 装配过滤的完整口径 = 本函数（逐行无状态）+ `strip_delivered_wait_pairs`
///（wait 控制对的成对剔除，需要跨行信息故独立成段）。
pub(crate) fn should_keep_llm_message(message: &ChatMessage) -> bool {
    match message.role.as_str() {
        "assistant" => {
            !is_process_only_assistant_message(&message.content)
                && !is_process_only_assistant_tool_call(message)
        }
        "tool" => !message
            .name
            .as_deref()
            .is_some_and(is_dispatch_plumbing_tool_name),
        _ => true,
    }
}

fn is_process_only_assistant_message(content: &str) -> bool {
    let trimmed = content.trim();
    matches!(
        trimmed,
        "🔄 子任务当前轮次已完成"
            | "✅ 子任务进程已结束"
            | "⚠️ 子任务进程已失败退出"
            | "⏹️ 子任务进程已取消"
            | "🔄 子任务当前轮次已完成，执行结果已同步供后续分析。"
            | "✅ 子任务进程已结束，执行结果已同步供后续分析。"
            | "⚠️ 子任务进程已失败退出，执行结果已同步供后续分析。"
            | "⏹️ 子任务进程已取消，执行结果已同步供后续分析。"
    ) || trimmed.starts_with("📋 已自动批准 ")
        || content.starts_with("📋 已提交 ")
        || content.starts_with("📨 已向 ")
        || content.starts_with("⏹️ 已向 ")
}

fn is_process_only_assistant_tool_call(message: &ChatMessage) -> bool {
    message
        .tool_calls
        .as_ref()
        .is_some_and(|calls| !calls.is_empty() && calls.iter().all(is_dispatch_plumbing_tool_call))
}

fn is_dispatch_plumbing_tool_call(call: &OutboundToolCall) -> bool {
    is_dispatch_plumbing_tool_name(&call.function.name)
}

fn is_dispatch_plumbing_tool_name(name: &str) -> bool {
    DISPATCH_PLUMBING_TOOL_NAMES.contains(&name)
}

/// 剔除「已交付进展」的 wait_for_tools 控制对（纯 wait 的 assistant 调用行 +
/// 配对 tool 结果行）：该次等待交付的事实已由紧随的 runtime 观察消息承载，
/// 控制噪音不回灌上下文。结果 JSON 的 ready 数组非空 = 交付了进展；无进展
/// 结果（no_pending/timeout 空/违规拒绝）与混批 assistant 行一律保留——保留
/// 是模型的纠错反馈，防止上下文逐字节不变导致退化式重复调用。
/// 与运行中内存侧（`rig_ext::loop::decision` 的 made_progress 分支）严格同构。
pub(crate) fn strip_delivered_wait_pairs(messages: &mut Vec<ChatMessage>) {
    let mut delivered_call_ids = std::collections::HashSet::<String>::new();
    for (index, message) in messages.iter().enumerate() {
        if message.role != "assistant" {
            continue;
        }
        let Some(calls) = message.tool_calls.as_ref() else {
            continue;
        };
        if calls.is_empty()
            || !calls
                .iter()
                .all(|call| call.function.name == WAIT_TOOL_NAME)
        {
            continue;
        }
        for call in calls {
            // 配对结果 = 紧随其后的连续 tool 消息中按 call id 匹配
            //（与 repair_tool_call_pairing 的配对口径一致）。
            let delivered = messages[index + 1..]
                .iter()
                .take_while(|msg| msg.role == "tool")
                .find(|msg| msg.tool_call_id.as_deref() == Some(call.id.as_str()))
                .is_some_and(|result| wait_result_made_progress(&result.content));
            if delivered {
                delivered_call_ids.insert(call.id.clone());
            }
        }
    }
    if delivered_call_ids.is_empty() {
        return;
    }
    messages.retain(|message| match message.role.as_str() {
        // 剔除分支必须复核调用名是 wait：delivered_call_ids 的命中只对
        // 纯 wait 批有意义，而 call id 全局唯一仅由服务商行为保证（决策层
        // 只硬校验批内重复）——若后续某行业务调用的 id 恰好碰撞，按 id 匹配
        // 会整行误删业务调用、其结果行沦为孤儿。
        "assistant" => !message.tool_calls.as_ref().is_some_and(|calls| {
            !calls.is_empty()
                && calls.iter().all(|call| {
                    call.function.name == WAIT_TOOL_NAME && delivered_call_ids.contains(&call.id)
                })
        }),
        "tool" => {
            !(message.name.as_deref() == Some(WAIT_TOOL_NAME)
                && message
                    .tool_call_id
                    .as_deref()
                    .is_some_and(|id| delivered_call_ids.contains(id)))
        }
        _ => true,
    });
}

/// wait 结果 JSON 的 ready 数组非空 = 该次等待交付了进展。解析失败一律视为
/// 无进展（保留，fail-safe）。
fn wait_result_made_progress(content: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(content)
        .ok()
        .and_then(|value| {
            value
                .get("ready")?
                .as_array()
                .map(|ready| !ready.is_empty())
        })
        .unwrap_or(false)
}

// ─── LLM Context Repair ────────────────────────────────────────────────────────

/// 未应答工具调用的占位结果文案（模型据此知道该结果不存在、可重新调用）。
/// rig `Message` 侧的整形兜底（`rig_ext::context::repair_pairing`）复用同一文案。
pub(crate) const UNANSWERED_TOOL_RESULT_PLACEHOLDER: &str =
    "（该工具调用没有产生结果：运行被中断，结果未落库。若仍需要，请重新调用。）";

/// 修复「assistant tool_calls ↔ tool 结果」配对（全仓唯一实现）。
///
/// 历史里可能缺少某个调用的结果行——run 在批量中途被取消、进程被杀、前序工具
/// 致命失败中止——也可能留下没有对应 assistant 的孤儿结果（其 assistant 被
/// `should_keep_llm_message` 过滤掉）。两者都会让服务端以 400 拒绝整轮请求
/// （assistant 的 tool_calls 之后必须跟齐 tool 消息），因此装配上下文前必须按
/// 调用顺序补齐缺失结果、剔除孤儿结果。写侧由运行循环保证成对落库（含取消/致命
/// 失败的占位补齐），此处只是读侧防御校验：触发即留痕——频繁出现说明写侧回归。
/// 库中既有的残缺历史无需数据迁移即可继续使用。
pub(crate) fn repair_tool_call_pairing(messages: &mut Vec<ChatMessage>) {
    let mut pending: Vec<ChatMessage> = std::mem::take(messages);
    pending.reverse();
    let mut repaired: Vec<ChatMessage> = Vec::with_capacity(pending.len());
    let mut placeholders_added = 0usize;
    let mut orphans_dropped = 0usize;

    while let Some(message) = pending.pop() {
        if message.role != "assistant" {
            // 排在 assistant 之外的工具结果是孤儿：没有前置 tool_calls 可应答。
            if message.role != "tool" {
                repaired.push(message);
            } else {
                orphans_dropped += 1;
            }
            continue;
        }

        let calls = message.tool_calls.clone().unwrap_or_default();
        if calls.is_empty() {
            repaired.push(message);
            continue;
        }

        let mut results: Vec<ChatMessage> = Vec::new();
        while pending.last().is_some_and(|next| next.role == "tool") {
            if let Some(result) = pending.pop() {
                results.push(result);
            }
        }

        repaired.push(message);
        for call in &calls {
            let matched = results
                .iter()
                .position(|result| result.tool_call_id.as_deref() == Some(call.id.as_str()));
            match matched {
                Some(index) => repaired.push(results.remove(index)),
                None => {
                    placeholders_added += 1;
                    repaired.push(unanswered_tool_result(&call.id, &call.function.name));
                }
            }
        }
        // 其余结果没有对应的 tool_call：一并丢弃，避免出现响应错位的工具消息。
        orphans_dropped += results.len();
    }

    if placeholders_added > 0 || orphans_dropped > 0 {
        eprintln!(
            "repair_tool_call_pairing 触发防御修复：补占位 {placeholders_added} 条、\
             剔除孤儿结果 {orphans_dropped} 条（库中残缺历史）。写侧已保证配对，\
             若新会话频繁出现此日志说明写侧回归"
        );
    }
    *messages = repaired;
}

/// 补齐用的占位工具结果：只承载「未产生结果」这一事实，不伪装成执行错误。
fn unanswered_tool_result(tool_call_id: &str, tool_name: &str) -> ChatMessage {
    ChatMessage {
        role: "tool".to_string(),
        content: UNANSWERED_TOOL_RESULT_PLACEHOLDER.to_string(),
        content_parts: Vec::new(),
        reasoning_content: None,
        tool_calls: None,
        tool_call_id: Some(tool_call_id.to_string()),
        name: Some(tool_name.to_string()),
        source_id: None,
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::db::FunctionCall;

    fn wait_assistant(call_id: &str) -> ChatMessage {
        ChatMessage {
            role: "assistant".into(),
            content: String::new(),
            content_parts: Vec::new(),
            reasoning_content: None,
            tool_calls: Some(vec![OutboundToolCall {
                id: call_id.into(),
                kind: "function".into(),
                function: FunctionCall {
                    name: WAIT_TOOL_NAME.into(),
                    arguments: "{}".into(),
                },
            }]),
            tool_call_id: None,
            name: None,
            source_id: None,
        }
    }

    fn tool_result(call_id: &str, name: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: "tool".into(),
            content: content.into(),
            content_parts: Vec::new(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: Some(call_id.into()),
            name: Some(name.into()),
            source_id: None,
        }
    }

    fn plain(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: role.into(),
            content: content.into(),
            content_parts: Vec::new(),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            source_id: None,
        }
    }

    #[test]
    fn delivered_wait_pair_is_stripped_and_runtime_kept() {
        let mut messages = vec![
            plain("user", "查一下"),
            wait_assistant("w1"),
            tool_result(
                "w1",
                WAIT_TOOL_NAME,
                r#"{"reason":"tools_ready","ready":[{"task_id":"t1","tool":"read_file","status":"succeeded"}],"pending_count":0}"#,
            ),
            plain(
                "runtime",
                r#"{"kind":"tool_completion","task_id":"t1","status":"succeeded"}"#,
            ),
            plain("assistant", "结论"),
        ];
        strip_delivered_wait_pairs(&mut messages);
        let roles: Vec<&str> = messages.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles, ["user", "runtime", "assistant"]);
    }

    #[test]
    fn no_progress_wait_pair_is_kept_as_feedback() {
        for content in [
            r#"{"reason":"no_pending_tasks","ready":[],"pending_count":0}"#,
            r#"{"reason":"timeout","ready":[],"pending_count":2}"#,
            "控制工具必须独占一批",
            "不是 JSON 的历史遗留文本",
        ] {
            let mut messages = vec![
                wait_assistant("w1"),
                tool_result("w1", WAIT_TOOL_NAME, content),
            ];
            strip_delivered_wait_pairs(&mut messages);
            assert_eq!(messages.len(), 2, "无进展/异常结果应保留：{content}");
        }
    }

    #[test]
    fn mixed_batch_assistant_and_its_wait_result_are_kept() {
        // 混批（wait + 业务调用）的 assistant 行不是纯 wait：即使 wait 结果
        // 显示有进展，整批拒绝的纠错语义必须完整保留，配对不被拆散。
        let mut mixed = wait_assistant("w1");
        mixed.tool_calls.as_mut().unwrap().push(OutboundToolCall {
            id: "r1".into(),
            kind: "function".into(),
            function: FunctionCall {
                name: "read_file".into(),
                arguments: "{}".into(),
            },
        });
        let mut messages = vec![
            mixed,
            tool_result(
                "w1",
                WAIT_TOOL_NAME,
                r#"{"reason":"tools_ready","ready":[{"task_id":"t1"}],"pending_count":0}"#,
            ),
            tool_result("r1", "read_file", "文件内容"),
        ];
        strip_delivered_wait_pairs(&mut messages);
        assert_eq!(messages.len(), 3);
    }

    #[test]
    fn strip_then_repair_leaves_no_orphans_or_placeholders() {
        // 端到端口径：剔除后紧跟配对修复，结果应既无孤儿结果也无占位补齐。
        let mut messages = vec![
            plain("user", "查一下"),
            wait_assistant("w1"),
            tool_result(
                "w1",
                WAIT_TOOL_NAME,
                r#"{"reason":"tools_ready","ready":[{"task_id":"t1"}],"pending_count":0}"#,
            ),
            plain("runtime", "{}"),
        ];
        strip_delivered_wait_pairs(&mut messages);
        repair_tool_call_pairing(&mut messages);
        assert_eq!(messages.len(), 2);
        assert!(messages.iter().all(|m| m.role != "tool"));
    }

    #[test]
    fn delivered_wait_id_collision_never_strips_business_rows() {
        // call id 全局唯一仅由服务商行为保证（决策层只硬校验批内重复）：
        // 退化 id（如空 id / 跨轮重复）下，业务行的 id 恰好命中已交付 wait
        // 的 id 时，剔除只允许作用于纯 wait 行——业务行与其结果必须原样保留。
        let mut business = wait_assistant("dup");
        business.tool_calls.as_mut().unwrap()[0].function.name = "read_file".into();
        let mut messages = vec![
            wait_assistant("dup"),
            tool_result(
                "dup",
                WAIT_TOOL_NAME,
                r#"{"reason":"tools_ready","ready":[{"task_id":"t1"}],"pending_count":0}"#,
            ),
            business,
            tool_result("dup", "read_file", "业务结果"),
        ];
        strip_delivered_wait_pairs(&mut messages);
        assert_eq!(messages.len(), 2, "只有 wait 对被剔除，业务行不得误删");
        assert_eq!(messages[0].role, "assistant");
        assert_eq!(
            messages[0].tool_calls.as_ref().unwrap()[0].function.name,
            "read_file"
        );
        assert_eq!(messages[1].role, "tool");
        assert_eq!(messages[1].name.as_deref(), Some("read_file"));
    }
}
