use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

use super::super::tools::DispatchAgent;

// ─── Subprocess Registry ──────────────────────────────────────────────────────

#[derive(Default)]
pub(crate) struct DispatcherSubprocessRegistry {
    subprocesses: Mutex<Vec<RegisteredSubprocess>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RegisteredSubprocessPhase {
    Running,
    RoundCompleted,
    Stopped,
    ExitRequested,
}

#[derive(Clone, Debug)]
pub(crate) struct RegisteredSubprocess {
    pub workspace_id: String,
    pub task_id: String,
    pub dispatch_id: String,
    pub agent: String,
    pub description: String,
    pub phase: RegisteredSubprocessPhase,
    pub force_idle: Arc<AtomicBool>,
}

impl DispatcherSubprocessRegistry {
    pub(crate) fn register(
        &self,
        workspace_id: &str,
        task_id: &str,
        dispatch_id: &str,
        agent: &str,
        description: &str,
    ) -> Arc<AtomicBool> {
        let mut subprocesses = self.subprocesses.lock();
        let force_idle = subprocesses
            .iter()
            .find(|item| item.task_id == task_id)
            .map(|item| Arc::clone(&item.force_idle))
            .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));

        subprocesses.retain(|item| !(item.workspace_id == workspace_id && item.agent == agent));
        subprocesses.push(RegisteredSubprocess {
            workspace_id: workspace_id.to_string(),
            task_id: task_id.to_string(),
            dispatch_id: dispatch_id.to_string(),
            agent: agent.to_string(),
            description: description.to_string(),
            phase: RegisteredSubprocessPhase::Running,
            force_idle: Arc::clone(&force_idle),
        });
        force_idle
    }

    pub(crate) fn mark_round_completed(&self, task_id: &str) {
        self.update_phase(task_id, RegisteredSubprocessPhase::RoundCompleted);
    }

    pub(crate) fn mark_running(&self, task_id: &str) {
        self.update_phase(task_id, RegisteredSubprocessPhase::Running);
    }

    pub(crate) fn mark_stopped(&self, task_id: &str) {
        self.update_phase(task_id, RegisteredSubprocessPhase::Stopped);
    }

    pub(crate) fn mark_exit_requested(&self, task_id: &str) {
        self.update_phase(task_id, RegisteredSubprocessPhase::ExitRequested);
    }

    pub(crate) fn mark_finished(&self, task_id: &str) {
        let mut subprocesses = self.subprocesses.lock();
        subprocesses.retain(|item| item.task_id != task_id);
    }

    pub(crate) fn force_idle(&self, task_id: &str) {
        if let Some(item) = self
            .subprocesses
            .lock()
            .iter()
            .find(|item| item.task_id == task_id)
        {
            item.force_idle.store(true, Ordering::Release);
        }
    }

    pub(crate) fn is_exit_requested(&self, task_id: &str) -> bool {
        self.subprocesses.lock().iter().any(|item| {
            item.task_id == task_id && item.phase == RegisteredSubprocessPhase::ExitRequested
        })
    }

    pub(crate) fn list_for_workspace(&self, workspace_id: &str) -> Vec<RegisteredSubprocess> {
        self.subprocesses
            .lock()
            .iter()
            .filter(|item| item.workspace_id == workspace_id)
            .cloned()
            .collect()
    }

    pub(crate) fn set_exit_requested_for(&self, workspace_id: &str, agent: &str) {
        let mut subprocesses = self.subprocesses.lock();
        if let Some(item) = subprocesses
            .iter_mut()
            .find(|item| item.workspace_id == workspace_id && item.agent == agent)
        {
            item.phase = RegisteredSubprocessPhase::ExitRequested;
        }
    }

    fn update_phase(&self, task_id: &str, phase: RegisteredSubprocessPhase) {
        let mut subprocesses = self.subprocesses.lock();
        if let Some(item) = subprocesses.iter_mut().find(|item| item.task_id == task_id) {
            item.phase = phase;
        }
    }
}

