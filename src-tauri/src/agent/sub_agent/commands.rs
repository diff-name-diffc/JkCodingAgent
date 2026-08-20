use std::collections::HashSet;
use std::sync::Arc;

use anyhow::{anyhow, Context};
use tauri::State;

use super::config::SubAgentConfig;
use super::db::{SubAgentRecord, SubAgentRunTraceRecord, ToolInfo};
use super::manager::SubAgentManager;
use crate::agent::tools::ToolRegistry;
use crate::agent::DispatcherState;
use crate::shared::error::{CommandResult, IntoCommandResult};

fn manager_from(state: &DispatcherState) -> anyhow::Result<Arc<SubAgentManager>> {
    state
        .sub_agent_manager()
        .ok_or_else(|| anyhow!("错误：子智能体管理器未初始化"))
}

/// 子智能体只从普通聊天 Agent 派生，运行时拿到的也是父级 plain-chat 注册表。
/// 保存校验必须使用同一 execution profile；不能取全局并集，否则会出现配置
/// 可保存但运行时静默缺工具。MCP 与嵌套子智能体工具均不在该 profile 中。
fn known_static_tool_names(state: &DispatcherState) -> HashSet<String> {
    let mcp = state.project_mcp_registry();
    let ssh = state.ssh_manager();
    ToolRegistry::plain_chat_tools(mcp, ssh)
        .tool_names_and_descriptions()
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

// 说明：以下命令均为 async Tauri 命令，manager 内部是同步 rusqlite I/O，
// 直接调用会阻塞 Tokio 执行器，因此统一用 spawn_blocking 包裹
// （manager 保持同步 API 不变，侵入最小；agents/* 运行期对 manager 的
// 调用点由各自模块负责同样处理）。

#[tauri::command]
pub async fn sub_agent_list(
    state: State<'_, DispatcherState>,
) -> CommandResult<Vec<SubAgentRecord>> {
    let result = match manager_from(&state) {
        Ok(manager) => {
            tokio::task::spawn_blocking(move || manager.list_all().context("查询子智能体列表失败"))
                .await
                .context("等待子智能体列表任务失败")
                .and_then(|inner| inner)
        }
        Err(error) => Err(error),
    };
    result.into_command_result()
}

#[tauri::command]
pub async fn sub_agent_create(
    state: State<'_, DispatcherState>,
    config_json: String,
) -> CommandResult<SubAgentRecord> {
    let result = match manager_from(&state) {
        Ok(manager) => {
            let prepared = serde_json::from_str::<SubAgentConfig>(&config_json)
                .context("解析子智能体配置失败")
                .and_then(|config| {
                    let known = known_static_tool_names(&state);
                    config.validate_allowed_tools(&known)?;
                    Ok(config)
                });
            match prepared {
                Ok(config) => tokio::task::spawn_blocking(move || {
                    manager.create(config).context("创建子智能体失败")
                })
                .await
                .context("等待子智能体创建任务失败")
                .and_then(|inner| inner),
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(error),
    };
    result.into_command_result()
}

#[tauri::command]
pub async fn sub_agent_update(
    state: State<'_, DispatcherState>,
    id: String,
    config_json: String,
) -> CommandResult<SubAgentRecord> {
    let result = match manager_from(&state) {
        Ok(manager) => {
            let prepared = serde_json::from_str::<SubAgentConfig>(&config_json)
                .context("解析子智能体配置失败")
                .and_then(|mut config| {
                    // 配置显式携带 agent_id 时必须与路径参数一致；静默改写等同
                    // 静默改名，会掩盖复制旧配置等错误，且落库内容与提交语义不符。
                    if !config.agent_id.is_empty() && config.agent_id != id {
                        anyhow::bail!(
                            "错误：配置中的 agent_id（{}）与路径参数 id（{}）不一致",
                            config.agent_id,
                            id
                        );
                    }
                    config.agent_id = id.clone();
                    let known = known_static_tool_names(&state);
                    config.validate_allowed_tools(&known)?;
                    Ok(config)
                });
            match prepared {
                Ok(config) => tokio::task::spawn_blocking(move || {
                    manager
                        .update(&id, config)
                        .with_context(|| format!("更新子智能体失败（id={id}）"))
                })
                .await
                .context("等待子智能体更新任务失败")
                .and_then(|inner| inner),
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(error),
    };
    result.into_command_result()
}

#[tauri::command]
pub async fn sub_agent_delete(state: State<'_, DispatcherState>, id: String) -> CommandResult<()> {
    let result = match manager_from(&state) {
        Ok(manager) => tokio::task::spawn_blocking(move || {
            manager
                .delete(&id)
                .with_context(|| format!("删除子智能体失败（id={id}）"))
        })
        .await
        .context("等待子智能体删除任务失败")
        .and_then(|inner| inner),
        Err(error) => Err(error),
    };
    result.into_command_result()
}

#[tauri::command]
pub async fn sub_agent_seed_browser(state: State<'_, DispatcherState>) -> CommandResult<()> {
    let result = match manager_from(&state) {
        Ok(manager) => tokio::task::spawn_blocking(move || {
            manager
                .seed_browser_force()
                .context("重建内置浏览器子智能体失败")
        })
        .await
        .context("等待重建内置浏览器子智能体任务失败")
        .and_then(|inner| inner),
        Err(error) => Err(error),
    };
    result.into_command_result()
}

#[tauri::command]
pub async fn sub_agent_list_tools(
    state: State<'_, DispatcherState>,
) -> CommandResult<Vec<ToolInfo>> {
    let result = state
        .registered_tool_names()
        .map(|names| {
            names
                .into_iter()
                .map(|(name, description)| ToolInfo { name, description })
                .collect()
        })
        .ok_or_else(|| anyhow!("错误：工具信息未初始化"));
    result.into_command_result()
}

#[tauri::command]
pub async fn sub_agent_set_global_enabled(
    state: State<'_, DispatcherState>,
    sub_agent_ids: Vec<String>,
) -> CommandResult<()> {
    let result = match manager_from(&state) {
        Ok(manager) => tokio::task::spawn_blocking(move || {
            // DB 层 INSERT OR IGNORE 会静默丢弃不存在的 id（表现为保存成功
            // 但未生效），因此在命令层先校验存在性，对未知 id 明确报错。
            let existing = manager.list_all().context("查询子智能体列表失败")?;
            let existing_ids: HashSet<&str> =
                existing.iter().map(|record| record.id.as_str()).collect();
            let unknown: Vec<&str> = sub_agent_ids
                .iter()
                .filter(|agent_id| !existing_ids.contains(agent_id.as_str()))
                .map(String::as_str)
                .collect();
            if !unknown.is_empty() {
                anyhow::bail!("错误：以下子智能体不存在：{}", unknown.join("、"));
            }
            manager
                .set_global_enabled(&sub_agent_ids)
                .context("保存全局启用子智能体失败")
        })
        .await
        .context("等待保存全局启用子智能体任务失败")
        .and_then(|inner| inner),
        Err(error) => Err(error),
    };
    result.into_command_result()
}

#[tauri::command]
pub async fn sub_agent_get_global_enabled(
    state: State<'_, DispatcherState>,
) -> CommandResult<Vec<SubAgentRecord>> {
    let result = match manager_from(&state) {
        Ok(manager) => tokio::task::spawn_blocking(move || {
            manager
                .get_global_enabled()
                .context("查询全局启用子智能体失败")
        })
        .await
        .context("等待查询全局启用子智能体任务失败")
        .and_then(|inner| inner),
        Err(error) => Err(error),
    };
    result.into_command_result()
}

#[tauri::command]
pub async fn sub_agent_get_run_trace(
    state: State<'_, DispatcherState>,
    workspace_id: String,
    tool_call_id: String,
) -> CommandResult<Option<SubAgentRunTraceRecord>> {
    let result = match manager_from(&state) {
        Ok(manager) => tokio::task::spawn_blocking(move || {
            manager
                .get_run_trace(&workspace_id, &tool_call_id)
                .with_context(|| {
                    format!(
                        "查询子智能体执行轨迹失败（workspace_id={workspace_id}, tool_call_id={tool_call_id}）"
                    )
                })
        })
        .await
        .context("等待子智能体轨迹查询任务失败")
        .and_then(|inner| inner),
        Err(error) => Err(error),
    };
    result.into_command_result()
}
