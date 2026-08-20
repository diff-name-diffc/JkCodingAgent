use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ProgramErrorKind {
    Parse,
    Validation,
    LimitExceeded,
    InvalidReference,
    PolicyDenied,
    ChildRecoverable,
    ChildFatal,
    Cancelled,
    DeadlineExceeded,
    Internal,
}

#[derive(Debug, Clone, Serialize, Deserialize, Error, Eq, PartialEq)]
#[error("{message}")]
#[serde(rename_all = "camelCase")]
pub struct ProgramError {
    pub kind: ProgramErrorKind,
    pub message: Box<str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_path: Option<Box<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_id: Option<Box<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<Box<str>>,
    #[serde(default, skip_serializing_if = "completed_steps_is_empty")]
    pub completed_steps: Box<[String]>,
}

fn completed_steps_is_empty(steps: &[String]) -> bool {
    steps.is_empty()
}

impl ProgramError {
    pub fn new(kind: ProgramErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into().into_boxed_str(),
            node_path: None,
            step_id: None,
            tool: None,
            completed_steps: Box::default(),
        }
    }

    pub fn at_path(mut self, node_path: impl Into<String>) -> Self {
        self.node_path = Some(node_path.into().into_boxed_str());
        self
    }

    pub fn for_step(mut self, step_id: impl Into<String>, tool: impl Into<String>) -> Self {
        self.step_id = Some(step_id.into().into_boxed_str());
        self.tool = Some(tool.into().into_boxed_str());
        self
    }

    pub fn with_completed_steps(mut self, completed_steps: Vec<String>) -> Self {
        self.completed_steps = completed_steps.into_boxed_slice();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::{ProgramError, ProgramErrorKind};

    #[test]
    fn error_context_is_structured_and_serializable() {
        let error = ProgramError::new(ProgramErrorKind::PolicyDenied, "工具不允许")
            .at_path("/root/steps/1")
            .for_step("write", "write_file")
            .with_completed_steps(vec!["read".to_string()]);

        let value = serde_json::to_value(&error).expect("serialize error");
        assert_eq!(value["kind"], "policy_denied");
        assert_eq!(value["nodePath"], "/root/steps/1");
        assert_eq!(value["stepId"], "write");
        assert_eq!(value["tool"], "write_file");
        assert_eq!(value["completedSteps"][0], "read");
        assert_eq!(error.to_string(), "工具不允许");
    }
}
