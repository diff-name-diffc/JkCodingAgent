# JKCodingAgent — AGENTS.md

桌面智能体应用：以会话为核心，dispatcher 智能体运行时（多轮工具调用、子智能体、命令审查门禁；项目 Agent = 工作流编排器，节点经 ACP 子进程执行）+ RAG + Shell/浏览器/Python 运行器 + 文件浏览器 + Git，外壳 Tauri 2。技术栈：React 19 + TS + Vite · Tauri 2 + Rust · Tailwind v4 CSS-first + shadcn 风格 · Zustand + React Query · xterm.js · Shiki · rusqlite。

## 命令

```bash
pnpm dev              # Vite（端口 1420）
pnpm typecheck        # tsc 全量（与 build 解耦，CI 单独跑）
pnpm build / lint / test
pnpm format           # Prettier 写入，提交前必跑（CI 门禁 format:check）
pnpm contract:check   # Tauri 命令双向契约（后端注册 ↔ 前端调用）
pnpm styles:report    # .ai-* 样式类双向 fail-closed 报告
pnpm tauri dev / tauri build   # 桌面应用（bundle 只出 .app）
pnpm app:install      # 退旧应用、覆盖安装 /Applications 并启动
pnpm rag:dev / rag:sync / rag:build   # RAG sidecar（uv）
```

Vitest 用例放被测模块旁 `*.test.ts(x)`（优先纯函数）。Rust 改后需重启 `tauri dev`。

## 代码索引

### 前端 `src/`

- `App.tsx` — 挂 WelcomePage / ProjectPage；跨视图状态 + Tauri 事件监听
- `types/` — 契约类型权威子模块（architecture / chat / infrastructure / rag / ssh-terminal / sub-agent / workflow），`types.ts` 统一再导出；改结构先改子模块
- `styles/tailwind.css` — Tailwind v4 CSS-first 入口（`@theme inline`、`@custom-variant dark`）+ `.ai-*` 组件类；`App.css` — 设计令牌（`:root` / `.dark` 双套）
- `lib/cn.ts`（clsx+tailwind-merge）· `lib/file-clipboard.ts`（文件剪贴板封装）· `lib/theme.ts`
- `stores/` — Zustand：`ui-store` / `workspace-store` / `chat-files-store`
- `components/ui/` — shadcn 基础组件；`providers/` — React Query 等全局 Provider
- WelcomePage 三视图：`HomeChatPage` + `chat/chat-shell.tsx`（独立聊天）· 项目网格 · `architecture/ArchitectureView`（画板经 architecture_canvas_* 命令存 `~/.jkcodingagent/architecture-canvases/`）
- ProjectPage：`ContextNav`（会话/文件/变更/历史）· `SessionPanel` · `chat-page-v2`（dispatcher 工作台）· `workflow/`（工作流编排 UI，`workflow_plan_list_for_session` 拉轻量摘要）· `SubAgentExecutionView` · `file-explorer/` + `FileViewer`/`LargeFileViewer` · `GitChanges`/`GitHistory` · `ShellTerminalPanel` · `BrowserPanel` · `shell/StatusDockBar` · `AppSettingsDialog`
- 分类文件面板：`chat-files/ChatCategoryFilesLayout`（chat-shell 内联）lazy 加载 `ChatCategoryFilesOpenLayout`（独立布局，复用 `project/main-tabs` 编辑器标签）；纯逻辑 `chat-files/chat-files-model.ts`；目标状态在 `chat-files-store`
- 文件剪贴板（⌘C/⌘V 复制真实文件）：快捷键/落点纯逻辑 `file-explorer/file-clipboard-actions.ts`，接线 `use-file-tree-clipboard.ts`，粘贴落盘走 `copy_fs_entry`
- 设置中心 `components/settings/`：`use-aha-settings.ts`（统一 store + debounce 自动保存）+ providers/ssh/tools/sub-agents/mcp/rag 各页；通知走 `components/Toast.tsx` 命令式 API
- 事件契约（改动即破坏前端）：`AgentEvent`/`AgentTurn` ← `src-tauri/src/agent/rig_ext/events.rs`；`workflow-plan-updated`/`workflow-run-event` ← `agent/workflow/types.rs`。在用事件：`dispatcher-session-updated` `sub-agent-event` `workflow-plan-updated` `workflow-run-event` `python-run-event` `shell-output` `browser-frame`/`browser-log`/`browser-status` `rag-log`
- webview 重载后运行对账：`components/dispatcher-chat/run-state-reconciliation.ts`（配 `dispatcher_active_runs` 命令）