pub(crate) fn subprocess_phase_label(phase: RegisteredSubprocessPhase) -> &'static str {
    match phase {
        RegisteredSubprocessPhase::Running => "running",
        RegisteredSubprocessPhase::RoundCompleted => "round_completed",
        RegisteredSubprocessPhase::Stopped => "stopped",
        RegisteredSubprocessPhase::ExitRequested => "exit_requested",
    }
}

// ─── Protocol Batch State ─────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub(crate) enum PlannedSubprocessState {
    Active {
        dispatch_id: String,
        phase: RegisteredSubprocessPhase,
    },
    PendingDispatch {
        dispatch_id: String,
    },
}

#[derive(Debug)]
pub(crate) struct ProtocolBatchState {
    by_agent: HashMap<String, PlannedSubprocessState>,
}

#[derive(Clone, Debug)]
pub(crate) enum ProtocolToolAction {
    Dispatch {
        dispatch_id: String,
        agent: DispatchAgent,
        description: String,
        task_prompt: String,
        permission_mode: String,
    },
    Continue {
        dispatch_id: String,
        agent: DispatchAgent,
        text: String,
    },
    Exit {
        dispatch_id: String,
        agent: DispatchAgent,
        reason: String,
    },
}

impl ProtocolBatchState {
    pub(crate) fn new(subprocesses: Vec<RegisteredSubprocess>) -> Self {
        let by_agent = subprocesses
            .into_iter()
            .map(|item| {
                (
                    item.agent,
                    PlannedSubprocessState::Active {
                        dispatch_id: item.dispatch_id,
                        phase: item.phase,
                    },
                )
            })
            .collect();

        Self { by_agent }
    }

    pub(crate) fn dispatch_id_for_agent(&self, agent: &str) -> Option<&str> {
        match self.by_agent.get(agent) {
            Some(PlannedSubprocessState::Active { dispatch_id, .. })
            | Some(PlannedSubprocessState::PendingDispatch { dispatch_id }) => Some(dispatch_id),
            None => None,
        }
    }

    pub(crate) fn ensure_dispatch_allowed(
        &self,
        agent: &str,
        agent_label: &str,
    ) -> std::result::Result<(), String> {
        match self.by_agent.get(agent) {
            Some(PlannedSubprocessState::Active { dispatch_id, phase }) => Err(format!(
                "错误：当前会话已有一个活跃的 {agent_label} 子进程（dispatch_id={}, phase={}）。禁止再次调用 dispatch_{}；请改用 continue_{}_session、exit_{}_session，或直接回复用户。",
                dispatch_id,
                subprocess_phase_label(*phase),
                agent,
                agent,
                agent
            )),
            Some(PlannedSubprocessState::PendingDispatch { dispatch_id }) => Err(format!(
                "错误：当前轮已为 {agent_label} 规划一个待启动子任务（dispatch_id={}）。禁止重复调用 dispatch_{}；请等待该子任务启动后再继续协调。",
                dispatch_id, agent
            )),
            None => Ok(()),
        }
    }

    pub(crate) fn record_dispatch(&mut self, agent: &str, dispatch_id: &str) {
        self.by_agent.insert(
            agent.to_string(),
            PlannedSubprocessState::PendingDispatch {
                dispatch_id: dispatch_id.to_string(),
            },
        );
    }

