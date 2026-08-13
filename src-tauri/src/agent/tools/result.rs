use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agent::db::ToolArtifactDraft;
use crate::agent::sub_agent::tool::SUB_AGENT_FAILURE_PREFIX;

/// 工具错误消息统一前缀（项目规范）：返回给 LLM 的错误结果一律以此开头。
/// `from_text` 依赖该前缀把错误结果与成功结果区分开，
/// 因此 `recoverable_error` / `fatal_error` 构造器会强制校验并补齐前缀。
pub const TOOL_ERROR_PREFIX: &str = "错误：";

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

    /// 错误类别词表，与 `as_run_status` 的取值保持一致（Success 无错误类别，
    /// 返回 None——调用方以 `error_kind().is_some()` 判定是否写入错误列）。
    pub fn error_kind(self) -> Option<&'static str> {
        match self {
            Self::RecoverableError => Some("recoverable_error"),
            Self::FatalError => Some("fatal_error"),
            Self::Cancelled => Some("cancelled"),
            Self::Success => None,
        }
    }
}

/// 强制错误消息满足前缀约定：已带「错误：」前缀或子智能体失败哨兵前缀的
/// 原样保留（哨兵前缀是父循环识别致命错误的协议标记，绝不能被覆盖），
/// 其余一律补齐「错误：」前缀，避免错误被 `from_text` 误判为成功。
fn ensure_error_prefix(message: impl Into<String>) -> String {
    let message = message.into();
    let trimmed = message.trim_start();
    if trimmed.starts_with(TOOL_ERROR_PREFIX) || trimmed.starts_with(SUB_AGENT_FAILURE_PREFIX) {
        return trimmed.to_string();
    }
    format!("{TOOL_ERROR_PREFIX}{trimmed}")
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ToolAction {
    FinalMessage { content: String },
}

impl ToolAction {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::FinalMessage { .. } => "final_message",
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
    /// 从工具原始输出文本构造结果：哨兵前缀判定为致命错误，
    /// 「错误：」前缀判定为可恢复错误，其余视为成功。
    /// 构造器（`recoverable_error` / `fatal_error`）会强制补齐前缀，
    /// 因此走构造器产出的错误结果在这里一定能被正确分类。
    pub fn from_text(output: impl Into<String>) -> Self {
        let output = output.into();
        let trimmed = output.trim();
        if trimmed.starts_with(SUB_AGENT_FAILURE_PREFIX) {
            Self::fatal_error(output)
        } else if trimmed.starts_with(TOOL_ERROR_PREFIX) {
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
        let message = ensure_error_prefix(message);
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
        let message = ensure_error_prefix(message);
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
        // toolResultMetadata 统一为对象结构：非空对象原样写入；
        // 数组/原始值包装为 { "value": ... }，避免下游按对象解析时类型出错；
        // 空对象/null 不写入（与「无业务 metadata」语义一致）。
        match &self.metadata {
            Value::Object(object) if !object.is_empty() => {
                metadata.insert("toolResultMetadata".to_string(), self.metadata.clone());
            }
            Value::Object(_) | Value::Null => {}
            other => {
                metadata.insert("toolResultMetadata".to_string(), json!({ "value": other }));
            }
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
    pub name: String,
    /// 补齐 schema 默认值后的参数：执行、落库与摘要压缩统一使用这套参数。
    pub effective_arguments: Value,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{ToolResult, ToolStatus, TOOL_ERROR_PREFIX};
    use crate::agent::sub_agent::tool::{sub_agent_failure, sub_agent_failure_message};

    #[test]
    fn from_text_classifies_by_prefix() {
        assert_eq!(
            ToolResult::from_text("错误：读取失败").status,
            ToolStatus::RecoverableError
        );
        assert_eq!(
            ToolResult::from_text(sub_agent_failure("子智能体崩溃")).status,
            ToolStatus::FatalError
        );
        assert_eq!(
            ToolResult::from_text("正常结果").status,
            ToolStatus::Success
        );
        // 前缀判定前先 trim，容忍首尾空白。
        assert_eq!(
            ToolResult::from_text("  错误：读取失败\n").status,
            ToolStatus::RecoverableError
        );
    }

    #[test]
    fn error_constructors_enforce_error_prefix() {
        let missing = ToolResult::recoverable_error("读取文件失败：权限不足");
        assert!(missing.display.starts_with(TOOL_ERROR_PREFIX));

        let present = ToolResult::recoverable_error("错误：读取文件失败");
        assert_eq!(present.display, "错误：读取文件失败");

        let fatal = ToolResult::fatal_error("内部崩溃");
        assert!(fatal.display.starts_with(TOOL_ERROR_PREFIX));
    }

    #[test]
    fn fatal_error_preserves_sub_agent_failure_sentinel() {
        // 哨兵前缀是父循环识别致命错误的协议标记，构造器不得覆盖它，
        // 否则 sub_agent_failure_message 的 strip_prefix 会失效。
        let wrapped = sub_agent_failure("子智能体执行失败");
        let result = ToolResult::from_text(wrapped.clone());

        assert_eq!(result.status, ToolStatus::FatalError);
        assert_eq!(
            sub_agent_failure_message(&result.output_for_llm()),
            Some("子智能体执行失败")
        );
    }

    #[test]
    fn run_metadata_json_normalizes_tool_result_metadata() {
        // 非空对象：原样写入。
        let mut result = ToolResult::success_text("abc");
        result.metadata = json!({ "k": "v" });
        let parsed: serde_json::Value =
            serde_json::from_str(&result.run_metadata_json().unwrap()).unwrap();
        assert_eq!(parsed["toolResultMetadata"]["k"], "v");
        assert_eq!(parsed["displayChars"], 3);
        assert_eq!(parsed["artifactCount"], 0);

        // 空对象：不写入该键。
        let mut result = ToolResult::success_text("abc");
        result.metadata = json!({});
        let parsed: serde_json::Value =
            serde_json::from_str(&result.run_metadata_json().unwrap()).unwrap();
        assert!(parsed.get("toolResultMetadata").is_none());

        // 数组/原始值：包装为对象，保证下游按对象解析不出错。
        let mut result = ToolResult::success_text("abc");
        result.metadata = json!([1, 2]);
        let parsed: serde_json::Value =
            serde_json::from_str(&result.run_metadata_json().unwrap()).unwrap();
        assert_eq!(parsed["toolResultMetadata"]["value"], json!([1, 2]));

        let mut result = ToolResult::success_text("abc");
        result.metadata = json!("plain");
        let parsed: serde_json::Value =
            serde_json::from_str(&result.run_metadata_json().unwrap()).unwrap();
        assert_eq!(parsed["toolResultMetadata"]["value"], "plain");
    }

    #[test]
    fn error_kind_shares_vocabulary_with_run_status() {
        for status in [
            ToolStatus::RecoverableError,
            ToolStatus::FatalError,
            ToolStatus::Cancelled,
        ] {
            assert_eq!(status.error_kind(), Some(status.as_run_status()));
        }
        // Success 无错误类别：调用方以 is_some 判定是否写错误列。
        assert_eq!(ToolStatus::Success.error_kind(), None);
    }
}