### 后端 `src-tauri/src/`

命令注册集中 `app/mod.rs` 的 `invoke_handler!`。

- `agent/` — dispatcher 核心（rig-core 0.42 portable contracts）：
  - `rig_ext/model.rs`（用途槽位→rig 模型、`PurposeSwitchingModel`）；`message.rs`（消息桥、`attach_turn_tool_images`、`apply_stored_session_summary`）；`loop/`（多轮工具循环+三段式执行+台账+协议拦截；`loop/batch.rs::persist_skipped_tool_results`）；`context.rs`（`compact_history` / `compact_history_offline` / `context_budget_chars`）；`tool_result.rs`（结果落盘/压缩）；`events.rs`；`tools/`（fs/edit/exec/log/media/program/mcp 组 + `spec.rs::TOOL_POLICY_TABLE`、`deps.rs::RigToolDeps`、`mod.rs::ORCHESTRATOR_RUNTIME_TOOL_NAMES`）；`agents/`（`plain_chat.rs::build_surface`、协议壳 `project_tools.rs`、拦截 `project_submit.rs` / `project_workflow_ops.rs`）；`sub_agent/runner.rs::build`（继承普通聊天 profile）；`chat_workspace.rs`（普通聊天目录系统提示注入）
  - `log_snapshot/`（会话日志代次、后台覆盖更新、FTS5 索引、精准扫描与分页精读）；`ssh_review/command_gate.rs`（`ssh_exec` 与日志远程工具共用的审查/确认/拦截审计门禁）
  - `workflow/`（定义/校验/执行引擎/acp_exec；`commands.rs::apply_draft_definition_update` 与前端 `workflow_plan_update` 同源）
  - `db/`（`schema.rs::SCHEMA_VERSION` = v18；settings/messages/sessions/summaries 等）
  - `commands/`（如 `chat_uploads.rs::chat_upload_file`）· `config.rs` · `chat_directories.rs`（分类共享目录校验）· `ssh_review.rs`（命令安全审查）· `state/`（`DispatcherState::session_run_is_active`、`ActiveRunStore`）· `common/message.rs::repair_tool_call_pairing`
- `task_runtime/pty.rs` · `project/storage.rs`（项目/会话存储、`project_delete`）· `mcp/`（`McpScope{Global,Project}`，Project = 全局 ∪ 项目 `.jkcodingagent/mcp.json` 同名覆盖）· `scm/git.rs` · `workspace/fs.rs`（读写/列举/`copy_fs_entry`）/ `rope.rs` · `platform/`（`app_settings.rs`、`clipboard_files.rs` = `clipboard_write_files`/`clipboard_read_files`，NSPasteboard 文件 URL + 路径文本）· `rag/` · `ssh_tool/`（SSH 执行 + AI 审查门禁、连接池、TOFU、备忘录 `~/.jkcodingagent/ssh-memos/`；`command_stream.rs` 大日志有界流式落盘）· `browser.rs` + `browser/`（sidecar 宿主；通用工具仅 `browser_open_url`，browser-agent 经 `call_sub_agent` 委派）· `chat_images.rs::save_image` · `python_runner.rs` · `architecture_canvas.rs`（白名单画板 id 防穿越）

### 数据与持久化

- SQLite `~/.jkcodingagent/jkbot.sqlite3`；资源目录 `~/.jkcodingagent/`（`chat-images/`、`plain-chat-browser/`、`ssh-memos/`、`architecture-canvases/`、`memory/`、`skills/`、`local_env/zsh/`）。应用配置权威 = 全局库（SSH / projects / mcp_servers / app_config / 模型库 / 主题 `dispatcher_settings.theme`）；项目目录只放 `.jkcodingagent/config.toml`（`[git].commit_prompt`）与 `mcp.json`
- 模型配置唯一权威 `dispatcher_settings.model_library`：用途槽位存 `libraryId` 引用、落库剥离凭据；容量缺省 maxTokens 未配→请求省略、contextWindow 未配→1M（`db::DEFAULT_CONTEXT_WINDOW_CAPACITY_TOKENS`）
- 普通聊天分类目录：`ChatCategoryAgentConfig.projectDirectories` 共享工程目录（校验在 `chat_directories.rs`）+ 会话独立临时目录 `plain-chat-browser/{会话目录}`；回形针非图片经 `chat_upload_file`（写租约、同名递增后缀、32 MB 上限）拷入会话目录根；图片统一 `chat-image://{image_id}` 协议，唯一保存入口 `chat_images::save_image`
- 日志快照存 `{会话目录}/log-snapshots/{snapshot_id}/`：`current.json` 原子指向当前代次，`generations/{generation}/` 含原文、清单与独立 FTS5 `index.sqlite3`，`.staging/` 为未发布暂存；不占全局业务库 schema

