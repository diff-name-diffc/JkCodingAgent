use async_trait::async_trait;
use serde_json::Value;

use super::super::{AgentTool, ToolContext, ToolResult};
use crate::agent::tools::program::tool_program_parameters_schema;

pub(super) fn run_tool_program_tool() -> Box<dyn AgentTool> {
    Box::new(RunToolProgramTool)
}

struct RunToolProgramTool;

#[async_trait]
impl AgentTool for RunToolProgramTool {
    fn name(&self) -> &'static str {
        "run_tool_program"
    }

    fn description(&self) -> &'static str {
        "在受限运行时中组合多个已授权工具调用。程序只支持 call、sequence、parallel、return；不执行 Python/JavaScript/Shell，不允许动态工具名。arguments 与 return.value 可用严格引用 {\"$ref\":{\"step\":\"步骤ID\",\"pointer\":\"/data/files\"}} 读取之前步骤的 JSON 结果。根节点必须是 sequence，且最后一步是全程序唯一 return。"
    }

    fn parameters(&self) -> Value {
        tool_program_parameters_schema()
    }

    async fn execute(&self, _args: &Value, _context: &ToolContext) -> ToolResult {
        ToolResult::fatal_error(
            "错误：run_tool_program 是编排器协议工具，必须由 ToolProgram 宿主拦截执行。",
        )
    }
}
