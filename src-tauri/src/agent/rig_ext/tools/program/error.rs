//! PTC 程序的运行失败。工具参数或单次调用错误留在程序内部的 `ToolCallError`，
//! 不使用这个类型。

use thiserror::Error;

/// 整次程序无法继续时的种类。文本里的名字与 DeepSeek Harness 的 `code run failed` 对齐。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum FailureKind {
    Exception,
    Timeout,
    Abort,
    OutputLimit,
}

impl FailureKind {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Exception => "exception",
            Self::Timeout => "timeout",
            Self::Abort => "abort",
            Self::OutputLimit => "output-limit",
        }
    }
}

#[derive(Debug, Clone, Error, Eq, PartialEq)]
#[error("{message}")]
pub(super) struct CodeRunFailed {
    pub kind: FailureKind,
    pub message: String,
    pub logs: String,
}

impl CodeRunFailed {
    pub(super) fn new(
        kind: FailureKind,
        message: impl Into<String>,
        logs: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            message: message.into(),
            logs: logs.into(),
        }
    }

    /// 模型可见文本：种类、消息、已经打印的内容。
    pub(super) fn model_text(&self) -> String {
        let mut text = format!("code run failed ({}): {}", self.kind.as_str(), self.message);
        if !self.logs.is_empty() {
            text.push('\n');
            text.push_str(&self.logs);
        }
        text
    }
}