## 行为约束

### 格式化 / TS / 状态 / UI

- 提交前必格式化：Rust `cargo fmt`（`src-tauri/`）；前端 `pnpm format`。排版以工具输出为准，不手写对齐、不在评审讨论格式。
- TS 严格模式，避免 `any`，扩展 `types/`；Tauri 命令 `invoke<ReturnType>()`；export-for-test 不算导出面污染。
- 全局 UI = Zustand、异步 = React Query，不引入第二套状态库。
- UI 原语用 Radix / `components/ui/`（非原生 `<select>`/`<dialog>`）；图标 `lucide-react`。

### Rust / Tauri

- 路径参数命令必须校验位于工作区内（防目录遍历）。
- 重 I/O、进程、网络、Git 一律 `spawn_blocking`，不阻塞主线程。
- 持锁（`parking_lot::Mutex`）期间禁止 I/O；`Mutex` 获取禁止裸 `.unwrap()`。
- 用 `tauri::Emitter` 推事件，不从命令返回大数据。
- 可执行命令的工具必须接入 AI 审查 fail-closed 门禁（`deps.review` + `ssh_review`）。
- 测试临时目录一律 `test_util::TempDirGuard`，禁止裸 `env::temp_dir()`+手工清理。
- `read_file_content` ≤ 2 MB（Rust 侧强制）。

### 样式

- 优先级：`components/ui/` → Tailwind 工具类（`cn()` 合并）→ `.ai-*`（`@layer components`）。
- 令牌在 `App.css`（亮/暗成对维护，不硬编码色值）；Tailwind v4 CSS-first，配置在 CSS、不用 config js。
- preflight 已禁用；未分层样式优先级高于一切工具类——全局/元素级 reset 只能进 `@layer base`（层序由 `styles/tailwind.css` 顶部锚定）。
- 不新建业务 `.css`、不引入 CSS-in-JS；一次性样式行内 `style`。

### 性能与规模

- 列表行组件 `memo`；高频事件（PTY 输出等）buffer/ref 批处理；长列表（消息流/文件列表）必须虚拟化；单条消息 >10KB 禁止同步 `marked()`；Shiki/CodeMirror/Monaco 语言包动态 `import()`；@提及/搜索 ~200ms 防抖。
- PTY 读缓冲 ≥ 32KB；会话消息流式/分页，禁止整文件加载。
- 单生产文件 ≤ 500 行（前端建议 ≤ 400）；超限按变化原因拆分，新增功能落在超限文件先拆再扩展。

### Agent 工具（rig `PortableDynamicTool`）

新增四步：① `rig_ext/tools/<组>.rs` 实现（依赖经 `RigToolDeps`；参数提取 `common::{string_arg, usize_arg, boolish_arg}`、路径沙箱 `resolve_path`、错误 `ToolExecutionError` 以「错误：」开头 / `.with_retryable(true)` / `.with_code("fatal")`）→ ② 挂工具面（普通聊天 `plain_chat::build_surface`；编排器数据面另登记 `ORCHESTRATOR_RUNTIME_TOOL_NAMES`；子智能体 `runner::build`；协议壳 `project_tools.rs` + 协议拦截）→ ③ `spec.rs::TOOL_POLICY_TABLE` 登记（台账/审查/超时/压缩策略唯一来源，未收录 = fail-closed 兜底）→ ④ 需要配置则 `config.rs` + `load()`。

