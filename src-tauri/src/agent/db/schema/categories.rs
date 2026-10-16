use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};

use super::super::util::now;

pub(super) fn ensure_chat_categories_table_tx(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS chat_categories (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            icon TEXT NOT NULL DEFAULT '',
            color TEXT NOT NULL DEFAULT '',
            sort_order INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        ",
    )
    .context("create chat_categories table")?;

    let ts = now();
    let defaults: Vec<(&str, &str, &str, &str, i32)> = vec![
        ("general", "综合", "MessageSquare", "", 0),
        ("life", "生活", "Heart", "#F43F5E", 1),
        ("work", "工作", "Briefcase", "#3B82F6", 2),
        ("tech", "技术", "Code2", "#8B5CF6", 3),
        ("learning", "学习", "GraduationCap", "#22C55E", 4),
    ];
    let mut stmt = tx
        .prepare(
            "INSERT OR IGNORE INTO chat_categories (id, name, icon, color, sort_order, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .context("prepare seed categories statement")?;
    for (id, name, icon, color, sort_order) in defaults {
        stmt.execute(params![id, name, icon, color, sort_order, &ts, &ts])
            .with_context(|| format!("seed chat category {id}"))?;
    }

    Ok(())
}

pub(super) fn ensure_chat_category_agent_configs_table_tx(
    tx: &rusqlite::Transaction<'_>,
) -> Result<()> {
    tx.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS chat_category_agent_configs (
            category_id TEXT PRIMARY KEY,
            allowed_tools_json TEXT NOT NULL DEFAULT '[]',
            system_prompt TEXT NOT NULL DEFAULT '',
            sub_agent_ids_json TEXT NOT NULL DEFAULT '[]',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            FOREIGN KEY (category_id) REFERENCES chat_categories(id) ON DELETE CASCADE
        );
        ",
    )
    .context("create chat_category_agent_configs table")
}

pub(in crate::agent::db) fn backfill_chat_category_agent_configs_tx(
    tx: &rusqlite::Transaction<'_>,
) -> Result<()> {
    let ts = now();
    let mut stmt = tx
        .prepare("SELECT id FROM chat_categories")
        .context("prepare chat categories for config backfill")?;
    let category_ids = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("load chat categories for config backfill")?;
    drop(stmt);

    for category_id in category_ids {
        let (allowed_tools_json, system_prompt) =
            default_chat_category_agent_config_tx(tx, &category_id)?;
        tx.execute(
            "
            INSERT OR IGNORE INTO chat_category_agent_configs (
                category_id,
                allowed_tools_json,
                system_prompt,
                sub_agent_ids_json,
                created_at,
                updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?5)
            ",
            params![category_id, allowed_tools_json, system_prompt, "[]", ts],
        )
        .with_context(|| format!("backfill chat category agent config {category_id}"))?;
    }
    Ok(())
}

pub(in crate::agent::db) fn default_chat_category_agent_config_tx(
    tx: &rusqlite::Transaction<'_>,
    category_id: &str,
) -> Result<(String, String)> {
    // v12 起 dispatcher_settings 为整对象 JSON：默认聊天工具面从
    // chat.allowed_tools 读取（无设置行/解析失败回落空清单——与旧列形态
    // 「无行回落 '[]'」语义一致）。
    let raw: Option<String> = tx
        .query_row(
            "SELECT settings_json FROM dispatcher_settings WHERE id = 'default'",
            [],
            |row| row.get(0),
        )
        .optional()
        .context("load default chat agent config")?;
    let allowed_tools_json = raw
        .and_then(|json| serde_json::from_str::<super::super::settings::AhaSettingsV2>(&json).ok())
        .map(|settings| {
            serde_json::to_string(&settings.chat.allowed_tools).unwrap_or_else(|_| "[]".to_string())
        })
        .unwrap_or_else(|| "[]".to_string());
    if let Some(default) = scenario_chat_category_agent_config(category_id) {
        return Ok((
            serde_json::to_string(default.tools)
                .context("serialize scenario chat category tools")?,
            default.system_prompt.to_string(),
        ));
    }

    Ok((
        allowed_tools_json,
        crate::agent::config::DEFAULT_PLAIN_CHAT_SYSTEM_PROMPT.to_string(),
    ))
}

pub(super) struct ScenarioChatCategoryAgentConfig {
    pub(super) tools: &'static [&'static str],
    pub(super) system_prompt: &'static str,
}

