use serde::Serialize;

use super::super::db::{
    DispatcherMessageRecord, DispatcherMessageUsageStats, DispatcherToolArtifactRef,
    DispatcherToolRunRecord,
};

/// 一轮 Agent 运行的收口结果。
///
/// G7-11：不再携带全量可见消息列表——完整消息由前端在收到 `Finished` 事件后
/// 通过 `dispatcher_list_messages` 命令拉取，避免 invoke 返回值与事件负载重复
/// 携带大体积 segments_json/context_payload。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurn {
    pub reply: DispatcherMessageRecord,
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
    /// 助手正文增量。
    ///
    /// G9-08：`seq` 为同一 `message_id` 内的单调递增序号（从 0 开始，与
    /// `AssistantThinkingDelta` 共享同一计数器），供前端在重连重放 / 乱序场景
    /// 做去重与完整性校验；完成时的末序号随 `AssistantMessage.last_seq` 下发。
    AssistantDelta {
        message_id: String,
        seq: u64,
        delta: String,
    },
    /// 助手思考增量。序号语义同 `AssistantDelta`（共享同一 message_id 计数器）。
    AssistantThinkingDelta {
        message_id: String,
        seq: u64,
        delta: String,
        elapsed_ms: u64,
    },
    /// 助手消息完成并落库。
    ///
    /// `last_seq` 为该消息对应流式过程实际发出的最后一个 delta 序号；
    /// 无关联流式输出（如工具循环后的合成收口消息）时为 None。
    AssistantMessage {
        message: DispatcherMessageRecord,
        last_seq: Option<u64>,
    },
    RunUsageUpdated {
        workspace_id: String,
        stats: DispatcherMessageUsageStats,
    },
    /// 工具调用已排入执行计划。
    ///
    /// G9-07：`tool_call_id` 必填——由 LLM 响应携带，贯穿
    /// Planned → Started → Finished 全链路，前端据此配对同一工具调用的事件。
    ToolPlanned {
        tool_call_id: String,
        name: String,
        arguments: String,
    },
    /// 工具开始执行。字段语义与 `ToolPlanned` 相同，`tool_call_id` 必填（G9-07）。
    ToolStarted {
        tool_call_id: String,
        name: String,
        arguments: String,
    },
    /// 预留事件：工具结果流式摘要开始。
    ///
    /// 当前主流程未接入（摘要在工具完成后整体产出，随 `ToolFinished` 一次性下发），
    /// 保留为协议预留位，前端已按此事件名实现处理。启用条件：将
    /// `persist_tool_result_with_compression` 的摘要生成改造成流式时接入。
    /// 约束：启用时 `result_mode` 取值必须与最终 `ToolFinished` 完全一致，
    /// 否则前端会收到互相矛盾的流式事件。
    #[allow(dead_code)]
    ToolSummaryStarted {
        tool_call_id: String,
        name: String,
        result_mode: String,
    },
    /// 预留事件：工具结果流式摘要增量。启用条件与约束同 `ToolSummaryStarted`。
    #[allow(dead_code)]
    ToolSummaryDelta {
        tool_call_id: String,
        name: String,
        delta: String,
        result_mode: String,
    },
    /// 工具执行完成。
    ///
    /// G9-07：`tool_call_id` 必填；补 `arguments`（schema 补全后的 effective
    /// 参数 JSON）与 Planned/Started 字段对称，前端无需缓存 Started 即可展示入参。
    ToolFinished {
        tool_call_id: String,
        name: String,
        arguments: String,
        display_text: String,
        /// 实际回灌给调用方 Agent 的工具结果。压缩模式下它通常比
        /// `display_text` 更详细；前端必须展示这份内容，确保可审计。
        context_payload: String,
        result_mode: String,
        detail_refs: Vec<DispatcherToolArtifactRef>,
    },
    ToolRunUpdated {
        run: DispatcherToolRunRecord,
    },
    /// 一轮运行成功收口。
    ///
    /// G7-11：轻量负载——不再携带全量消息（含 segments_json/context_payload），
    /// 仅携带 workspace_id 与可见消息总数供前端对账；前端收到后自行调用
    /// `dispatcher_list_messages` 拉全量刷新。事件名保持 `finished` 不变。
    Finished {
        workspace_id: String,
        message_count: usize,
    },
    Failed {
        workspace_id: String,
        message: String,
    },
}
