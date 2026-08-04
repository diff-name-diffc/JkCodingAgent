//! 编排器系统提示。
//!
//! 结构：静态部分每轮构建一次（角色提示 + USER.md + 记忆 + 技能），
//! 动态部分每次迭代重建（可用工具、系统时间）；PI Harness 目录每轮发现一次。
//! 与 run_loop「每轮重建系统消息」的骨架对齐。

use std::path::Path;

use anyhow::Result;

use crate::agent::llm::ToolDefinition;
use crate::agent::prompt::PromptBundle;

use super::OrchestratorAgent;

const ORCHESTRATOR_ROLE_PROMPT: &str = r#"# 项目编排 Agent

你是桌面客户端中的项目编排 Agent。你本身不写代码、不执行命令；你的核心职责是：
完整理解用户需求 → 用只读工具探索项目（证据优先）→ 把复杂任务拆解为一张「执行图」（DAG），交给专业的执行 Agent 完成。

## 工作方式判定

- 简单问题（问答、解释、小范围咨询、无需动代码）：直接用 message 工具答复用户，不要出图。
- 复杂任务（多步骤、多角色协作、跨模块改动）：先探索、再出图。出图前不要向用户输出长篇计划说明，直接用 submit_graph 提交即可，系统会把图呈现给用户确认。

## 可用工具

- 只读探索：`read_file` / `list_dir` / `glob` / `grep`。
- `message`：向用户发送最终答复（简单问题的收口方式）。
- `submit_graph`：提交执行图（复杂任务的收口方式）。每轮最多提交一次；提交后等待用户确认，不要重复提交。

## 执行图 schema

```json
{
  "version": 2,
  "title": "图标题",
  "summary": "一句话编排思路",
  "stateKeys": [{ "key": "snake_case 键名", "description": "用途说明" }],
  "nodes": [{
    "id": "n1",
    "title": "节点标题",
    "role": "该节点 Agent 的角色定位",
    "modelRef": "<当前模型目录中的稳定 id>",
    "baseToolGroup": "read_only 或 coding",
    "specialTools": [{ "source": "pi_extension 或 aha", "name": "工具名" }],
    "task": "自包含的子任务说明",
    "dependsOn": ["上游节点 id"],
    "injectStateKeys": ["需要注入的 state key"],
    "outputKey": "本节点输出写回 state 的 key"
  }]
}
```

- 每个节点只使用一个主模型。模型、基础工具组和特殊工具必须来自本轮 PI Harness 目录。
- 边由 `dependsOn` 派生，必须构成无环图；`dependsOn` 引用的节点必须存在；`id`、`outputKey` 全局唯一；节点数 ≤ 20。
- 节点完成后 `state[outputKey] = 节点输出`（截断 32k）；下游节点通过 `dependsOn` 收到全部上游输出全文，通过 `injectStateKeys` 收到指定 state 的当前值。
- 节点输入由系统装配：总体需求 + 角色 + 子任务 + 上游输出 + 注入的 state 节选。节点拿不到聊天记录，因此 `task` 必须自包含（目标、背景、相关文件/符号、约束、验证方式、期望产出）。

## 节点设计原则

- 单一职责：一个节点只做一件事，调研 / 改造 / 验证分开。
- 上下文最小化 + 显式数据流：先由调研节点产出结论（outputKey），改造节点 inject 该结论后再动手；最后通常加验证节点。
- 根据任务性质、模型分类和能力标签选择主模型；只读任务优先 `read_only`，确需修改或命令时使用 `coding`。
- Harness Engineering：基础工具保持最小，只有任务确实需要时才选择 PI 扩展或 Aha/MCP 特殊工具。
- 无依赖关系的节点会并行执行（同层最多 3 个）；可并行的子任务请拆成平行节点。
- 禁止引用 PI Harness 目录之外的模型或工具；不要生成 subAgent、Claude CLI 或 Codex CLI 节点。

## 探索纪律

- `glob` 缩小范围 → `grep` 精确匹配 → `read_file` 加载确认；证据不足时继续收缩，不臆测。
- `list_dir` 只返回指定 path 之下最多两层，文件条目后的 `(:N行)` 是文件总行数；先用它了解局部结构，再用 `read_file path:start-end` 加载所需行段。
- 互相独立的只读探索尽量在同一轮发起多个调用；调查工具支持 `paths` / `patterns` 数组参数减少轮次。
- 调查工具支持 `compress` / `compress_intent` 参数：`compress=false` 时绝不进行摘要，超过 2000 字符的结果会带截断行信息返回前 2000 字符；只有 `compress=true` 且结果超过 5000 字符时才进行摘要。分析代码、配置等需要精确内容时保持 `compress=false`；需从超长输出中提取关键内容时显式设置 `compress=true` 并写明 `compress_intent`。`read_file` 的 `paths` 可使用 `path:start-end` 协议精确读取包含边界的行范围。

## 输出语言

默认简体中文，面向有经验的开发者，结论直接清晰。
"#;

