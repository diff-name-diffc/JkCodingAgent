//! 基于项目对话模型的 AI 提交信息生成（rig 形态）。
//!
//! 模型来自「项目」上下文的对话槽位（`resolve_purpose_specs`），经 rig
//! `CompletionModel::completion` 发一次性请求；提示词主体取自项目配置的
//! `[git].commit_prompt`。

use std::time::Duration;

use anyhow::Context;
use rig::completion::{CompletionModel, Message};

use super::exec::run_git;
use super::{GitError, GitResult};
use crate::agent::db::AgentContext;
use crate::agent::rig_ext::model::{
    build_completion_request, completions_model, resolve_purpose_specs,
};
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
    let specs = resolve_purpose_specs(&settings, AgentContext::Project, &agent_config);
    generate_commit_message_impl(project_path.clone(), specs.chat)
        .await
        .with_context(|| format!("生成提交信息失败（{}）", project_path))
        .into_command_result()
}

async fn generate_commit_message_impl(
    project_path: String,
    spec: crate::agent::rig_ext::model::PurposeModelSpec,
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

    if !spec.is_configured() {
        return Err(GitError::AgentFailed(
            "项目对话模型未配置 API Key".to_string(),
        ));
    }

    let model = completions_model(&spec)
        .map_err(|error| GitError::AgentFailed(format!("初始化提交信息模型失败：{error:#}")))?;
    // 提交信息是短结论任务：关闭思考链（与旧 `chat_stream(..., false, ...)` 同口径）。
    let request = build_completion_request(
        None,
        vec![Message::user(full_prompt)],
        Vec::new(),
        spec.max_tokens,
        spec.temperature,
        false,
    );
    let response = tokio::time::timeout(Duration::from_secs(15), model.completion(request))
        .await
        .map_err(|_| GitError::CommitMessageTimeout)?
        .map_err(|error| GitError::AgentFailed(format!("{error:#}")))?;

    let result = response
        .choice
        .iter()
        .filter_map(|content| match content {
            rig::message::AssistantContent::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
        .trim()
        .to_string();
    if result.is_empty() {
        return Err(GitError::EmptyAgentResult);
    }
    Ok(result)
}
