## 后端死代码清理计划

基于从命令注册页（`app/mod.rs` 的 `invoke_handler`）出发的完整排查，已确认 **14 个前端从未调用、后端无内部复用的死命令**。本计划逐一删除这些命令及其独有的实现代码。

### 第一步：删除死命令函数 + 连带独占的 impl/DB 方法

| 文件 | 删除的命令函数 | 连带删除的独占代码 |
|------|---------------|-------------------|
| `agent/commands.rs` | `dispatcher_clear_message_context`、`dispatcher_list_sessions`、`dispatcher_create_session`、`session_get_keywords`、`session_search_keywords`、`dispatcher_is_subprocess_exited`、`aha_get_shared_models` | 仅这些命令用的 import |
| `agent/db/messages.rs` | — | `clear_context_messages()`（仅被 dispatcher_clear_message_context 调用） |
| `agent/db/sessions.rs` | — | `list_sessions()`、`create_session()`（仅被待删 dispatcher 命令调用；chat/project 版本用的是独立的 paginated 方法） |
| `agent/db/keywords.rs` | — | `search_sessions_by_keywords()`（仅被 session_search_keywords 调用）。**保留** `list_session_keywords`/`apply_keyword_actions`（被存活的关键词生成后台逻辑使用） |
| `agent/db/settings.rs` | — | `get_shared_models()`（仅被 aha_get_shared_models 调用） |
| `workspace/fs.rs` | `list_project_files`、`list_project_files_impl`、`read_file_chunk`、`read_file_chunk_impl` | — |
| `workspace/rope.rs` | `rope_is_dirty` | — |
| `chat_images.rs` | `save_chat_image`、`save_chat_image_impl` | — |
| `rag/commands.rs` | `rag_health`、`rag_health_impl`、`rag_sidecar_config`、`rag_sidecar_config_impl` | — |
| `ssh_tool/mod.rs` | `ssh_tool_test_connection` | — |
| `platform/app_settings.rs` | `detect_agent_versions` | — |

### 第二步：从命令注册表移除

在 `app/mod.rs` 的 `generate_handler!` 列表中删除上述 14 个命令的注册行。

### 第三步：清理孤立的类型与 import

- 删除 `SessionSearchResult` 类型（仅 search 用）。
- 删除 `AhaSharedModels`（如仅 get_shared_models 用，需确认）。
- 清理 `agent/commands.rs`、`agent/db/mod.rs` 中因删除产生的未使用 import。
- **保留** `list_session_keywords`/`apply_keyword_actions`/`SessionKeywordRecord`/`KeywordAction`（存活后台逻辑用）、`is_subprocess_exit_requested`（state 方法，保守保留）、`list_sessions` 对应的 `DispatcherSessionKind`/`DispatcherSessionRecord`（delete_session 仍用）。

### 第四步：处理保留型死命令（保守不删函数，仅留注册）

以下命令前端未调用，但**后端内部有大量复用**，仅从注册表移除收益小且有风险，本计划**保留不动**，仅记录：
- `read_project_config` / `write_project_config`（pty.rs、git.rs、browser.rs 内部复用）
- `read_session_metrics` / `read_session_messages`（独立工具，未来可能用）

### 第五步：依赖核查

已确认 `Cargo.toml` 所有依赖均有使用（`tokio-tungstenite` 用于 voice.rs 的 WebSocket ASR，`ssh2`、`image` 等均被引用）。**无需删除依赖**。

### 第六步：验证

1. `cargo check`（src-tauri）确认无编译错误、无新增 dead_code 警告。
2. `pnpm build`（tsc 类型检查）确认前端无引用断裂。
3. 报告清理掉的代码行数与删除清单。

### 不在本次范围内

- AGENTS.md 中提到的技术债重构（spawn_blocking 包裹、锁优化、PTY 缓冲区扩大等）——这是独立的较大重构，需单独进行。本次聚焦"扫描命令→清理死代码"。

### 风险控制

- 每个删除前已验证调用链：前端零出现 + 后端无存活调用者。
- 连带删除的 DB 方法均经 grep 确认仅被待删命令调用。
- 编译器会兜底：若有遗漏引用，`cargo check` 会立即报错。