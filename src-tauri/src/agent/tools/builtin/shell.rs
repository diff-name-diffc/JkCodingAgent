use async_trait::async_trait;
use serde_json::{json, Value};

use super::common::{string_arg, with_compression_parameters, DEFAULT_FORCE_COMPRESS_AFTER_CHARS};
use crate::agent::tools::context::ToolContext;
use crate::agent::tools::registry::AgentTool;
use crate::agent::tools::{ToolAction, ToolResult};

pub(super) fn message_tool() -> Box<dyn AgentTool> {
    Box::new(MessageTool)
}

struct MessageTool;

#[async_trait]
impl AgentTool for MessageTool {
    fn name(&self) -> &'static str {
        "message"
    }

    fn description(&self) -> &'static str {
        "向用户发送最终回复。通常在调查完成、结果整理完成或协调结束后使用。通常保持默认 compress=false 即可。"
    }

    fn parameters(&self) -> Value {
        with_compression_parameters(
            json!({
                "type": "object",
                "properties": {
                    "content": { "type": "string", "description": "要发送给用户的内容" }
                },
                "required": ["content"]
            }),
            false,
            DEFAULT_FORCE_COMPRESS_AFTER_CHARS,
            "消息工具一般只返回简短确认信息，默认关闭压缩。",
        )
    }

    async fn execute(&self, args: &Value, _context: &ToolContext) -> ToolResult {
        match string_arg(args, "content") {
            Some(content) => {
                ToolResult::success_text(format!("消息已发送（{} 字符）", content.len()))
                    .with_action(ToolAction::FinalMessage { content })
            }
            None => ToolResult::recoverable_error("错误：缺少必填参数 content"),
        }
    }
}
