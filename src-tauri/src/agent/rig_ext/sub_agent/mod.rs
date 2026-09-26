//! 子智能体运行时（rig 形态）：`SubAgentRuntime` 的替代实现。
//!
//! 与主 Agent 的区别（与旧实现一致）：无图编排、无协议动作，是纯粹的
//! 「LLM ↔ 工具」循环；结果截断后返回父循环；轨迹事件单独下发。

mod events;
mod failure;
mod loop_events;
mod runner;
mod tools;

// 子智能体内部（runner/tools/events）相互直接引用；对外只导出父 Agent 装配
// 需要的两个工具构造器。
pub use tools::{call_sub_agent_tool, list_sub_agents_tool, notify_user_progress_tool};
