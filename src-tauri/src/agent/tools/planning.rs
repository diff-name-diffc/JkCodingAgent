use async_trait::async_trait;
use serde_json::{json, Value};

use super::context::ToolContext;
use super::registry::AgentTool;

pub(super) fn planning_tools() -> Vec<Box<dyn AgentTool>> {
    vec![
        Box::new(UpdatePlanTool),
        Box::new(AskPlanQuestionTool),
        Box::new(CreatePlanDocumentTool),
        Box::new(ReadPlanDocumentTool),
        Box::new(ReplacePlanDocumentTool),
        Box::new(EditPlanDocumentTool),
        Box::new(PresentPlanTool),
        Box::new(MarkPlanImplementedTool),
    ]
}

struct UpdatePlanTool;
struct AskPlanQuestionTool;
struct CreatePlanDocumentTool;
struct ReadPlanDocumentTool;
struct ReplacePlanDocumentTool;
struct EditPlanDocumentTool;
struct PresentPlanTool;
struct MarkPlanImplementedTool;

#[derive(Debug, Clone)]
pub struct UpdatePlanDraft {
    pub explanation: Option<String>,
    pub items: Vec<UpdatePlanItemDraft>,
}

#[derive(Debug, Clone)]
pub struct UpdatePlanItemDraft {
    pub id: Option<String>,
    pub step: String,
    pub status: String,
    pub agent: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PlanQuestionDraft {
    pub question: String,
    pub options: Vec<PlanQuestionOptionDraft>,
}

#[derive(Debug, Clone)]
pub struct PlanQuestionOptionDraft {
    pub id: Option<String>,
    pub label: String,
    pub description: String,
}

#[async_trait]
impl AgentTool for UpdatePlanTool {
    fn name(&self) -> &'static str {
        "update_plan"
    }

    fn description(&self) -> &'static str {
        "维护 Default 模式下展示给用户的 Checklist 式任务计划。仅用于复杂任务的进度清单；Plan 模式禁用。一旦决定使用，必须在探索、委派或执行前先创建本次任务规划步骤。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "explanation": {
                    "type": "string",
                    "description": "可选，说明本次计划更新的原因"
                },
                "plan": {
                    "type": "array",
                    "description": "Checklist 步骤列表。同一时间最多一个步骤为 in_progress。",
                    "items": {
                        "type": "object",
                        "properties": {
                            "step": { "type": "string", "description": "步骤描述" },
                            "id": {
                                "type": "string",
                                "description": "可选稳定步骤 ID；用于在多次 update_plan 中保留子任务执行状态"
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed"],
                                "description": "步骤状态"
                            },
                            "agent": {
                                "type": "string",
                                "enum": ["claude", "codex"],
                                "description": "可选，执行该步骤时倾向使用的子 Agent"
                            }
                        },
                        "required": ["step", "status"]
                    }
                }
            },
            "required": ["plan"]
        })
    }

    async fn execute(&self, _args: &Value, _context: &ToolContext) -> String {
        "update_plan 已由运行时处理。".to_string()
    }
}

#[async_trait]
impl AgentTool for AskPlanQuestionTool {
    fn name(&self) -> &'static str {
        "ask_plan_question"
    }

    fn description(&self) -> &'static str {
        "Plan 模式专用：当探索后信息不足以生成详细计划时，向用户提出一个关键问题。必须提供 3 个候选选项；UI 会自动追加自定义输入选项。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "question": { "type": "string", "description": "需要用户回答的问题" },
                "options": {
                    "type": "array",
                    "description": "恰好 3 个候选选项。不要提供“其他/自定义”，UI 会自动添加。",
                    "minItems": 3,
                    "maxItems": 3,
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string", "description": "可选稳定 ID" },
                            "label": { "type": "string", "description": "简短选项标题" },
                            "description": { "type": "string", "description": "选择该项的影响或取舍" }
                        },
                        "required": ["label", "description"]
                    }
                }
            },
            "required": ["question", "options"]
        })
    }

    async fn execute(&self, _args: &Value, _context: &ToolContext) -> String {
        "ask_plan_question 已由运行时处理。".to_string()
    }
}

