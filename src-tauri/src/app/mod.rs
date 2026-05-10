use crate::agent::DispatcherState;
use crate::{
    agent, knowledge, platform, project, scm, shared::TaskManager, task_runtime, workspace,
};
use tauri::Manager;

fn build_task_manager() -> TaskManager {
    TaskManager::default()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let project_mcp_registry = project::mcp::ProjectMcpRegistry::default();
    let dispatcher_state = DispatcherState::new(project_mcp_registry.clone())
        .expect("failed to initialize Dispatcher state");

    tauri::Builder::default()
        .setup(|app| {
            if let Ok(resource_dir) = app.path().resource_dir() {
                knowledge::set_resource_dir_hint(resource_dir);
            }
            // 后台预热 login shell 环境，避免第一次启动任务时阻塞
            std::thread::spawn(|| {
                platform::get_login_shell_path();
            });
            Ok(())
        })
        .manage(build_task_manager())
        .manage(dispatcher_state)
        .manage(agent::voice::VoiceAsrManager::default())
        .manage(project_mcp_registry)
        .manage(workspace::RopeManager::new())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_window_state::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            task_runtime::pty::start_dispatcher_subprocess,
            task_runtime::pty::resume_dispatcher_subprocess,
            task_runtime::pty::stop_task,
            task_runtime::pty::send_input,
            task_runtime::pty::resize_pty,
            task_runtime::pty::open_shell,
            task_runtime::pty::kill_shell,
            workspace::fs::read_dir_entries,
            workspace::fs::read_file_content,
            workspace::fs::read_image_preview,
            workspace::fs::write_file_content,
            workspace::fs::move_fs_entry,
            workspace::fs::delete_fs_entry,
            workspace::fs::list_project_files,
            workspace::fs::get_file_meta,
            workspace::fs::read_file_chunk,
            workspace::rope::rope_open,
            workspace::rope::rope_read_lines,
            workspace::rope::rope_edit,
            workspace::rope::rope_replace_line,
            workspace::rope::rope_save,
            workspace::rope::rope_is_dirty,
            workspace::rope::rope_close,
            workspace::rope::rope_undo,
            workspace::rope::rope_redo,
            knowledge::collection::knowledge_list_collections,
            knowledge::collection::knowledge_create_collection,
            knowledge::collection::knowledge_update_collection,
            knowledge::collection::knowledge_delete_collection,
            knowledge::settings::knowledge_get_settings,
            knowledge::settings::knowledge_save_settings,
            knowledge::settings::knowledge_test_model,
            knowledge::ingest::knowledge_import_sources,
            knowledge::jobs::knowledge_get_ingest_jobs,
            knowledge::jobs::knowledge_cancel_ingest,
            knowledge::jobs::knowledge_retry_ingest,
            knowledge::pages::knowledge_list_pages,
            knowledge::pages::knowledge_read_page,
            knowledge::pages::knowledge_write_page,
            knowledge::pages::knowledge_delete_page,
            knowledge::search::knowledge_search,
            knowledge::chunk::knowledge_reindex_collection,
            knowledge::cache::knowledge_vector_stats,
            knowledge::graph::knowledge_build_graph,
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
            task_runtime::session::read_session_messages,
            project::config::init_project_config,
            project::config::read_project_config,
            project::config::write_project_config,
            project::mcp::refresh_project_mcp_status,
            project::mcp::set_project_mcp_server_enabled,
            project::config::read_agent_config_file,
            project::config::write_agent_config_file,
            project::storage::load_projects,
            project::storage::save_projects,
            project::storage::load_project_tasks,
            project::storage::save_project_tasks,
            platform::app_settings::load_app_settings,
            platform::app_settings::save_app_settings,
            platform::app_settings::detect_agent_paths,
            platform::app_settings::detect_agent_versions,
            platform::app_settings::detect_agent_versions_for_settings,
            platform::notification::get_notifications,
            platform::notification::mark_notification_read,
            platform::notification::mark_all_notifications_read,
            platform::usage::read_usage_snapshot,
            agent::commands::dispatcher_send_message,
            agent::commands::dispatcher_send_plain_chat_message,
            agent::commands::dispatcher_list_messages,
            agent::commands::dispatcher_get_session_token_usage,
            agent::commands::dispatcher_clear_messages,
            agent::commands::dispatcher_clear_message_context,
            agent::commands::dispatcher_get_tool_artifact,
            agent::commands::dispatcher_get_settings,
            agent::commands::dispatcher_list_sessions,
            agent::commands::dispatcher_create_session,
            agent::commands::dispatcher_get_session_runtime_state,
            agent::commands::dispatcher_set_session_mode,
            agent::commands::dispatcher_delete_session,
            agent::commands::dispatcher_save_settings,
            agent::commands::dispatcher_set_auto_approve_dispatch,
            agent::commands::dispatcher_fetch_models,
            agent::commands::dispatcher_continue_after_dispatch,
            agent::commands::dispatcher_attach_checklist_subprocess,
            agent::commands::dispatcher_clear_checklist_dispatch,
            agent::commands::dispatcher_stop_run,
            agent::commands::dispatcher_start_voice_input,
            agent::commands::dispatcher_append_voice_audio,
            agent::commands::dispatcher_finish_voice_input,
            agent::commands::dispatcher_cancel_voice_input,
            agent::commands::dispatcher_mark_subprocess_round_completed,
            agent::commands::dispatcher_mark_subprocess_running,
            agent::commands::dispatcher_mark_subprocess_stopped,
            agent::commands::dispatcher_mark_subprocess_finished,
            agent::commands::dispatcher_send_to_subprocess,
            agent::commands::dispatcher_exit_subprocess,
            agent::commands::dispatcher_is_subprocess_exited,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