    pub(crate) fn ensure_continue_allowed(
        &self,
        agent: &str,
        agent_label: &str,
    ) -> std::result::Result<(), String> {
        match self.by_agent.get(agent) {
            Some(PlannedSubprocessState::Active {
                phase:
                    RegisteredSubprocessPhase::Running | RegisteredSubprocessPhase::RoundCompleted,
                ..
            }) => Ok(()),
            Some(PlannedSubprocessState::Active {
                phase: RegisteredSubprocessPhase::Stopped,
                ..
            }) => Err(format!(
                "错误：{agent_label} 子进程当前处于 stopped 状态，请先由 UI 恢复运行后再继续注入指令。"
            )),
            Some(PlannedSubprocessState::Active {
                phase: RegisteredSubprocessPhase::ExitRequested,
                ..
            }) => Err(format!(
                "错误：{agent_label} 子进程已收到退出请求，当前只能等待其结束，不能再继续注入指令。"
            )),
            Some(PlannedSubprocessState::PendingDispatch { .. }) => Err(format!(
                "错误：{agent_label} 子任务已在当前轮提出但尚未真正启动，当前不能继续注入指令。"
            )),
            None => Err(format!(
                "错误：当前会话没有可继续的 {agent_label} 活跃子进程。"
            )),
        }
    }

    pub(crate) fn record_continue(&mut self, agent: &str) {
        if let Some(PlannedSubprocessState::Active { phase, .. }) = self.by_agent.get_mut(agent) {
            *phase = RegisteredSubprocessPhase::Running;
        }
    }

    pub(crate) fn ensure_exit_allowed(
        &self,
        agent: &str,
        agent_label: &str,
    ) -> std::result::Result<(), String> {
        match self.by_agent.get(agent) {
            Some(PlannedSubprocessState::Active {
                phase:
                    RegisteredSubprocessPhase::Running | RegisteredSubprocessPhase::RoundCompleted,
                ..
            }) => Ok(()),
            Some(PlannedSubprocessState::Active {
                phase: RegisteredSubprocessPhase::Stopped,
                ..
            }) => Err(format!(
                "错误：{agent_label} 子进程当前处于 stopped 状态，请先恢复运行后再决定是否退出。"
            )),
            Some(PlannedSubprocessState::Active {
                phase: RegisteredSubprocessPhase::ExitRequested,
                ..
            }) => Err(format!(
                "错误：{agent_label} 子进程已经收到退出命令，请等待进程结束，不要重复 exit。"
            )),
            Some(PlannedSubprocessState::PendingDispatch { .. }) => Err(format!(
                "错误：{agent_label} 子任务尚未真正启动，当前不能发送退出命令。"
            )),
            None => Err(format!(
                "错误：当前会话没有可退出的 {agent_label} 活跃子进程。"
            )),
        }
    }

    pub(crate) fn record_exit(&mut self, agent: &str) {
        if let Some(PlannedSubprocessState::Active { phase, .. }) = self.by_agent.get_mut(agent) {
            *phase = RegisteredSubprocessPhase::ExitRequested;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    use super::*;

    #[test]
    fn protocol_state_allows_parallel_dispatch_for_different_agents() {
        let mut state = ProtocolBatchState::new(Vec::new());
        state.record_dispatch("claude", "dispatch-claude");
        assert!(state.ensure_dispatch_allowed("codex", "Codex").is_ok());
    }

    #[test]
    fn protocol_state_blocks_duplicate_dispatch_in_same_batch() {
        let mut state = ProtocolBatchState::new(Vec::new());
        state.record_dispatch("claude", "dispatch-claude");
        let error = state
            .ensure_dispatch_allowed("claude", "Claude")
            .expect_err("duplicate dispatch should be rejected");
        assert!(error.contains("待启动子任务"));
    }

    #[test]
    fn protocol_state_updates_existing_phase_on_exit() {
        let mut state = ProtocolBatchState::new(vec![RegisteredSubprocess {
            workspace_id: "ws".to_string(),
            task_id: "task".to_string(),
            dispatch_id: "dispatch".to_string(),
            agent: "claude".to_string(),
            description: "desc".to_string(),
            phase: RegisteredSubprocessPhase::RoundCompleted,
            force_idle: Arc::new(AtomicBool::new(false)),
        }]);
        state.record_exit("claude");
        match state.by_agent.get("claude") {
            Some(PlannedSubprocessState::Active { phase, .. }) => {
                assert_eq!(*phase, RegisteredSubprocessPhase::ExitRequested);
            }
            _ => panic!("expected active claude subprocess"),
        }
    }
}