#[async_trait]
impl AgentTool for CreatePlanDocumentTool {
    fn name(&self) -> &'static str {
        "create_plan_document"
    }

    fn description(&self) -> &'static str {
        "Plan 模式专用：在当前项目 .jkcodingagent/plan/ 下创建计划书 Markdown 文件，并让 UI 自动打开。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": { "type": "string", "description": "计划标题，用于生成文件名" },
                "content": { "type": "string", "description": "完整 Markdown 计划内容" }
            },
            "required": ["title", "content"]
        })
    }

    async fn execute(&self, _args: &Value, _context: &ToolContext) -> String {
        "create_plan_document 已由运行时处理。".to_string()
    }
}

#[async_trait]
impl AgentTool for ReadPlanDocumentTool {
    fn name(&self) -> &'static str {
        "read_plan_document"
    }

    fn description(&self) -> &'static str {
        "Plan 模式专用：读取当前项目 .jkcodingagent/plan/ 下的计划书内容。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "计划书路径，可为绝对路径或相对项目根目录路径" }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, _args: &Value, _context: &ToolContext) -> String {
        "read_plan_document 已由运行时处理。".to_string()
    }
}

#[async_trait]
impl AgentTool for ReplacePlanDocumentTool {
    fn name(&self) -> &'static str {
        "replace_plan_document"
    }

    fn description(&self) -> &'static str {
        "Plan 模式专用：整体替换未实现计划书内容。只能写入当前项目 .jkcodingagent/plan/ 下且文件名不含 -已实现.md 的计划。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "计划书路径" },
                "content": { "type": "string", "description": "新的完整 Markdown 内容" }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, _args: &Value, _context: &ToolContext) -> String {
        "replace_plan_document 已由运行时处理。".to_string()
    }
}

#[async_trait]
impl AgentTool for EditPlanDocumentTool {
    fn name(&self) -> &'static str {
        "edit_plan_document"
    }

    fn description(&self) -> &'static str {
        "Plan 模式专用：用 old_text/new_text 精确编辑未实现计划书。命中多处时必须补充上下文或设置 replace_all=true。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "计划书路径" },
                "old_text": { "type": "string", "description": "要替换的原文本" },
                "new_text": { "type": "string", "description": "替换后的文本" },
                "replace_all": {
                    "type": "string",
                    "enum": ["true", "false"],
                    "default": "false",
                    "description": "是否替换全部命中项"
                }
            },
            "required": ["path", "old_text", "new_text"]
        })
    }

    async fn execute(&self, _args: &Value, _context: &ToolContext) -> String {
        "edit_plan_document 已由运行时处理。".to_string()
    }
}

#[async_trait]
impl AgentTool for PresentPlanTool {
    fn name(&self) -> &'static str {
        "present_plan"
    }

    fn description(&self) -> &'static str {
        "Plan 模式专用：声明计划书已规划完成，触发 UI 的实施/清上下文实施/继续修改交互。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "计划书路径" },
                "title": { "type": "string", "description": "计划标题" },
                "summary": { "type": "string", "description": "面向用户的计划摘要" }
            },
            "required": ["path", "title", "summary"]
        })
    }

    async fn execute(&self, _args: &Value, _context: &ToolContext) -> String {
        "present_plan 已由运行时处理。".to_string()
    }
}

#[async_trait]
impl AgentTool for MarkPlanImplementedTool {
    fn name(&self) -> &'static str {
        "mark_plan_implemented"
    }

    fn description(&self) -> &'static str {
        "Default 模式专用：计划实施完成后，把计划书重命名为 *-已实现.md，并提示后续模型该计划已经落地。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "已实施完成的计划书路径" },
                "summary": { "type": "string", "description": "实施结果摘要" }
            },
            "required": ["path", "summary"]
        })
    }

    async fn execute(&self, _args: &Value, _context: &ToolContext) -> String {
        "mark_plan_implemented 已由运行时处理。".to_string()
    }
}

