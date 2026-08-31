use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agent::db::artifacts::MAX_RAW_TOOL_ARTIFACT_BYTES;
use crate::agent::db::ToolArtifactDraft;

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

/// 强制错误消息满足前缀约定：已带「错误：」前缀的原样保留，其余一律补齐
/// 「错误：」前缀，避免错误被 `from_text` 误判为成功。
fn ensure_error_prefix(message: impl Into<String>) -> String {
    let message = message.into();
    let trimmed = message.trim_start();
    if trimmed.starts_with(TOOL_ERROR_PREFIX) {
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
    /// 供运行时组合、字段引用与条件判断使用的规范业务数据。
    ///
    /// `None` 明确表示该工具仍只提供文本结果；调用方不得尝试从 display 或
    /// context_payload 反向猜测结构。大体积原文仍由 raw_output/artifacts 承载。
    pub data: Option<Value>,
    pub display: String,
    pub context_payload: String,
    pub raw_output: Option<String>,
    pub artifacts: Vec<ToolArtifactDraft>,
    pub action: Option<ToolAction>,
    pub metadata: Value,
}

impl ToolResult {
    /// 从工具原始输出文本构造结果：「错误：」前缀判定为可恢复错误，其余视为
    /// 成功。致命错误一律走类型化的 `fatal_error` 构造器，不做文本推断。
    /// 构造器（`recoverable_error` / `fatal_error`）会强制补齐前缀，
    /// 因此走构造器产出的错误结果在这里一定能被正确分类。
    pub fn from_text(output: impl Into<String>) -> Self {
        let output = output.into();
        if output.trim().starts_with(TOOL_ERROR_PREFIX) {
            Self::recoverable_error(output)
        } else {
            Self::success_text(output)
        }
    }

    pub fn success_text(output: impl Into<String>) -> Self {
        let output = output.into();
        Self {
            status: ToolStatus::Success,
            data: None,
            display: output.clone(),
            context_payload: output.clone(),
            raw_output: Some(output),
            artifacts: Vec::new(),
            action: None,
            metadata: json!({}),
        }
    }

    /// 构造同时具备机器可读数据和稳定文本呈现的成功结果。
    ///
    /// data 不会自动复制进 metadata；避免结构化大结果在工具运行记录中重复落库。
    pub fn success_data(
        data: Value,
        display: impl Into<String>,
        context_payload: impl Into<String>,
    ) -> Self {
        let display = display.into();
        let context_payload = context_payload.into();
        Self {
            status: ToolStatus::Success,
            data: Some(data),
            raw_output: Some(context_payload.clone()),
            display,
            context_payload,
            artifacts: Vec::new(),
            action: None,
            metadata: json!({}),
        }
    }

    pub fn recoverable_error(message: impl Into<String>) -> Self {
        let message = ensure_error_prefix(message);
        Self {
            status: ToolStatus::RecoverableError,
            data: None,
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
            data: None,
            display: message.clone(),
            context_payload: message.clone(),
            raw_output: Some(message),
            artifacts: Vec::new(),
            action: None,
            metadata: json!({}),
        }
    }

    pub fn cancelled(message: impl Into<String>) -> Self {
        let message = ensure_error_prefix(message);
        Self {
            status: ToolStatus::Cancelled,
            data: None,
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

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    pub fn ensure_raw_artifact(&mut self, tool_name: &str) {
        if self
            .artifacts
            .iter()
            .any(|artifact| artifact.kind == "tool_raw_output")
        {
            return;
        }
        let Some(raw_output) = self.raw_output.as_deref() else {
            return;
        };
        let artifact = ToolArtifactDraft::raw_tool_output(tool_name, raw_output);
        let raw_artifact_metadata = json!({
            "originalBytes": raw_output.len(),
            "storedBytes": artifact.content.len(),
            "limitBytes": MAX_RAW_TOOL_ARTIFACT_BYTES,
            "truncated": raw_output.len() > MAX_RAW_TOOL_ARTIFACT_BYTES,
        });
        match &mut self.metadata {
            Value::Object(metadata) => {
                metadata.insert("rawArtifact".to_string(), raw_artifact_metadata);
            }
            Value::Null => {
                self.metadata = json!({ "rawArtifact": raw_artifact_metadata });
            }
            other => {
                let previous = std::mem::replace(other, Value::Null);
                self.metadata = json!({
                    "value": previous,
                    "rawArtifact": raw_artifact_metadata,
                });
            }
        }
        self.artifacts.push(artifact);
    }

    pub fn output_for_llm(&self) -> String {
        self.output_for_llm_ref().to_string()
    }

    pub fn output_for_llm_ref(&self) -> &str {
        if !self.context_payload.is_empty() {
            return &self.context_payload;
        }
        self.raw_output.as_deref().unwrap_or(&self.display)
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

    use super::{ToolResult, ToolStatus, MAX_RAW_TOOL_ARTIFACT_BYTES, TOOL_ERROR_PREFIX};

    #[test]
    fn from_text_classifies_by_prefix() {
        assert_eq!(
            ToolResult::from_text("错误：读取失败").status,
            ToolStatus::RecoverableError
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

        let cancelled = ToolResult::cancelled("用户已停止");
        assert_eq!(cancelled.status, ToolStatus::Cancelled);
        assert!(cancelled.display.starts_with(TOOL_ERROR_PREFIX));
    }

    #[test]
    fn structured_success_keeps_data_separate_from_text_channels() {
        let data = json!({ "files": ["src/main.rs"] });
        let result = ToolResult::success_data(data.clone(), "找到 1 个文件", "src/main.rs");

        assert_eq!(result.status, ToolStatus::Success);
        assert_eq!(result.data, Some(data));
        assert_eq!(result.display, "找到 1 个文件");
        assert_eq!(result.context_payload, "src/main.rs");
        assert_eq!(result.raw_output.as_deref(), Some("src/main.rs"));
        let metadata = result.run_metadata_json().unwrap();
        assert!(!metadata.contains("src/main.rs"));
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

    #[test]
    fn raw_artifact_is_utf8_safe_bounded_and_records_truncation_metadata() {
        // 让多字节字符跨过字节预算附近，验证截断点不会切坏 UTF-8。
        let raw_output = format!("{}界", "x".repeat(MAX_RAW_TOOL_ARTIFACT_BYTES));
        let original_bytes = raw_output.len();
        let original_chars = raw_output.chars().count();
        let mut result = ToolResult {
            status: ToolStatus::Success,
            data: None,
            display: String::new(),
            context_payload: String::new(),
            raw_output: Some(raw_output),
            artifacts: Vec::new(),
            action: None,
            metadata: json!({}),
        };

        result.ensure_raw_artifact("large_output");

        let artifact = &result.artifacts[0];
        assert!(artifact.content.len() <= MAX_RAW_TOOL_ARTIFACT_BYTES);
        assert!(artifact.content.contains("truncated=true"));
        assert!(artifact.preview.contains("originalBytes="));
        assert_eq!(artifact.char_count, original_chars);
        assert_eq!(result.metadata["rawArtifact"]["truncated"], true);
        assert_eq!(
            result.metadata["rawArtifact"]["originalBytes"],
            original_bytes
        );
        assert_eq!(
            result.metadata["rawArtifact"]["storedBytes"],
            artifact.content.len()
        );
        // content 是 String，额外检查预算末端仍落在合法字符边界。
        assert!(artifact.content.is_char_boundary(artifact.content.len()));
    }
}
