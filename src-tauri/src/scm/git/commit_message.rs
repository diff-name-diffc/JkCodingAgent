//! 基于项目对话模型的 AI 提交信息生成。
//!
//! 与普通聊天/项目编排器共用 `resolve_project_chat_provider` 解析出的模型；
//! 读取项目配置中的 `[git].commit_prompt` 作为提示词主体。

use std::time::Duration;

use anyhow::Context;

use super::exec::run_git;
use super::{GitError, GitResult};
use crate::agent::agents::project::resolve_project_chat_provider;
use crate::agent::llm::ChatMessage;
use crate::agent::DispatcherState;
use crate::project::read_project_config;
use crate::shared::error::{CommandResult, IntoCommandResult};
use crate::shared::truncate_for_display;

#[tauri::command]
pub async fn generate_commit_message(
    project_path: String,
    state: tauri::State<'_, DispatcherState>,
) -> CommandResult<String> {
    let agent_config = state.agent_config();
    let settings = match state.db().get_settings_v2() {
        Ok(settings) => settings,
        Err(error) => return Err(format!("读取模型设置失败：{error:#}")),
    };
    let provider = resolve_project_chat_provider(&agent_config, &settings);
    generate_commit_message_impl(project_path.clone(), provider)
        .await
        .with_context(|| format!("生成提交信息失败（{}）", project_path))
        .into_command_result()
}

async fn generate_commit_message_impl(
    project_path: String,
    provider: crate::agent::llm::OpenAiCompatProvider,
) -> GitResult<String> {
    // 1. Get staged diff
    let diff_output = run_git(&project_path, &["diff", "--staged"]).await?;
    let diff = String::from_utf8_lossy(&diff_output.stdout).into_owned();
    if diff.trim().is_empty() {
        return Err(GitError::NoStagedChanges);
    }

    // Truncate diff if too large to avoid CLI arg limits
    let diff = truncate_for_display(&diff, 50_000, "...（diff 已截断）");

    // 2. Read project config for prompt.
    let config = read_project_config(project_path.clone()).map_err(GitError::ProjectConfig)?;
    let commit_prompt = config.git.commit_prompt;

    // 3. Build full prompt
    let full_prompt = format!(
        "{}\n\n以下是 Git diff：\n```diff\n{}\n```\n\n只输出提交信息正文，不要附加解释。",
        commit_prompt, diff
    );

    if !provider.is_configured() {
        return Err(GitError::AgentFailed(
            "项目对话模型未配置 API Key".to_string(),
        ));
    }

    let messages = vec![ChatMessage {
        role: "user".to_string(),
        content: full_prompt,
        content_parts: Vec::new(),
        reasoning_content: None,
        tool_calls: None,
        tool_call_id: None,
        name: None,
    }];
    let response = tokio::time::timeout(
        Duration::from_secs(15),
        provider.chat_stream(&messages, &[], false, |_| {}),
    )
    .await
    .map_err(|_| GitError::CommitMessageTimeout)?
    .map_err(|error| GitError::AgentFailed(format!("{error:#}")))?;

    let result = response.content.trim().to_string();
    if result.is_empty() {
        return Err(GitError::EmptyAgentResult);
    }
    Ok(result)
}
