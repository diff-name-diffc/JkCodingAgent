//! 执行引擎的单一分派点；两种引擎共享父调用身份与终态轨迹持久化。

use std::sync::Arc;

use super::browser_agent::BrowserAgentRuntime;
use super::runner::{RigSubAgentRequest, RigSubAgentRuntime};
use crate::agent::sub_agent::{config::BROWSER_AGENT_ID, manager::SubAgentManager};

pub(super) enum SubAgentRuntime {
    Browser(BrowserAgentRuntime),
    Rig(RigSubAgentRuntime),
}

impl SubAgentRuntime {
    pub fn build(request: &RigSubAgentRequest<'_>) -> Result<Self, String> {
        if request.config.agent_id == BROWSER_AGENT_ID {
            Ok(Self::Browser(BrowserAgentRuntime::build(request)))
        } else {
            RigSubAgentRuntime::build(request).map(Self::Rig)
        }
    }

    pub fn is_browser(&self) -> bool {
        matches!(self, Self::Browser(_))
    }

    pub async fn execute(
        &self,
        task: &str,
        manager: Arc<SubAgentManager>,
    ) -> Result<String, String> {
        match self {
            Self::Browser(runtime) => runtime.execute(task, manager).await,
            Self::Rig(runtime) => runtime.execute(task).await,
        }
    }

    pub fn model(&self) -> &str {
        match self {
            Self::Browser(runtime) => runtime.model(),
            Self::Rig(runtime) => runtime.model(),
        }
    }

    pub fn status(&self, success: bool) -> &str {
        match self {
            Self::Browser(runtime) => runtime.status(),
            Self::Rig(_) => {
                if success {
                    "completed"
                } else {
                    "failed"
                }
            }
        }
    }

    pub fn trace_events_json(&self) -> Result<String, String> {
        match self {
            Self::Browser(runtime) => runtime.trace_events_json(),
            Self::Rig(runtime) => runtime.trace_events_json(),
        }
    }
}
