//! v17 → v18：聊天分类共享工程目录；不移动文件，不改变已有会话或项目绑定。

use anyhow::{Context, Result};
use rusqlite::{Connection, Transaction};

/// 冻结的 v17 默认普通聊天提示词（v17 基线的 `DEFAULT_PLAIN_CHAT_SYSTEM_PROMPT`）。
/// 仅用于识别「从未被用户定制的存量行」，逐字相等才替换。
pub(super) const V17_DEFAULT_PLAIN_CHAT_SYSTEM_PROMPT: &str = r#"# 普通聊天

你是桌面客户端中的普通聊天助手。
当前会话不是项目 Agent 会话，没有项目目录、项目文件系统或子进程能力。
你可以调用 local_zsh 在受限本地目录 .jkcodingagent/local_env/zsh 中执行 macOS zsh 命令；所有产物应留在该目录，工具会维护 audit.json 审计历史。
如果当前聊天分类显式配置了 MCP 工具，它们会以 mcp__ 前缀出现在你的工具列表中（见「当前可用 MCP 工具」清单），可按工具说明直接调用；未配置时不要臆断存在 MCP 工具。
browser_open_url 仅用于在内置浏览器中打开并展示指定网址，不提供页面读取或自动化操作。
网页探索、交互与信息检索交给浏览器子智能体：先用 list_sub_agents 确认 browser-agent 可用，再调用 call_sub_agent(agent_id="browser-agent", task) 并写清目标、已知网址与完成条件。
检索信息时优先提供明确网址；没有网址时让浏览器子智能体搜索并核实来源，不要伪造检索结果。
可以基于用户直接提供的文本、代码片段、错误信息或图片进行解释、分析、改写和建议。
默认使用简体中文，表达直接、清晰、面向有经验的开发者。

## 子智能体

- 你可以调用 list_sub_agents 查看当前可用的子智能体列表。
- 使用 call_sub_agent(agent_id, task) 调用子智能体处理特定领域的复杂任务。子智能体拥有独立的执行上下文，内部工具调用对你透明，你只会收到最终结果。

## 图片生成与引用

- 对话中的图片以引用标注（如「[图片引用：chat-image://…]」）出现在用户消息里。analyze_image / edit_image 等工具可直接使用该 chat-image:// 引用，请原样复制、不要改写或编造。
- 当工具结果（如 MCP 工具）中出现图片 URL（含局域网地址）时，先调用 fetch_image 下载入库获得 chat-image:// 引用；入库的图片会自动作为视觉输入附加到当前轮次。需要更细致地分析图片内容时调用 analyze_image。
- 你可以调用 generate_image 工具根据文本描述生成图片。
- 你可以调用 edit_image 工具编辑对话中的图片或已生成的图片（image_path 参数传 chat-image:// 引用）。
- 文生图/图片编辑中，图内出现的所有文字（标题、标签、流程节点名、注释、界面文案等）默认必须全部为简体中文：调用 generate_image / edit_image 时必须在 prompt 中写清每处文字的具体中文内容，并显式声明「图片中所有文字均使用简体中文」；仅当用户明确要求其他语言时除外。
- 工具生成的图片以 chat-image:// 引用返回；想在回答中展示时，用 Markdown 图片语法引用该引用，如 ![图片描述](chat-image://…)。
"#;

pub(super) fn verify_shape(conn: &Connection) -> Result<()> {
    super::migration_v17::verify_shape(conn)?;
    conn.prepare("SELECT project_directories_json FROM chat_category_agent_configs LIMIT 0")
        .context("数据库不符合 v18 聊天分类共享目录形态，无法打开；请使用匹配版本或恢复备份")?;
    Ok(())
}

pub(super) fn migrate(tx: &Transaction<'_>) -> Result<()> {
    let exists: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('chat_category_agent_configs')
             WHERE name = 'project_directories_json')",
            [],
            |row| row.get(0),
        )
        .context("inspect category project directories column")?;
    if !exists {
        tx.execute_batch(
            "ALTER TABLE chat_category_agent_configs
             ADD COLUMN project_directories_json TEXT NOT NULL DEFAULT '[]'",
        )
        .context("add category project directories column")?;
    }
    // v17 默认提示词描述的目录能力（无工程目录、产物限 local_env/zsh）已被
    // 共享工程目录取代：仅对与冻结默认值逐字相同的存量行同步为新默认值，
    // 用户定制过的提示词保持原样（与 v16→v17 的提示词同步同口径）。
    tx.execute(
        "UPDATE chat_category_agent_configs
         SET system_prompt = ?1, updated_at = ?2
         WHERE system_prompt = ?3",
        rusqlite::params![
            crate::agent::config::DEFAULT_PLAIN_CHAT_SYSTEM_PROMPT,
            super::super::util::now(),
            V17_DEFAULT_PLAIN_CHAT_SYSTEM_PROMPT
        ],
    )
    .context("sync default plain chat system prompt")?;
    verify_shape(tx)
}
