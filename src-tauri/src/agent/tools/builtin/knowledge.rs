use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::task;

use crate::agent::tools::context::ToolContext;
use crate::agent::tools::registry::AgentTool;
use crate::knowledge;

pub(super) fn search_knowledge_base_tool() -> Box<dyn AgentTool> {
    Box::new(SearchKnowledgeBaseTool)
}

pub(super) fn read_knowledge_page_tool() -> Box<dyn AgentTool> {
    Box::new(ReadKnowledgePageTool)
}

struct SearchKnowledgeBaseTool;
struct ReadKnowledgePageTool;

#[async_trait]
impl AgentTool for SearchKnowledgeBaseTool {
    fn name(&self) -> &'static str {
        "search_knowledge_base"
    }

    fn description(&self) -> &'static str {
        "检索应用内知识库集合。适合在回答或编码前查询用户沉淀的 Wiki 知识。需要已配置知识库 embedding 模型。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "检索问题或关键词" },
                "collection_ids": {
                    "type": "array",
                    "description": "可选，限定检索的集合 ID 列表；不传则检索全部集合。",
                    "items": { "type": "string" }
                },
                "limit": { "type": "integer", "description": "最多返回多少条，默认 8", "minimum": 1, "maximum": 20 }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: &Value, _context: &ToolContext) -> String {
        let Some(query) = args.get("query").and_then(Value::as_str).map(str::trim) else {
            return "错误：缺少必填参数 query".to_string();
        };
        if query.is_empty() {
            return "错误：query 不能为空".to_string();
        }
        let collection_ids = args
            .get("collection_ids")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            });
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .map(|value| value as usize)
            .unwrap_or(8)
            .clamp(1, 20);
        knowledge::search_for_agent(query.to_string(), collection_ids, limit).await
    }
}

#[async_trait]
impl AgentTool for ReadKnowledgePageTool {
    fn name(&self) -> &'static str {
        "read_knowledge_page"
    }

    fn description(&self) -> &'static str {
        "读取知识库页面内容。通常先调用 search_knowledge_base，再用其返回的 collection_id 和 path 读取完整页面。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "collection_id": { "type": "string", "description": "知识库集合 ID" },
                "relative_path": { "type": "string", "description": "页面相对路径，例如 wiki/concepts/foo.md" },
                "max_chars": { "type": "integer", "description": "最多返回字符数，默认 6000", "minimum": 500, "maximum": 20000 }
            },
            "required": ["collection_id", "relative_path"]
        })
    }

    async fn execute(&self, args: &Value, _context: &ToolContext) -> String {
        let Some(collection_id) = args
            .get("collection_id")
            .and_then(Value::as_str)
            .map(str::trim)
        else {
            return "错误：缺少必填参数 collection_id".to_string();
        };
        let Some(relative_path) = args
            .get("relative_path")
            .and_then(Value::as_str)
            .map(str::trim)
        else {
            return "错误：缺少必填参数 relative_path".to_string();
        };
        let max_chars = args
            .get("max_chars")
            .and_then(Value::as_u64)
            .map(|value| value as usize)
            .unwrap_or(6000)
            .clamp(500, 20_000);
        let collection_id = collection_id.to_string();
        let relative_path = relative_path.to_string();
        task::spawn_blocking(move || {
            knowledge::read_page_for_agent(collection_id, relative_path, max_chars)
        })
        .await
        .unwrap_or_else(|error| format!("读取知识库页面任务失败：{error}"))
    }
}