impl OrchestratorAgent {
    /// 静态提示词：角色 + 用户偏好（USER.md）+ 记忆 + 技能。
    /// 每轮构建一次（文件读取走 spawn_blocking）。
    pub(super) async fn build_static_prompt(&self) -> Result<String> {
        let root = self.config.root_dir.clone();
        let extra = tokio::task::spawn_blocking(move || load_prompt_files(&root))
            .await
            .map_err(|error| anyhow::anyhow!("读取编排器提示词文件失败：{error}"))??;

        let mut prompt = ORCHESTRATOR_ROLE_PROMPT.to_string();
        if !extra.is_empty() {
            prompt.push_str("\n\n---\n\n");
            prompt.push_str(&extra);
        }
        Ok(prompt)
    }

    /// 每次迭代重建的完整系统提示：静态内容 + 动态分片。
    pub(super) fn build_iteration_system_prompt(
        &self,
        static_bundle: &PromptBundle,
        _workspace_id: &str,
        tool_definitions: &[ToolDefinition],
    ) -> String {
        let mut rendered = static_bundle.static_content.clone();

        let tools_block = render_available_tools_block(tool_definitions);
        if !tools_block.is_empty() {
            rendered.push_str("\n\n---\n\n");
            rendered.push_str(&tools_block);
        }

        rendered.push_str("\n\n---\n\n");
        rendered.push_str(&format!(
            "# 系统时间\n\n当前本地时间：{}",
            crate::agent::prompt::current_local_time()
        ));

        rendered
    }

    pub(super) fn render_graph_harness_catalog(
        &self,
        catalog: &crate::agent::graph::types::GraphHarnessCatalog,
    ) -> String {
        let mut lines = vec![
            "# 当前 PI Harness 目录".to_string(),
            "该目录是 graph v2 的唯一模型与特殊工具来源；ID 必须原样引用。".to_string(),
            "\n## 主模型（每节点恰好一个）".to_string(),
        ];
        for model in &catalog.models {
            lines.push(format!(
                "- `{}`：{} / {} / category={} / capabilities={}",
                model.id,
                model.label,
                model.model,
                model.category,
                model.capabilities.join(",")
            ));
        }
        lines.push("\n## 基础工具组".to_string());
        lines.push("- `read_only`: read, grep, find, ls".to_string());
        lines.push("- `coding`: read, grep, find, ls, bash, edit, write".to_string());
        lines.push("\n## 特殊工具".to_string());
        for tool in &catalog.tools {
            lines.push(format!(
                "- `{}:{}` [{} / {}]：{}",
                tool.source,
                tool.name,
                tool.category,
                if tool.review_required {
                    "需审查"
                } else {
                    "直接执行"
                },
                tool.description
            ));
        }
        if !catalog.diagnostics.is_empty() {
            lines.push(format!(
                "\n## 发现诊断\n- {}",
                catalog.diagnostics.join("\n- ")
            ));
        }
        lines.join("\n")
    }
}

fn render_available_tools_block(tool_definitions: &[ToolDefinition]) -> String {
    if tool_definitions.is_empty() {
        return String::new();
    }

    let mut lines = vec![
        "# 当前实际可用工具".to_string(),
        "以下列表来自本轮运行时实际注入的工具定义，是当前可调用工具的唯一准确信息源。".to_string(),
    ];

    let mut tools = tool_definitions
        .iter()
        .map(|tool| {
            (
                tool.function.name.clone(),
                tool.function.description.trim().to_string(),
            )
        })
        .collect::<Vec<_>>();
    tools.sort_by(|left, right| left.0.cmp(&right.0));

    for (name, description) in tools {
        lines.push(format!("- `{name}`：{description}"));
    }

    lines.join("\n")
}

/// 读取用户级提示词文件（USER.md / 记忆 / 技能）。
/// 刻意不读 SOUL.md：其内容是旧调度 Agent 的角色设定，与编排器角色冲突。
fn load_prompt_files(root: &Path) -> Result<String> {
    let mut sections: Vec<String> = Vec::new();

    let user = root.join("USER.md");
    if user.exists() {
        sections.push(
            std::fs::read_to_string(&user)
                .map_err(|error| anyhow::anyhow!("读取 {} 失败：{error}", user.display()))?,
        );
    }

    let skills_dir = root.join("skills");
    if skills_dir.exists() {
        let mut skill_parts = Vec::new();
        for entry in std::fs::read_dir(&skills_dir)
            .map_err(|error| anyhow::anyhow!("读取 {} 失败：{error}", skills_dir.display()))?
        {
            let entry = entry?;
            let skill_md = entry.path().join("SKILL.md");
            if skill_md.exists() {
                skill_parts.push(format!(
                    "### 技能：{}\n\n{}",
                    entry.file_name().to_string_lossy(),
                    std::fs::read_to_string(&skill_md).map_err(|error| {
                        anyhow::anyhow!("读取 {} 失败：{error}", skill_md.display())
                    })?
                ));
            }
        }
        skill_parts.sort();
        if !skill_parts.is_empty() {
            sections.push(format!("# 已启用技能\n\n{}", skill_parts.join("\n\n")));
        }
    }

    let memory = root.join("memory").join("MEMORY.md");
    if memory.exists() {
        sections.push(format!(
            "# 记忆\n\n{}",
            std::fs::read_to_string(&memory)
                .map_err(|error| anyhow::anyhow!("读取 {} 失败：{error}", memory.display()))?
        ));
    }

    Ok(sections
        .into_iter()
        .map(|section| section.trim().to_string())
        .filter(|section| !section.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n---\n\n"))
}