- **配对不变量**：assistant 的 `tool_calls` 与 tool 结果必须成对进入 LLM 上下文（缺失则整轮 400）。写侧 `persist_skipped_tool_results`，读侧 `repair_tool_call_pairing`。
- 超时：统一超时工具由策略层包（到点取消→宽限收敛→移交后台收口，绝不 drop future）；自管超时工具（`browser_open_url`/`call_sub_agent`/命令执行类，清单以策略表为准）自带预算与优雅终止，**必须登记 `self_managed_settle_ceiling_secs`**；`local_zsh` 走 yield/session 模型（超窗转后台返回 `run_id`、`wait_run_id` 收割；后台仍受 `exec_timeout_secs` 硬超时；审查在启动前）。调用可声明 `timeout_secs` 白名单仅 `generate_image`/`edit_image`/`fetch_image`，单一解析出处 `spec.rs::effective_timeout_secs`（策略层与工具内 HTTP 预算必须共用）；新增白名单须同步五处（match / schema 注入 / 设置中心默认 / `tools/deps.rs` 下传路由 / 守护测试）。
- 压缩：`compress=true` + 阈值双条件（命令类 12000、默认 5000）；命令类整体截断前先行级截断（单行 >500 字符保行首）；截断标记带可执行恢复指引；工具产物仅供界面查看，模型不可再读。
- 日志调查工具：`ssh_log_discover` → `ssh_log_snapshot_update`（首次创建/后续同 ID 覆盖更新，sources 可省略复用上一代，支持 Docker、SSH 文件、本地授权文件）→ `ssh_log_snapshot_status` / `cancel` → `ssh_log_search`（fixed 优先 FTS5 trigram，regex/短模式走有界 ripgrep）→ `ssh_log_read`（行/字节分页）。每次更新全量同步并自动建索引，全部成功才原子切换 generation，失败/取消保留旧代次；旧 cursor 不跨代次使用。先 files/count 收敛，再 matches 定位、read 精读，不默认全量日志进上下文。`LogSnapshotManager` 由 `AgentServices` 应用级共享，不随 run 重建；删除/清空先 `block_workspace`，活动更新拒绝清理，清理期间禁止新任务。
- 日志观测与证据：`update.json` / manifest 记录 review/download/index/publish/total 耗时、索引行数/字节/比例及每来源采集速率；`SnapshotFile.evidence` 保存采集前后 device/inode/size/mtime 元数据指纹和 `sourceChanged`，变更来源进入 search/read 的 `warnings`。指纹不是内容哈希，false 仅表示观测元数据未变；Docker 改存冻结 since/until/tail 窗口，不伪造文件指纹。index.sqlite3 与 wal/shm 显式 `0600`。检索词和待排查字段只放 `pattern/patterns`，结果量只用 `max_matches/max_files/context/max_line_chars`，`max_output_tokens` / `max_tokens` 等模型参数不是日志工具参数。

### 上下文整形

- 主对话每轮 `compact_history`（保头保尾 + 配对安全滑窗；中段滚动摘要入 `dispatcher_session_summaries` 跨 run 持久化，摘要模型失败回退规则抽取）；上下文 400 超限预算减半重试 ≤ 2 次；reasoning 不回灌。
- 子智能体 `compact_history_offline` 规则兜底，保护头部 2 条。
- 预算 `context_budget_chars` = 窗口 × 3.5 字符/token × 0.6。

### 存储 schema

- 当前 v18。schema 变更三件套缺一不可：① 更新基线 DDL；② `SCHEMA_VERSION` +1 且 `init()` 追加 `if current_version < N` 事务迁移块（幂等）；③ 破坏性操作前整库快照（`VACUUM INTO`）。禁止改写历史迁移块；领域表 DDL 放各领域 `ensure_*_tx`。
- 改数据结构必须同步 `types/` 契约与 Rust 结构体/SQL——新字段序列化时被静默丢弃。
- 开发阶段重置本地库：退应用后删 `~/.jkcodingagent/jkbot.sqlite3`（连同 `-wal`/`-shm`）。

### 会话资源清理

- 删除/清空会话必须同步清理文件资源，不依赖 DB 级联；活动 run 时 fail-closed 拒绝（`session_run_is_active` 守卫 `session_delete`/`dispatcher_clear_messages`/`dispatcher_truncate_messages_from`/`project_delete`）；新增破坏性命令接入同一守卫。
- 图片随 `chat_images` 记录整目录回收；`truncate_messages_from` 有意保留图片文件（重发复用 image_id）。
- 工具产物文件（`dispatcher_tool_artifacts`）显式清理；`project_delete` 同事务级联全项目会话，保留 config.toml / mcp.json。
- 截断（regenerate/编辑重发）还须回收子智能体 trace（按 `tool_call_id`）、工作流产物（按计划创建时刻）、滚动摘要（锚点在被删范围精确删除）；有意保留 token 用量与会话关键字。
- 普通聊天清理只回收会话临时目录（含日志快照原文、索引和暂存目录），**绝不回收分类绑定的共享工程目录**。
