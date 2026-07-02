use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agent::db::ToolArtifactDraft;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Success,
    RecoverableError,
    FatalError,
    Cancelled,
}

impl ToolStatus {
    pub fn as_run_status(self) -> &'static str {
        match self {
            Self::Success => "succeeded",
            Self::RecoverableError => "recoverable_error",
            Self::FatalError => "fatal_error",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn error_kind(self) -> Option<&'static str> {
        match self {
            Self::RecoverableError => Some("recoverable"),
            Self::FatalError => Some("fatal"),
            Self::Cancelled => Some("cancelled"),
            Self::Success => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ToolAction {
    FinalMessage { content: String },
    DispatchSubAgent { agent: String },
    ContinueSubAgent { agent: String },
    ExitSubAgent { agent: String },
}

impl ToolAction {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::FinalMessage { .. } => "final_message",
            Self::DispatchSubAgent { .. } => "dispatch_sub_agent",
            Self::ContinueSubAgent { .. } => "continue_sub_agent",
            Self::ExitSubAgent { .. } => "exit_sub_agent",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub status: ToolStatus,
    pub display: String,
    pub context_payload: String,
    pub raw_output: Option<String>,
    pub artifacts: Vec<ToolArtifactDraft>,
    pub action: Option<ToolAction>,
    pub metadata: Value,
}

impl ToolResult {
    pub fn from_text(output: impl Into<String>) -> Self {
        let output = output.into();
        let trimmed = output.trim();
        if trimmed.starts_with(crate::agent::sub_agent::tool::SUB_AGENT_FAILURE_PREFIX) {
            Self::fatal_error(output)
        } else if trimmed.starts_with("错误：") {
            Self::recoverable_error(output)
        } else {
            Self::success_text(output)
        }
    }

    pub fn success_text(output: impl Into<String>) -> Self {
        let output = output.into();
        Self {
            status: ToolStatus::Success,
            display: output.clone(),
            context_payload: output.clone(),
            raw_output: Some(output),
            artifacts: Vec::new(),
            action: None,
            metadata: json!({}),
        }
    }

    pub fn recoverable_error(message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            status: ToolStatus::RecoverableError,
            display: message.clone(),
            context_payload: message.clone(),
            raw_output: Some(message),
            artifacts: Vec::new(),
            action: None,
            metadata: json!({}),
        }
    }

    pub fn fatal_error(message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            status: ToolStatus::FatalError,
            display: message.clone(),
            context_payload: message.clone(),
            raw_output: Some(message),
            artifacts: Vec::new(),
            action: None,
            metadata: json!({}),
        }
    }

    pub fn with_action(mut self, action: ToolAction) -> Self {
        self.action = Some(action);
        self
    }

    pub fn with_artifacts(mut self, artifacts: Vec<ToolArtifactDraft>) -> Self {
        self.artifacts = artifacts;
        self
    }

    pub fn output_for_llm(&self) -> String {
        if !self.context_payload.is_empty() {
            return self.context_payload.clone();
        }
        self.raw_output
            .as_deref()
            .unwrap_or(&self.display)
            .to_string()
    }

    pub fn run_metadata_json(&self) -> Option<String> {
        let mut metadata = serde_json::Map::new();
        if let Some(object) = self.metadata.as_object() {
            if !object.is_empty() {
                metadata.insert("toolResultMetadata".to_string(), self.metadata.clone());
            }
        } else if !self.metadata.is_null() {
            metadata.insert("toolResultMetadata".to_string(), self.metadata.clone());
        }
        metadata.insert(
            "displayChars".to_string(),
            json!(self.display.chars().count()),
        );
        metadata.insert("artifactCount".to_string(), json!(self.artifacts.len()));
        Some(Value::Object(metadata).to_string())
    }
}

#[derive(Debug, Clone)]
pub struct ToolInput {
    pub call_id: String,
    pub name: String,
    pub arguments: Value,
    pub effective_arguments: Value,
}
