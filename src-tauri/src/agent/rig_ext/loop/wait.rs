//! `wait_for_tools` 控制工具的统一语义：参数解析、工具定义、等待结果与
//! 在途快照渲染。主循环与子智能体循环共用本模块，禁止再各自维护字面量副本。
use super::scheduler::PendingTask;
use crate::agent::db::tool_completions::ToolCompletion;
use std::{collections::HashSet, time::Duration};

// `WAIT_TOOL_NAME` 的唯一出处在 `crate::agent::common::message`（LLM 上下文
// 过滤层需要它识别历史控制对），此处 re-export 供决策循环统一引用。
pub(crate) use crate::agent::common::WAIT_TOOL_NAME;

/// 控制工具混批/独占违规的拒绝文案（主循环与子智能体共用同一口径）。
pub(crate) const EXCLUSIVE_BATCH_VIOLATION: &str = "控制工具必须独占一批";

/// 等待目标的结算模式。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum WaitMode {
    /// 目标全部结算才唤醒（join 语义，默认）：fan-out 后等齐结果一次唤醒。
    All,
    /// 任一目标结算即唤醒：需要尽早响应首个完成时使用。
    Any,
}

/// 一次等待的唤醒条件（由 `parse_wait_args` 从调用参数解析，或代码直接构造）。
#[derive(Debug, Clone)]
pub(crate) struct WaitCondition {
    /// 只等待这些任务（tool_run_id）；None = 进入等待时的全部未交付任务。
    pub targets: Option<HashSet<String>>,
    pub mode: WaitMode,
    pub timeout: Option<Duration>,
}

impl WaitCondition {
    /// 隐式等待（模型给出文本答复但仍有在途任务）：任一结算即唤醒。
    pub(crate) fn any_pending() -> Self {
        Self {
            targets: None,
            mode: WaitMode::Any,
            timeout: None,
        }
    }
}

/// 宽容解析（模型面向）：缺字段/非法值一律回退默认，不因参数问题拒绝等待。
pub(crate) fn parse_wait_args(args: &serde_json::Value) -> WaitCondition {
    let targets = args
        .get("task_ids")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect::<HashSet<_>>()
        })
        .filter(|set: &HashSet<String>| !set.is_empty());
    let mode = match args.get("mode").and_then(serde_json::Value::as_str) {
        Some("any") => WaitMode::Any,
        _ => WaitMode::All,
    };
    let timeout = args
        .get("timeout_secs")
        .and_then(serde_json::Value::as_u64)
        .map(|secs| Duration::from_secs(secs.clamp(1, 3600)));
    WaitCondition {
        targets,
        mode,
        timeout,
    }
}

/// wait_for_tools 的唤醒原因（结果 JSON 的 reason 字段）。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum WaitReason {
    /// 条件满足，有就绪结算待交付。
    Ready,
    /// 没有在途任务（竞态兜底：工具面只在有未结算任务时才注入该工具）。
    NoPendingTasks,
    /// 调用声明的 timeout_secs 到点。
    Timeout,
    /// 条件未满足且已无在途 worker（防御兜底：结算监督已移交后台等极端
    /// 场景）。交回决策权、由循环顶部交付现有结果，但如实区别于 Ready，
    /// 避免「reason=tools_ready 而 ready 为空」的自相矛盾反馈。
    NoActiveWorkers,
}

impl WaitReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "tools_ready",
            Self::NoPendingTasks => "no_pending_tasks",
            Self::Timeout => "timeout",
            Self::NoActiveWorkers => "no_active_workers",
        }
    }
}

pub(crate) fn wait_tool_definition() -> rig::completion::ToolDefinition {
    rig::completion::ToolDefinition {
        name: WAIT_TOOL_NAME.into(),
        description: "暂停决策并等待在途工具任务结算，条件满足时宿主自动唤醒（不要轮询、不要用 sleep 代替）。仅当后续每一步都依赖在途结果时才调用；还有独立工作就继续做。默认等待全部在途任务完成（join 语义）；传 task_ids 只等指定子集；mode=any 在任一完成时尽早唤醒。必须独占一批调用。".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "task_ids": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "只等待这些任务（id 取自 accepted 回执或在途任务快照）；缺省为全部在途任务"
                },
                "mode": {
                    "type": "string",
                    "enum": ["all", "any"],
                    "description": "all=目标全部完成才唤醒（默认）；any=任一完成即唤醒"
                },
                "timeout_secs": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 3600,
                    "description": "可选的最长等待秒数；到点以 timeout 唤醒并交回决策权"
                }
            },
            "additionalProperties": false
        }),
    }
}

