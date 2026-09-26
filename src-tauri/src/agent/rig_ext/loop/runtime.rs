//! 仅供宿主与 webview 对账的运行快照；模型工具面不暴露查询入口。
use crate::agent::{common::emit, db::DispatcherToolRunRecord, rig_ext::events::AgentEvent};
use parking_lot::Mutex;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    sync::{Arc, OnceLock, Weak},
};
use tauri::ipc::Channel;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScopeSnapshot {
    pub agent_run_id: String,
    pub scope_id: String,
    pub workspace_id: String,
    pub phase: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuntimeSnapshot {
    pub scopes: Vec<ScopeSnapshot>,
    pub tasks: Vec<DispatcherToolRunRecord>,
}

pub(super) struct ScopeRuntime {
    state: Mutex<ScopeSnapshot>,
    events: Channel<AgentEvent>,
}

fn registry() -> &'static Mutex<BTreeMap<String, Weak<ScopeRuntime>>> {
    static SCOPES: OnceLock<Mutex<BTreeMap<String, Weak<ScopeRuntime>>>> = OnceLock::new();
    SCOPES.get_or_init(Default::default)
}

impl ScopeRuntime {
    pub fn new(run: &str, scope: &str, workspace: &str, events: Channel<AgentEvent>) -> Arc<Self> {
        let runtime = Arc::new(Self {
            state: Mutex::new(ScopeSnapshot {
                agent_run_id: run.into(),
                scope_id: scope.into(),
                workspace_id: workspace.into(),
                phase: "deciding".into(),
            }),
            events,
        });
        let mut scopes = registry().lock();
        scopes.retain(|_, value| value.strong_count() > 0);
        scopes.insert(scope.into(), Arc::downgrade(&runtime));
        runtime
    }
    pub fn phase(&self, phase: &str) {
        let snapshot = {
            let mut state = self.state.lock();
            if state.phase == phase {
                return;
            }
            state.phase = phase.into();
            state.clone()
        };
        emit(
            &self.events,
            AgentEvent::RunPhaseChanged {
                agent_run_id: snapshot.agent_run_id,
                scope_id: snapshot.scope_id,
                workspace_id: snapshot.workspace_id,
                phase: snapshot.phase,
            },
        );
    }
}

pub(crate) fn scopes(workspace: &str) -> Vec<ScopeSnapshot> {
    let runtimes = registry()
        .lock()
        .values()
        .filter_map(Weak::upgrade)
        .collect::<Vec<_>>();
    runtimes
        .iter()
        .map(|runtime| runtime.state.lock().clone())
        .filter(|scope| scope.workspace_id == workspace)
        .collect()
}