pub(super) fn scenario_chat_category_agent_config(
    category_id: &str,
) -> Option<ScenarioChatCategoryAgentConfig> {
    match category_id {
        "general" => Some(ScenarioChatCategoryAgentConfig {
            tools: &["browser_open_url", "list_sub_agents", "call_sub_agent"],
            system_prompt: r#"# 综合聊天

你是一个高信息密度的通用助手，适合处理日常问答、快速判断、文本整理和轻量信息检索。

工作方式：
- 网页探索与检索使用 call_sub_agent(agent_id="browser-agent", task)，先用 list_sub_agents 确认可用；browser_open_url 仅展示网页。
- 先直接回答用户问题；缺少事实依据时再使用浏览器读取公开信息。
- 回答保持简洁、清晰、可执行，默认使用简体中文。
- 不主动执行本地命令；除非用户明确要求或问题需要本地验证。
- 如任务明显属于专业领域，可先列出可用子智能体，再选择合适的子智能体协助。
"#,
        }),
        "life" => Some(ScenarioChatCategoryAgentConfig {
            tools: &["browser_open_url", "list_sub_agents", "call_sub_agent"],
            system_prompt: r#"# 生活助理

你是面向生活场景的助理，适合规划、比较、行程、消费决策、健康常识和日常文本处理。

工作方式：
- 网页探索与检索使用 call_sub_agent(agent_id="browser-agent", task)，先用 list_sub_agents 确认可用；browser_open_url 仅展示网页。
- 对会影响时间、金钱或安全的建议，优先检索当前信息并说明依据。
- 给出选择时用清晰的取舍标准，而不是堆砌选项。
- 遇到医疗、法律、金融等高风险问题时，只做信息整理和风险提示，不替代专业意见。
- 输出务实、温和、简洁，默认使用简体中文。
"#,
        }),
        "work" => Some(ScenarioChatCategoryAgentConfig {
            tools: &[
                "browser_open_url",
                "local_zsh",
                "ssh_list_servers",
                "list_sub_agents",
                "call_sub_agent",
            ],
            system_prompt: r#"# 工作助理

你是面向工作流的执行型助理，适合处理资料整理、流程推进、网页操作、轻量自动化和远程环境巡检。

工作方式：
- 网页探索与检索使用 call_sub_agent(agent_id="browser-agent", task)，先用 list_sub_agents 确认可用；browser_open_url 仅展示网页。
- 先明确目标、约束和交付物，再选择工具。
- 本地命令仅在 .jkcodingagent/local_env/zsh 中执行，命令要短小、可审计，避免高风险操作。
- SSH 默认只做服务器列表和只读巡检；执行变更前必须说明影响并等待用户确认。
- 默认使用简体中文，输出结论优先。
"#,
        }),
        "tech" => Some(ScenarioChatCategoryAgentConfig {
            tools: &[
                "local_zsh",
                "browser_open_url",
                "ssh_list_servers",
                "ssh_exec",
                "ssh_memo_read",
                "ssh_memo_upsert",
                "ssh_memo_delete",
                "list_sub_agents",
                "call_sub_agent",
            ],
            system_prompt: r#"# 技术助手

你是面向工程问题的技术助手，适合排查错误、解释代码、验证命令、阅读文档和推进技术方案。

工作方式：
- 网页探索与检索使用 call_sub_agent(agent_id="browser-agent", task)，先用 list_sub_agents 确认可用；browser_open_url 仅展示网页。
- 事实优先，必要时用浏览器查看官方文档或公开资料；不要编造 API、参数或版本信息。
- 本地验证优先使用 local_zsh，并保持命令小步、可复现、可审计。
- 远程命令必须先确认目标服务器；涉及写入、删除、部署、重启等操作前必须说明影响并等待用户确认。
- 对不熟悉的服务器，先读运维备忘录（ssh_memo_read）复用部署/目录索引与注意事项；仅把已核实、后续会复用的环境索引或高频易踩坑克制写入（先读后写、合并去重），不记录报错全文、排障步骤、一次性过程和凭据。
- 对复杂任务先拆解，再给出可执行步骤；回答默认简体中文，保持工程化、直接、少废话。
"#,
        }),
        "learning" => Some(ScenarioChatCategoryAgentConfig {
            tools: &[
                "browser_open_url",
                "local_zsh",
                "list_sub_agents",
                "call_sub_agent",
            ],
            system_prompt: r#"# 学习教练

你是学习型助手，适合讲解概念、制定学习路径、做题辅导、资料检索和小实验验证。

工作方式：
- 网页探索与检索使用 call_sub_agent(agent_id="browser-agent", task)，先用 list_sub_agents 确认可用；browser_open_url 仅展示网页。
- 先判断用户当前水平，再用递进方式解释：直觉、例子、关键细节、练习。
- 复杂概念要拆成短段落，并给出可验证的小任务。
- 需要最新资料时优先用浏览器读取可信来源；需要演示时可用 local_zsh 做小实验。
- 不替用户跳过思考：给答案，也给判断依据和可迁移的方法。
"#,
        }),
        _ => None,
    }
}
