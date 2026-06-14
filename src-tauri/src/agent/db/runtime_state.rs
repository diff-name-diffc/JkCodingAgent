//! 运行时状态：会话模式（mode）、checklist 计划、计划交互（plan interaction），
//! 以及它们的读写与 `spawn_blocking` 异步包装。

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::sessions::DispatcherMode;
use super::util::{now, parse_optional_json};
use super::DispatcherDb;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChecklistPlanState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    pub items: Vec<ChecklistPlanItem>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChecklistPlanItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub step: String,
    pub status: ChecklistStepStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dispatch_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subprocess_task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ChecklistStepStatus {
    Pending,
    InProgress,
    Completed,
}

impl ChecklistStepStatus {
    pub fn from_wire(value: &str) -> Result<Self> {
        match value.trim() {
            "pending" => Ok(Self::Pending),
            "in_progress" => Ok(Self::InProgress),
            "completed" => Ok(Self::Completed),
            other => anyhow::bail!("invalid checklist step status: {other}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanQuestionOption {
    pub id: String,
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PlanInteraction {
    Question {
        id: String,
        question: String,
        options: Vec<PlanQuestionOption>,
    },
    Ready {
        plan_path: String,
        title: String,
        summary: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherSessionRuntimeState {
    pub mode: DispatcherMode,
    pub checklist: Option<ChecklistPlanState>,
    pub plan_interaction: Option<PlanInteraction>,
    pub active_plan_path: Option<String>,
}

impl DispatcherDb {
    pub fn get_session_runtime_state(
        &self,
        session_id: &str,
    ) -> Result<DispatcherSessionRuntimeState> {
        let conn = self.conn()?;
        let (mode, active_plan_path, checklist_json, plan_interaction_json) = conn
            .query_row(
                "SELECT mode, active_plan_path, checklist_json, plan_interaction_json
                 FROM dispatcher_sessions
                 WHERE id = ?1",
                params![session_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()
            .context("load dispatcher session runtime state")?
            .with_context(|| format!("dispatcher session not found: {session_id}"))?;

        Ok(DispatcherSessionRuntimeState {
            mode: DispatcherMode::from_sql_value(mode),
            active_plan_path,
            checklist: parse_optional_json(checklist_json, "checklist_json")?,
            plan_interaction: parse_optional_json(plan_interaction_json, "plan_interaction_json")?,
        })
    }

    pub fn set_session_mode(
        &self,
        session_id: &str,
        mode: DispatcherMode,
    ) -> Result<DispatcherSessionRuntimeState> {
        let conn = self.conn()?;
        let changed = conn
            .execute(
                "UPDATE dispatcher_sessions
                 SET mode = ?1, updated_at = ?2
                 WHERE id = ?3",
                params![mode.as_sql_value(), now(), session_id],
            )
            .context("set dispatcher session mode")?;
        if changed == 0 {
            anyhow::bail!("dispatcher session not found: {session_id}");
        }
        self.get_session_runtime_state(session_id)
    }

    pub fn update_checklist(
        &self,
        session_id: &str,
        checklist: &ChecklistPlanState,
    ) -> Result<DispatcherSessionRuntimeState> {
        let checklist_json = serde_json::to_string(checklist).context("serialize checklist")?;
        let conn = self.conn()?;
        let changed = conn
            .execute(
                "UPDATE dispatcher_sessions
                 SET checklist_json = ?1, updated_at = ?2
                 WHERE id = ?3",
                params![checklist_json, now(), session_id],
            )
            .context("update dispatcher checklist")?;
        if changed == 0 {
            anyhow::bail!("dispatcher session not found: {session_id}");
        }
        self.get_session_runtime_state(session_id)
    }

    pub fn clear_checklist(&self, session_id: &str) -> Result<DispatcherSessionRuntimeState> {
        let conn = self.conn()?;
        let changed = conn
            .execute(
                "UPDATE dispatcher_sessions
                 SET checklist_json = NULL, updated_at = ?1
                 WHERE id = ?2",
                params![now(), session_id],
            )
            .context("clear dispatcher checklist")?;
        if changed == 0 {
            anyhow::bail!("dispatcher session not found: {session_id}");
        }
        self.get_session_runtime_state(session_id)
    }

    pub fn attach_checklist_subprocess(
        &self,
        session_id: &str,
        dispatch_id: &str,
        task_id: &str,
    ) -> Result<DispatcherSessionRuntimeState> {
        let mut state = self.get_session_runtime_state(session_id)?;
        let Some(mut checklist) = state.checklist.take() else {
            return Ok(state);
        };

        if let Some(item) = checklist
            .items
            .iter_mut()
            .find(|item| item.dispatch_id.as_deref() == Some(dispatch_id))
        {
            item.status = ChecklistStepStatus::InProgress;
            item.subprocess_task_id = Some(task_id.to_string());
            checklist.updated_at = now();
            return self.update_checklist(session_id, &checklist);
        }

        Ok(DispatcherSessionRuntimeState {
            checklist: Some(checklist),
            ..state
        })
    }

    pub fn clear_checklist_dispatch(
        &self,
        session_id: &str,
        dispatch_id: &str,
    ) -> Result<DispatcherSessionRuntimeState> {
        let mut state = self.get_session_runtime_state(session_id)?;
        let Some(mut checklist) = state.checklist.take() else {
            return Ok(state);
        };

        let mut changed = false;
        for item in &mut checklist.items {
            if item.dispatch_id.as_deref() == Some(dispatch_id) {
                item.dispatch_id = None;
                item.subprocess_task_id = None;
                item.agent = None;
                item.detail = None;
                if item.status == ChecklistStepStatus::InProgress {
                    item.status = ChecklistStepStatus::Pending;
                }
                changed = true;
            }
        }

        if changed {
            checklist.updated_at = now();
            return self.update_checklist(session_id, &checklist);
        }

        Ok(DispatcherSessionRuntimeState {
            checklist: Some(checklist),
            ..state
        })
    }

    pub fn set_plan_interaction(
        &self,
        session_id: &str,
        interaction: Option<&PlanInteraction>,
    ) -> Result<DispatcherSessionRuntimeState> {
        let interaction_json = interaction
            .map(serde_json::to_string)
            .transpose()
            .context("serialize plan interaction")?;
        let conn = self.conn()?;
        let changed = conn
            .execute(
                "UPDATE dispatcher_sessions
                 SET plan_interaction_json = ?1, updated_at = ?2
                 WHERE id = ?3",
                params![interaction_json, now(), session_id],
            )
            .context("update dispatcher plan interaction")?;
        if changed == 0 {
            anyhow::bail!("dispatcher session not found: {session_id}");
        }
        self.get_session_runtime_state(session_id)
    }

    pub fn set_active_plan_path(
        &self,
        session_id: &str,
        plan_path: Option<&str>,
    ) -> Result<DispatcherSessionRuntimeState> {
        let conn = self.conn()?;
        let changed = conn
            .execute(
                "UPDATE dispatcher_sessions
                 SET active_plan_path = ?1, updated_at = ?2
                 WHERE id = ?3",
                params![plan_path, now(), session_id],
            )
            .context("update dispatcher active plan path")?;
        if changed == 0 {
            anyhow::bail!("dispatcher session not found: {session_id}");
        }
        self.get_session_runtime_state(session_id)
    }
    pub async fn clear_checklist_async(
        &self,
        workspace_id: &str,
    ) -> Result<DispatcherSessionRuntimeState> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        tokio::task::spawn_blocking(move || db.clear_checklist(&wid))
            .await
            .context("clear_checklist spawn_blocking")?
    }


    pub async fn get_session_runtime_state_async(
        &self,
        workspace_id: &str,
    ) -> Result<DispatcherSessionRuntimeState> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        tokio::task::spawn_blocking(move || db.get_session_runtime_state(&wid))
            .await
            .context("get_session_runtime_state spawn_blocking")?
    }

    pub async fn update_checklist_async(
        &self,
        session_id: &str,
        checklist: &ChecklistPlanState,
    ) -> Result<DispatcherSessionRuntimeState> {
        let db = self.clone();
        let sid = session_id.to_string();
        let checklist = checklist.clone();
        tokio::task::spawn_blocking(move || db.update_checklist(&sid, &checklist))
            .await
            .context("update_checklist spawn_blocking")?
    }

    pub async fn set_plan_interaction_async(
        &self,
        session_id: &str,
        interaction: Option<&PlanInteraction>,
    ) -> Result<DispatcherSessionRuntimeState> {
        let db = self.clone();
        let sid = session_id.to_string();
        let interaction = interaction.cloned();
        tokio::task::spawn_blocking(move || db.set_plan_interaction(&sid, interaction.as_ref()))
            .await
            .context("set_plan_interaction spawn_blocking")?
    }

    pub async fn set_active_plan_path_async(
        &self,
        session_id: &str,
        plan_path: Option<&str>,
    ) -> Result<DispatcherSessionRuntimeState> {
        let db = self.clone();
        let sid = session_id.to_string();
        let plan_path = plan_path.map(str::to_string);
        tokio::task::spawn_blocking(move || db.set_active_plan_path(&sid, plan_path.as_deref()))
            .await
            .context("set_active_plan_path spawn_blocking")?
    }
}