pub fn parse_update_plan(args: &Value) -> Result<UpdatePlanDraft, String> {
    let explanation = args
        .get("explanation")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let items = args
        .get("plan")
        .and_then(Value::as_array)
        .ok_or_else(|| "错误：缺少必填参数 plan，且 plan 必须是数组".to_string())?
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let step = item
                .get("step")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| format!("错误：plan[{index}].step 不能为空"))?;
            let status = item
                .get("status")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| format!("错误：plan[{index}].status 不能为空"))?;
            Ok(UpdatePlanItemDraft {
                id: item
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                step: step.to_string(),
                status: status.to_string(),
                agent: item
                    .get("agent")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok(UpdatePlanDraft { explanation, items })
}

pub fn parse_ask_plan_question(args: &Value) -> Result<PlanQuestionDraft, String> {
    let question = string_field(args, "question")?;
    let options = args
        .get("options")
        .and_then(Value::as_array)
        .ok_or_else(|| "错误：缺少必填参数 options，且 options 必须是数组".to_string())?;
    if options.len() != 3 {
        return Err(format!(
            "错误：options 必须恰好包含 3 个选项，实际收到 {} 个",
            options.len()
        ));
    }

    let options = options
        .iter()
        .enumerate()
        .map(|(index, option)| {
            let label = option
                .get("label")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| format!("错误：options[{index}].label 不能为空"))?;
            let description = option
                .get("description")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| format!("错误：options[{index}].description 不能为空"))?;
            let id = option
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            Ok(PlanQuestionOptionDraft {
                id,
                label: label.to_string(),
                description: description.to_string(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok(PlanQuestionDraft { question, options })
}

pub fn parse_create_plan_document(args: &Value) -> Result<(String, String), String> {
    Ok((string_field(args, "title")?, string_field(args, "content")?))
}

pub fn parse_replace_plan_document(args: &Value) -> Result<(String, String), String> {
    Ok((string_field(args, "path")?, string_field(args, "content")?))
}

pub fn parse_edit_plan_document(args: &Value) -> Result<(String, String, String, bool), String> {
    let replace_all = args
        .get("replace_all")
        .and_then(Value::as_str)
        .map(|value| value.eq_ignore_ascii_case("true"))
        .or_else(|| args.get("replace_all").and_then(Value::as_bool))
        .unwrap_or(false);
    Ok((
        string_field(args, "path")?,
        string_field(args, "old_text")?,
        string_field(args, "new_text")?,
        replace_all,
    ))
}

pub fn parse_present_plan(args: &Value) -> Result<(String, String, String), String> {
    Ok((
        string_field(args, "path")?,
        string_field(args, "title")?,
        string_field(args, "summary")?,
    ))
}

fn string_field(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("错误：缺少必填参数 {key}，且不能为空"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{parse_ask_plan_question, parse_update_plan, planning_tools};

    #[test]
    fn update_plan_rejects_empty_step() {
        let parsed = parse_update_plan(&json!({
            "plan": [{ "step": "", "status": "pending" }]
        }));
        assert!(parsed.expect_err("empty step should fail").contains("step"));
    }

    #[test]
    fn update_plan_description_requires_first_plan_when_used() {
        let tools = planning_tools();
        let update_plan = tools
            .iter()
            .find(|tool| tool.name() == "update_plan")
            .expect("update_plan tool should exist");

        let description = update_plan.description();

        assert!(description.contains("仅用于复杂任务"));
        assert!(description.contains("必须在探索、委派或执行前先创建本次任务规划步骤"));
    }

    #[test]
    fn ask_plan_question_requires_three_options() {
        let parsed = parse_ask_plan_question(&json!({
            "question": "怎么做？",
            "options": [
                { "label": "A", "description": "a" },
                { "label": "B", "description": "b" }
            ]
        }));
        assert!(parsed.expect_err("two options should fail").contains("3"));
    }
}
