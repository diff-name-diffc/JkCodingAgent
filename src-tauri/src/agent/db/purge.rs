//! 会话级联资源清理的共享助手。
//!
//! clear_messages / delete_session / delete_project 三处曾对同一组
//! workspace 键控表手工双写 DELETE 清单（v2 复核确认三处逐条一致），
//! 收敛到本模块单一出处——新增 workspace 键控表时只改这里。
//!
//! 注意与 `truncate_messages_from`（cleanup.rs）的边界：截断是
//! 「从某条消息起重发」语义，token 用量 / 关键字 / 图片文件等有
//! 「有意保留」的差异化策略，**不走**本助手。

use std::path::PathBuf;

use anyhow::{Context, Result};
use rusqlite::{params, Transaction};

use super::content::delete_chat_image_resources;

/// 在事务内删除会话的全部从属资源（不删会话行本身）：
/// tool 产物/运行、子智能体 trace、图编排产物、token 用量、图片记录、
/// 关键字与消息。`python_code_runs` / `chat_images` 行由消息外键级联，
/// 这里显式删 chat_images 是为了拿回图片目录路径供提交后回收。
///
/// 返回待回收的图片目录（事务提交后由调用方 best-effort 删除；
/// DB 已提交时文件清理失败不应把删除误报为失败）。
pub(crate) fn purge_session_resources_tx(
    tx: &Transaction<'_>,
    workspace_id: &str,
) -> Result<Option<PathBuf>> {
    tx.execute(
        "DELETE FROM dispatcher_tool_artifacts WHERE workspace_id = ?1",
        params![workspace_id],
    )
    .context("purge dispatcher tool artifacts")?;
    tx.execute(
        "DELETE FROM dispatcher_tool_runs WHERE workspace_id = ?1",
        params![workspace_id],
    )
    .context("purge dispatcher tool runs")?;
    tx.execute(
        "DELETE FROM sub_agent_run_traces WHERE workspace_id = ?1",
        params![workspace_id],
    )
    .context("purge sub-agent run traces")?;
    // 图编排产物（graph_plans / graph_node_runs）随会话清理同步删除。
    tx.execute(
        "DELETE FROM graph_node_runs
         WHERE plan_id IN (SELECT id FROM graph_plans WHERE workspace_id = ?1)",
        params![workspace_id],
    )
    .context("purge graph node runs")?;
    tx.execute(
        "DELETE FROM graph_plans WHERE workspace_id = ?1",
        params![workspace_id],
    )
    .context("purge graph plans")?;
    tx.execute(
        "DELETE FROM dispatcher_session_token_usage WHERE workspace_id = ?1",
        params![workspace_id],
    )
    .context("purge dispatcher session token usage")?;
    let image_dir = delete_chat_image_resources(tx, workspace_id)?;
    tx.execute(
        "DELETE FROM session_keywords WHERE session_id = ?1",
        params![workspace_id],
    )
    .context("purge session keywords")?;
    tx.execute(
        "DELETE FROM dispatcher_messages WHERE workspace_id = ?1",
        params![workspace_id],
    )
    .context("purge dispatcher messages")?;
    Ok(image_dir)
}
