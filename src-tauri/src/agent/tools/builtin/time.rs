use async_trait::async_trait;
use chrono::{Local, Utc};
use serde_json::{json, Value};

use crate::agent::tools::context::ToolContext;
use crate::agent::tools::registry::AgentTool;

pub(super) fn current_time_tool() -> Box<dyn AgentTool> {
    Box::new(CurrentTimeTool)
}

struct CurrentTimeTool;

#[async_trait]
impl AgentTool for CurrentTimeTool {
    fn name(&self) -> &'static str {
        "get_current_time"
    }

    fn description(&self) -> &'static str {
        "获取当前实时时间。适合回答今天日期、当前时间、时间戳或需要按当前日期判断的请求。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn execute(&self, _args: &Value, _context: &ToolContext) -> String {
        let local = Local::now();
        let utc = Utc::now();
        format!(
            "本地时间：{}\nUTC 时间：{}\nUnix 时间戳：{}\n本地时区偏移：{}",
            local.to_rfc3339(),
            utc.to_rfc3339(),
            local.timestamp(),
            local.offset()
        )
    }
}
