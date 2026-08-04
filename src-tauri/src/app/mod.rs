use crate::agent::DispatcherState;
use crate::{
    agent, browser, chat_images, platform, project, python_runner, rag, scm, shared::TaskManager,
    ssh_tool, task_runtime, workspace,
};
use tauri::Manager;

fn build_task_manager() -> TaskManager {
    TaskManager::default()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let project_mcp_registry = project::mcp::ProjectMcpRegistry::default();
    let ssh_session_manager = ssh_tool::SshSessionManager::default();
    let dispatcher_state =
        DispatcherState::new(project_mcp_registry.clone(), ssh_session_manager.clone())
            .expect("failed to initialize Dispatcher state");

    tauri::Builder::default()
        .setup(|app| {
            // 后台预热 login shell 环境，避免第一次启动任务时阻塞
            std::thread::spawn(|| {
                platform::get_login_shell_path();
            });

            // 自动启动 RAG sidecar（异步，不阻塞 setup；失败仅记日志，
            // 用户可在「RAG 知识库」配置面板查看状态并手动重启）
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let manager = app_handle.state::<rag::RagManager>();
                let store = app_handle.state::<rag::RagConfigStore>();
                if let Err(error) = manager.ensure_started(&app_handle, store.inner()).await {
                    let logs = app_handle.state::<rag::RagLogStore>();
                    logs.append_system(&app_handle, format!("RAG sidecar 自动启动失败：{error:#}"));
                    eprintln!("[rag] 自动启动失败：{error:#}");
                }
            });

            Ok(())
        })
        .manage(build_task_manager())
        .manage(dispatcher_state)
        .manage(browser::BrowserManager::default())
        .manage(agent::voice::VoiceAsrManager::default())
        .manage(python_runner::PythonRunnerState::default())
        .manage(project_mcp_registry)
        .manage(ssh_session_manager)
        .manage(workspace::RopeManager::new())
        .manage(rag::RagManager::default())
        .manage(rag::RagConfigStore::default())
        .manage(rag::RagLogStore::default())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_window_state::Builder::new().build())
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            task_runtime::pty::stop_task,
            task_runtime::pty::send_input,
            task_runtime::pty::resize_pty,
            task_runtime::pty::get_pty_output_snapshot,
            task_runtime::pty::open_shell,
            task_runtime::pty::kill_shell,
            browser::browser_start,
            browser::browser_start_plain_chat,
            browser::browser_import_chrome_profile,
            browser::browser_list_chrome_profile_candidates,
            browser::browser_stop,
            python_runner::python_runner_list_results,
            python_runner::python_runner_start,
            python_runner::python_runner_stop,
            python_runner::python_runner_clear_result,
            rag::commands::rag_start,
            rag::commands::rag_stop,
            rag::commands::rag_restart,
            rag::commands::rag_status,
            rag::commands::rag_get_kb_config,
            rag::commands::rag_save_kb_config,
            rag::commands::rag_test_qdrant,
            rag::commands::rag_test_embedding,
            rag::commands::rag_logs_snapshot,
            rag::commands::rag_logs_clear,
            rag::commands::rag_ingest_files,
            rag::commands::rag_ingest_job_status,
            browser::browser_click_at,
            browser::browser_go_back,
            browser::browser_navigate,
            browser::browser_reload,
            browser::browser_get_status,
            browser::browser_minimize,
            browser::browser_restore,
            browser::browser_reopen,
            browser::browser_list_sessions,
            chat_images::resolve_chat_image,
            chat_images::save_chat_image,
            workspace::fs::read_dir_entries,
            workspace::fs::read_file_content,
            workspace::fs::read_image_preview,
            workspace::fs::write_file_content,
            workspace::fs::move_fs_entry,
            workspace::fs::delete_fs_entry,
            workspace::fs::get_file_meta,
            workspace::rope::rope_open,
            workspace::rope::rope_read_lines,
            workspace::rope::rope_edit,
            workspace::rope::rope_replace_line,
            workspace::rope::rope_save,
            workspace::rope::rope_close,
            workspace::rope::rope_undo,
            workspace::rope::rope_redo,
            scm::git::generate_commit_message,
            scm::git::git_status,
            scm::git::git_list_branches,
            scm::git::git_create_branch,
            scm::git::git_checkout_branch,
            scm::git::git_log,
            scm::git::git_commit_detail,
            scm::git::git_show_diff,
            scm::git::git_show_file_diff,
            scm::git::git_file_diff,
            scm::git::git_stage,
            scm::git::git_unstage,
            scm::git::git_stage_all,
            scm::git::git_unstage_all,
            scm::git::git_commit,
            scm::git::git_push,
            scm::git::git_pull,
            scm::git::git_remote_counts,
            project::analytics::read_session_metrics,
            project::analytics::get_weekly_analytics,
            project::config::init_project_config,
            project::config::read_project_config,
            project::config::write_project_config,
            project::mcp::refresh_project_mcp_status,
            project::mcp::set_project_mcp_server_enabled,
            ssh_tool::ssh_tool_load_config,
            ssh_tool::ssh_tool_load_audit,
            ssh_tool::ssh_tool_save_config,
            ssh_tool::ssh_tool_test_server_config,
            project::storage::load_projects,
            project::storage::save_projects,
            project::storage::load_project_tasks,
            project::storage::save_project_tasks,
            platform::app_settings::load_app_settings,
            platform::app_settings::save_app_settings,
            platform::notification::get_notifications,
            platform::notification::mark_notification_read,
            platform::notification::mark_all_notifications_read,
            agent::commands::dispatcher_send_project_agent_message,
            agent::commands::dispatcher_send_chat_agent_message,
            agent::commands::dispatcher_list_messages,
            agent::commands::dispatcher_get_session_token_usage,
            agent::commands::dispatcher_clear_messages,
            agent::commands::dispatcher_truncate_messages_from,
            agent::commands::dispatcher_get_tool_artifact,
            agent::commands::dispatcher_delete_session,
            agent::commands::chat_list_sessions,
            agent::commands::chat_create_session,
            agent::commands::chat_delete_session,
            agent::commands::chat_update_session_title,
            agent::commands::chat_set_session_category_v6,
            agent::commands::project_list_sessions,
            agent::commands::project_create_session,
            agent::commands::project_delete_session,
            agent::commands::session_search_keywords,
            agent::commands::chat_list_categories,
            agent::commands::chat_create_category,
            agent::commands::chat_update_category,
            agent::commands::chat_delete_category,
            agent::commands::chat_set_session_category,
            agent::commands::chat_reorder_categories,
            agent::commands::aha_get_settings_v2,
            agent::commands::aha_save_settings_v2,
            agent::commands::aha_set_active_chat_model,
            agent::commands::aha_get_chat_category_agent_configs,
            agent::commands::aha_save_chat_category_agent_configs,
            agent::commands::aha_get_context_config,
            agent::commands::aha_list_agent_tools,
            agent::commands::aha_resolve_ssh_workspace,
            agent::commands::dispatcher_fetch_models,
            agent::commands::dispatcher_test_model,
            agent::commands::dispatcher_stop_run,
            agent::commands::dispatcher_start_voice_input,
            agent::commands::dispatcher_append_voice_audio,
            agent::commands::dispatcher_finish_voice_input,
            agent::commands::dispatcher_cancel_voice_input,
            agent::sub_agent::commands::sub_agent_list,
            agent::sub_agent::commands::sub_agent_get,
            agent::sub_agent::commands::sub_agent_create,
            agent::sub_agent::commands::sub_agent_update,
            agent::sub_agent::commands::sub_agent_delete,
            agent::sub_agent::commands::sub_agent_seed_browser,
            agent::sub_agent::commands::sub_agent_list_tools,
            agent::sub_agent::commands::sub_agent_set_global_enabled,
            agent::sub_agent::commands::sub_agent_get_global_enabled,
            agent::sub_agent::commands::sub_agent_get_run_trace,
            agent::graph::commands::graph_plan_get,
            agent::graph::commands::graph_plan_latest_for_session,
            agent::graph::commands::graph_plan_update,
            agent::graph::commands::graph_harness_catalog_get,
            agent::graph::commands::graph_run_get,
            agent::graph::commands::graph_run_start,
            agent::graph::commands::graph_run_cancel,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // 应用退出时优雅停止 RAG sidecar，避免僵尸子进程。
            // 退出回调是同步上下文，无法 await；这里只 take + kill，
            // 进程退出由 OS 回收，端口随进程消失而释放。
            if let tauri::RunEvent::ExitRequested { .. } = event {
                let manager = app_handle.state::<rag::RagManager>();
                manager.stop_for_exit();
            }
        });
}