/// wait 调用的结果文本：结构化摘要（唤醒原因 + 就绪任务清单 + 剩余在途数）。
/// `ready` 传尚未交付的就绪结算；完整正文仍由协调器经统一 runtime 消息通道交付。
/// reason 取 `WaitReason::as_str` 或控制违规文案——两种来源的 `ready` 均为空
/// 时读侧装配（`strip_delivered_wait_pairs`）会把该控制对保留在上下文中作反馈。
pub(crate) fn render_wait_result(
    reason: &str,
    ready: &[&ToolCompletion],
    pending_count: usize,
) -> String {
    serde_json::json!({
        "reason": reason,
        "ready": ready
            .iter()
            .map(|completion| serde_json::json!({
                "task_id": completion.tool_run_id,
                "tool": completion.tool_name,
                "status": completion.status,
            }))
            .collect::<Vec<_>>(),
        "pending_count": pending_count,
    })
    .to_string()
}

/// 在途任务快照（注入每轮 preamble）：确定性生成，不调 LLM、不读 DB。
/// 无在途任务时返回 None（该轮 preamble 不附加段落）。
pub(crate) fn render_pending_snapshot(pending: &[PendingTask]) -> Option<String> {
    if pending.is_empty() {
        return None;
    }
    const MAX_LINES: usize = 16;
    let mut out = String::from(
        "## 在途工具任务\n\n以下任务仍在执行，完成时其 tool_completion 观察会自动送达，不要轮询；\
        仅当后续每一步都依赖其结果时才调用 wait_for_tools 等待（可按 task_id 指定子集）：",
    );
    for task in pending.iter().take(MAX_LINES) {
        out.push_str(&format!(
            "\n- {} {} · {} · 已耗时 {}s",
            task.task_id,
            task.tool_name,
            if task.running { "running" } else { "queued" },
            task.elapsed.as_secs()
        ));
    }
    if pending.len() > MAX_LINES {
        out.push_str(&format!("\n- …另 {} 项", pending.len() - MAX_LINES));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_wait_args_defaults_to_join_all_pending() {
        let condition = parse_wait_args(&serde_json::json!({}));
        assert_eq!(condition.mode, WaitMode::All);
        assert!(condition.targets.is_none());
        assert!(condition.timeout.is_none());
    }

    #[test]
    fn parse_wait_args_reads_subset_mode_and_timeout() {
        let condition = parse_wait_args(&serde_json::json!({
            "task_ids": ["a", "b"],
            "mode": "any",
            "timeout_secs": 30,
        }));
        assert_eq!(condition.mode, WaitMode::Any);
        let targets = condition.targets.expect("应解析出目标集");
        assert!(targets.contains("a") && targets.contains("b"));
        assert_eq!(condition.timeout, Some(Duration::from_secs(30)));
    }

    #[test]
    fn parse_wait_args_tolerates_invalid_values() {
        let condition = parse_wait_args(&serde_json::json!({
            "task_ids": [],
            "mode": "everything",
            "timeout_secs": 999_999,
        }));
        assert_eq!(condition.mode, WaitMode::All);
        assert!(condition.targets.is_none(), "空数组视为未指定");
        assert_eq!(condition.timeout, Some(Duration::from_secs(3600)));
        // 非对象输入不 panic。
        let condition = parse_wait_args(&serde_json::json!("not-an-object"));
        assert_eq!(condition.mode, WaitMode::All);
    }

    #[test]
    fn render_pending_snapshot_is_none_when_empty() {
        assert!(render_pending_snapshot(&[]).is_none());
    }

    #[test]
    fn render_pending_snapshot_lists_tasks_and_caps_lines() {
        let pending = (0..20)
            .map(|i| PendingTask {
                task_id: format!("task-{i}"),
                tool_name: "read_file".into(),
                running: i % 2 == 0,
                elapsed: Duration::from_secs(i),
            })
            .collect::<Vec<_>>();
        let snapshot = render_pending_snapshot(&pending).expect("有在途任务应有快照");
        assert!(snapshot.contains("## 在途工具任务"));
        assert!(snapshot.contains("- task-0 read_file · running · 已耗时 0s"));
        assert!(snapshot.contains("- task-1 read_file · queued · 已耗时 1s"));
        assert!(!snapshot.contains("task-16"), "超过 16 行应截断");
        assert!(snapshot.contains("…另 4 项"));
    }

    #[test]
    fn render_wait_result_is_structured_json() {
        let completion = ToolCompletion {
            event_id: 1,
            tool_run_id: "task-a".into(),
            tool_name: "local_zsh".into(),
            tool_call_id: "call-a".into(),
            dispatch_round: 0,
            agent_run_id: "run".into(),
            scope_id: "scope".into(),
            status: "succeeded".into(),
            error_kind: None,
            fatal: false,
            retryable: false,
            display_content: "done".into(),
            context_payload: "payload".into(),
            result_mode: "raw".into(),
            usage_json: None,
            delivery_message_id: None,
            observed_request_step: None,
        };
        let text = render_wait_result(WaitReason::Ready.as_str(), &[&completion], 2);
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("结果应为合法 JSON");
        assert_eq!(parsed["reason"], "tools_ready");
        assert_eq!(parsed["ready"][0]["task_id"], "task-a");
        assert_eq!(parsed["ready"][0]["tool"], "local_zsh");
        assert_eq!(parsed["pending_count"], 2);
    }
}
