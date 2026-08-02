//! Agent 统一运行骨架。
//!
//! 这个模块只承载“如何跑一轮 Agent”：入口请求、事件、对话内存循环、
//! `AgentRunAdapter` / `RunLoopAgent` trait，以及公共工具迭代循环。
//! 具体 Agent 差异放在 `agent::agents::*`，避免公共骨架反向掺入业务形态。

pub(crate) mod agent_loop;
pub(crate) mod core;
pub(crate) mod types;

pub(crate) use core::{run_agent_turn, AgentRunRequest, RuntimeAgentKind};
pub use types::{AgentEvent, AgentTurn};
