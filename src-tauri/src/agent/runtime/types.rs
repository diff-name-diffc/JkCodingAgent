use serde::Serialize;

use super::super::db::{
    ChecklistPlanState, DispatcherMessageRecord, DispatcherMessageUsageStats,
    DispatcherToolArtifactRef, PlanInteraction,
};

/// Feedback state reported by a subprocess when it yields control back to the
/// dispatcher.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DispatchFeedbackState {
    RoundCompleted,
    ProcessDone,
    ProcessFailed,
    ProcessCancelled,
}

impl DispatchFeedbackState {
    pub fn from_wire(value: &str) -> Self {
        match value {
            "process_done" => Self::ProcessDone,
            "process_failed" => Self::ProcessFailed,
            "process_cancelled" => Self::ProcessCancelled,
            _ => Self::RoundCompleted,
        }
    }

    pub fn visible_message(self) -> &'static str {
        match self {
            Self::RoundCompleted => "🔄 子任务当前轮次已完成",
            Self::ProcessDone => "✅ 子任务进程已结束",
            Self::ProcessFailed => "⚠️ 子任务进程已失败退出",
            Self::ProcessCancelled => "⏹️ 子任务进程已取消",
        }
    }

    pub fn hidden_prefix(self) -> &'static str {
        match self {
            Self::RoundCompleted => {
                "[系统通知] 子任务当前轮次已完成，但子进程仍在运行，可继续注入后续指令，也可在确认无需继续后主动退出。请先分析执行状态，再决定下一步："
            }
            Self::ProcessDone => "[系统通知] 子任务进程已结束。请根据以下执行结果总结反馈：",
            Self::ProcessFailed => {
                "[系统通知] 子任务进程已失败退出。请根据以下执行结果分析问题并决定下一步："
            }
            Self::ProcessCancelled => {
                "[系统通知] 子任务进程已取消。请根据以下执行结果判断是否需要重试或调整方案："
            }
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurn {
    pub reply: DispatcherMessageRecord,
    pub messages: Vec<DispatcherMessageRecord>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "event",
    content = "data"
)]
pub enum AgentEvent {
    Started {
        workspace_id: String,
    },
    UserMessage {
        message: DispatcherMessageRecord,
    },
    AssistantStarted {
        message_id: String,
    },
    ModelSwitched {
        from_model: String,
        to_model: String,
        reason: String,
    },
    AssistantDelta {
        message_id: String,
        delta: String,
    },
    AssistantThinkingDelta {
        message_id: String,
        delta: String,
        elapsed_ms: u64,
    },
    AssistantMessage {
        message: DispatcherMessageRecord,
    },
    RunUsageUpdated {
        workspace_id: String,
        stats: DispatcherMessageUsageStats,
    },
    ToolPlanned {
        tool_call_id: Option<String>,
        name: String,
        arguments: String,
    },
    ToolStarted {
        tool_call_id: Option<String>,
        name: String,
        arguments: String,
    },
    #[allow(dead_code)]
    ToolSummaryStarted {
        tool_call_id: Option<String>,
        name: String,
        result_mode: String,
    },
    #[allow(dead_code)]
    ToolSummaryDelta {
        tool_call_id: Option<String>,
        name: String,
        delta: String,
        result_mode: String,
    },
    ToolFinished {
        tool_call_id: Option<String>,
        name: String,
        display_text: String,
        result_mode: String,
        detail_refs: Vec<DispatcherToolArtifactRef>,
    },
    ChecklistPlanUpdated {
        state: ChecklistPlanState,
    },
    PlanQuestionRequested {
        interaction: PlanInteraction,
    },
    PlanDocumentOpened {
        plan_path: String,
    },
    PlanReady {
        interaction: PlanInteraction,
    },
    PlanImplemented {
        plan_path: String,
        implemented_path: String,
        summary: String,
    },
    DispatchProposed {
        dispatch_id: String,
        agent: String,
        description: String,
        task_prompt: String,
        permission_mode: String,
    },
    DispatchContinue {
        dispatch_id: String,
        agent: String,
        text: String,
    },
    DispatchExit {
        dispatch_id: String,
        agent: String,
        reason: String,
    },
    Finished {
        messages: Vec<DispatcherMessageRecord>,
    },
}
