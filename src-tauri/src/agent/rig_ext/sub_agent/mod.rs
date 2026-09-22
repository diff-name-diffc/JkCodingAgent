//! 子智能体运行时（rig 形态）：`SubAgentRuntime` 的替代实现。
//!
//! 与主 Agent 的区别（与旧实现一致）：无图编排、无协议动作，是纯粹的
//! 「LLM ↔ 工具」循环；结果截断后返回父循环；轨迹事件单独下发。

mod context;
mod events;
mod runner;
mod tools;

// 子智能体工具与事件的接入点是 T3.1 的 chat agent 装配；接入后移除 allow。
#[allow(unused_imports)]
pub use events::{SubAgentEvent, SubAgentEventPayload, SubAgentUsage};
#[allow(unused_imports)]
pub use runner::{RigSubAgentRequest, RigSubAgentRuntime};
#[allow(unused_imports)]
pub use tools::{call_sub_agent_tool, list_sub_agents_tool, notify_user_progress_tool};
