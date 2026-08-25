use crate::agent::DispatcherState;
use crate::{
    agent, browser, chat_images, mcp, platform, project, python_runner, rag, scm,
    shared::TaskManager, ssh_tool, task_runtime, workspace,
};
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;

fn build_task_manager() -> TaskManager {
    TaskManager::default()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(move |app| {
            // G11-02：DispatcherState 构造移出主线程同步路径——DB 打开/迁移、
            // 中断运行恢复、子智能体配置加载等阻塞 I/O 全部在 spawn_blocking
            // 中进行，这里只等待异步构造结果。失败时给出可读错误对话框并退出，
            // 而不是裸 panic 导致无法启动且无任何提示。
            let dispatcher_state = tauri::async_runtime::block_on(DispatcherState::new());
            let dispatcher_state = match dispatcher_state {
                Ok(state) => state,
                Err(error) => {
                    let message = format!("初始化智能体核心状态失败：{error:#}");
                    eprintln!("{message}");
                    app.dialog()
                        .message(message)
                        .title("JKCodingAgent 启动失败")
                        .blocking_show();
                    return Err("初始化智能体核心状态失败".into());
                }
            };
            // SSH 管理器由 DispatcherState 内部基于共享 DB 连接池创建，
            // 这里把同一实例注册给 Tauri 命令层（Clone 共享底层 Arc 与连接池）。
            app.manage(dispatcher_state.ssh_manager());
            // MCP 注册表由 DispatcherState 内部基于共享 DB 构造，
            // 这里把同一实例注册给 Tauri 命令层（Clone 共享底层缓存与连接池）。
            app.manage(dispatcher_state.mcp_registry());
            // RAG 配置权威源同样在全局库：注入读取入口（store 在 builder 链注册）。
            app.state::<rag::RagConfigStore>()
                .attach_db(dispatcher_state.db().clone());
            app.manage(dispatcher_state);

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
        .manage(browser::BrowserManager::default())
        .manage(python_runner::PythonRunnerState::default())
        .manage(workspace::RopeManager::new())
        .manage(rag::RagManager::default())
        .manage(rag::RagConfigStore::default())
        .manage(rag::RagLogStore::default())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_window_state::Builder::new().build())
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            task_runtime::pty::send_input,
            task_runtime::pty::resize_pty,
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
            project::config::init_project_config,
            mcp::commands::mcp_project_status,
            mcp::commands::mcp_global_status,
            mcp::commands::mcp_global_status_recent,
            mcp::commands::mcp_project_set_server_enabled,
            mcp::commands::mcp_global_config_get,
            mcp::commands::mcp_global_config_save,
            ssh_tool::ssh_tool_load_config,
            ssh_tool::ssh_tool_load_audit,
            ssh_tool::ssh_tool_save_config,
            ssh_tool::ssh_tool_test_server_config,
            ssh_tool::config_import::ssh_tool_import_ssh_config,
            project::storage::load_projects,
            project::storage::save_projects,
            project::storage::project_delete,
            platform::app_settings::load_app_settings,
            platform::app_settings::save_app_settings,
            platform::notification::get_notifications,
            platform::notification::mark_notification_read,
            platform::notification::mark_all_notifications_read,
            agent::commands::dispatcher_send_project_agent_message,
            agent::commands::dispatcher_send_chat_agent_message,
            agent::commands::dispatcher_list_messages,
            agent::commands::dispatcher_get_tool_run_tree,
            agent::commands::dispatcher_get_session_token_usage,
            agent::commands::dispatcher_clear_messages,
            agent::commands::dispatcher_truncate_messages_from,
            agent::commands::dispatcher_get_tool_artifact,
            agent::commands::chat_list_sessions,
            agent::commands::chat_create_session,
            agent::commands::chat_delete_session,
            agent::commands::chat_set_session_category_v6,
            agent::commands::project_list_sessions,
            agent::commands::project_create_session,
            agent::commands::project_delete_session,
            agent::commands::session_search_keywords,
            agent::commands::chat_list_categories,
            agent::commands::chat_create_category,
            agent::commands::chat_update_category,
            agent::commands::chat_delete_category,
            agent::commands::aha_get_settings_v2,
            agent::commands::aha_save_settings_v2,
            agent::commands::aha_get_chat_category_agent_configs,
            agent::commands::aha_save_chat_category_agent_configs,
            agent::commands::aha_list_agent_tools,
            agent::commands::dispatcher_fetch_models,
            agent::commands::dispatcher_test_model,
            agent::commands::dispatcher_stop_run,
            agent::sub_agent::commands::sub_agent_list,
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
            agent::graph::commands::graph_run_resume,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // 应用退出时优雅停止 RAG sidecar，避免孤儿进程。
            // 退出回调是同步上下文，无法 await；这里只 take + kill，
            // 进程退出由 OS 回收，端口随进程消失而释放。
            //
            // 必须同时挂 ExitRequested 与 Exit 两个事件：
            // - macOS 的 Cmd+Q / Dock 退出走 applicationShouldTerminate，
            //   tao 只发 LoopDestroyed（→ RunEvent::Exit），不发
            //   ExitRequested（仅「最后一个窗口 Destroyed」与
            //   app_handle.exit() 两条路径触发），只挂 ExitRequested
            //   会在 macOS 正常退出时漏杀 sidecar（tauri#9198/#13778）。
            // - RunEvent::Exit 在事件循环结束后、进程退出前必达，
            //   是兜底清理点；stop_for_exit 内部 take() 幂等，重复触发无害。
            if matches!(
                event,
                tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
            ) {
                let manager = app_handle.state::<rag::RagManager>();
                manager.stop_for_exit();
            }
        });
}
