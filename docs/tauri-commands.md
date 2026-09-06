# Tauri 命令全景清单与合并/删除分析（v2 深度复核版）

> 初版：2026-09-06 · 深度复核：2026-09-06 · 统计口径：`src-tauri/src/app/mod.rs` 的 `invoke_handler!` 全量注册命令（130 条）。
> v2 复核方式：在 v1（后端注册清单与前端 `invoke()` 调用点双向比对 + 7 域分组核查）基础上，对全部「有疑点」的命令接口逐条下钻到 Rust 实现、DB 层、前端 hook 与事件联动，验证 v1 结论并补漏。本版新增：**通知域整体死功能判定**、**会话级联清理三处手工双写**、**useBindChatModel 无界陈旧覆盖**、**SSH/RAG 凭据明文回传双标**等 v1 未覆盖的设计问题，并给出完整的目标命令面重设计（第九节）。
>
> 标识图例：🟢 保留 · 🟡 可合并（附目标方案与优先级） · 🔴 可删除 · 🔧 保留但建议改造

---

## 〇、执行状态（v3 · 2026-09-06 已实施）

第九节方案已于 2026-09-06 全量实施（两个文档自评「可不做」的可选项除外），**命令面 130 → 115**（`pnpm contract:check` 验证：115 个后端注册命令，112 个前端直接调用命令）。第二~八节的域表是 v2 复核快照，**执行差异以本节与第九节状态列为准**。

### 已删除 / 已合并命令映射（旧 → 新）

| 旧命令 | 处置 | 新形态 |
|---|---|---|
| `get_notifications` / `mark_notification_read` / `mark_all_notifications_read` | 🔴 整域删除 | 无（NotificationBell 组件、`platform/notification.rs`、类型与 `.ai-notification-*` / `.ai-sidebar-tool-button` 样式一并移除） |
| `browser_start_plain_chat` | 🔴 并入 start | `browser_start(project_path: Option<String>)`（空白/缺省回退 plain-chat 工作区） |
| `chat_delete_session` + `project_delete_session` | ⭐⭐⭐ 合并 | `session_delete(session_id)`（DB 层 `delete_session` 查统一表 kind 分流子表删除） |
| `chat_create_session` + `project_create_session` | ⭐⭐ 合并 | `session_create(kind, title, category?, project_id?)`；category 缺省 `"tech"` 收敛到 DB 层单点 |
| `git_stage` / `git_unstage` / `git_stage_all` / `git_unstage_all` | ⭐⭐ 合并 | `git_stage(project_path, files: Option<Vec<String>>, unstage: Option<bool>)`（`files` 缺省/空 = 全量） |
| `browser_minimize` / `browser_restore` / `browser_reopen` | ⭐⭐ 合并 | `browser_window_action(session_id, action: "minimize"\|"restore"\|"reopen")`（manager 层超时/语义差异保留） |
| `ssh_tool_load_config` + `ssh_tool_load_audit` | ⭐⭐ 合并 | `ssh_tool_load_settings() → { servers, audit }` |
| `rag_test_qdrant` + `rag_test_embedding` | ⭐⭐⭐ 合并 | `rag_test_connection(config, target: "qdrant"\|"embedding")`，**内部 save 已移除**（保存责任归调用方，双重落库已修） |
| `mcp_project_status` + `mcp_global_status` | ⭐ 合并 | `mcp_status(project_path?, force_refresh?)`（传路径 = 项目作用域无条件探活；不传 = 全局 ensure_recent / force_refresh 强刷） |
| `sub_agent_get_global_enabled` | ⭐ 折叠 | `sub_agent_list` 返回体新增 `globalEnabled` 字段（LEFT JOIN `global_sub_agents`；「全局可用」= `enabled && globalEnabled`） |
| `chat_set_session_category_v6` | 🔧 改名 | `chat_set_session_category`（`_v6` 后缀摘除） |
| `dispatcher_stop_run` | 🔧 归组 | 实现移至 `run_commands.rs`（注册路径 `agent::commands::run_commands::dispatcher_stop_run`） |
| `Project.branch` 死列 | 🔴 字段级删除 | schema v3→v4 迁移（`ALTER TABLE projects DROP COLUMN branch` + 迁移前 `VACUUM INTO` 快照；基线 DDL/Rust struct/TS interface/WelcomePage pill 同步移除） |

### 9.4 工程修缮执行情况

1. ✅ `dispatcher_get_session_token_usage` 改 async + `run_dispatcher_db`。
2. ✅ `dispatcher_stop_run` 移出 settings_commands → run_commands（含跨域副作用注释）。
3. ✅ `dispatcher_clear_messages` 补 `forget_session`（与删会话执行对齐）。
4. ✅ 会话级联清理抽共享 helper `db/purge.rs::purge_session_resources_tx(tx, workspace_id)`——clear_messages / delete_session / delete_project 三处统一；truncate 的「有意保留」语义独立保留。
5. ✅ 三个 send 抽 `run_commands::run_agent_turn_skeleton`（begin_run → 守卫 → run_agent_turn → finish → spawn 元数据；Agent 构建差异由调用方 future 惰性完成，架构命令 `with_keywords=false`）。
6. ✅ browser 命令组抽 `resolve_project_path(Option<String>)`（5 处回退 match 消除）。
7. ✅ `init_project_config` 改 async + spawn_blocking。
8. ✅ `write_file_content` 改原子写（复用 `project::storage::atomic_write`）+ 2MB 尺寸上限（`MAX_WRITE_FILE_BYTES`，与 read 侧 2MB 分流对齐）。
9. ✅ `ToolCatalog` 直写式死缓存删除（`registered_tool_names` 读取即重建、返回类型 Option→Vec；`tool_catalog.rs` 仅保留枚举助手）。
10. ✅ `graph_run_cancel` 残留自愈分支 DB 错误留痕（复位成功才广播事件）。
11. ⏸ PTY 命令组命名统一（`pty_*` 前缀 + 参数归一）——按本文档「下次触碰该域时顺带」暂缓；且 `send_input` 同时服务 agent 任务 stdin，重命名需专门梳理，不宜搭车。
12. ✅ `_v6` 后缀摘除（见上表）。
13. ✅ `architecture_run_complete` 补 `workspace_id` 范围校验（ArchRunRegistry 条目记录归属会话，错会话回传按未消费处理、槽位保留）。

### 连带前端变化

- `SidebarFooterActions` 不再挂载 NotificationBell（应用菜单仅剩设置入口）。
- `use-aha-settings` 全局启用集合改从 `sub_agent_list` 过滤（`enabled && globalEnabled`）。
- `SshServersPage` 成对 `Promise.all` 改单次 `ssh_tool_load_settings`。
- `useRagKbConfig.runTest` 统一走 `rag_test_connection`（前端先 `persistConfig` 的顺序不变，后端不再二次落库）。
- `types/sub-agent.ts` 的 `SubAgentRecord` 新增 `globalEnabled: boolean`；`types/infrastructure.ts` 移除 `Project.branch` 与通知类型。

### 验证记录（2026-09-06）

`cargo test`（src-tauri）503 通过（新增 v3→v4 迁移测试、arch run 越会话回传拒绝测试）；`pnpm contract:check` 115 注册 / 112 调用对齐；`pnpm build`（tsc + vite）、`pnpm lint`（--max-warnings 0）、`pnpm test`（11 通过）、`pnpm styles:report`（0 个无引用定义）全部通过。

---

## 一、总览

| 业务域 | 命令数 | 注册位置（app/mod.rs 行号） | 主要前端消费方 |
|---|---|---|---|
| A. 会话 / 消息 / Agent 运行 | 18 | :166-183 | chat-page-v2、dispatcher-chat、SessionPanel |
| B. 聊天分类 / 应用设置 / 模型 | 12 | :184-195 | AppSettingsDialog 各页、sidebar |
| C. 子智能体 / 图编排 | 17 | :196-212 | SubAgentManagePanel、components/graph |
| D. Git 集成 / 项目管理 | 20 | :101, 131-136, 152-155 | GitChanges、GitHistory、App.tsx |
| E. 文件工作区 / Rope 编辑 / 聊天图片 / 通知 | 20 | :114-130, 163-165 | file-explorer、file-viewer、NotificationBell |
| F. 内嵌浏览器 / Python 运行器 / PTY Shell | 21 | :82-113 | BrowserPanel、ShellTerminalPanel、PythonRunDrawer |
| G. RAG / MCP / SSH | 22 | :84-100, 137-151, 156-161 | settings 下 rag/mcp/ssh 各页 |
| **合计** | **130**（v2 快照；v3 执行后 **115**，见〇节） | | |

联动机制说明（贯穿各域）：
- **流式运行事件**走 Tauri `Channel<AgentEvent>`（随命令入参传入，非全局事件），`finished` 后前端以 `dispatcher_list_messages` 全量对账。
- **全局事件**：`dispatcher-session-updated`（会话标题/创建）、`session-keywords-updated`（关键字）、`sub-agent-event`、`graph-plan-updated`、`graph-run-event`、`python-run-event`、`shell-output`、`browser-frame/-status/-log`、`rag-log`、`architecture-run-request`。
- **删除/清空/截断类命令均不 emit 事件**，前端靠 mutation onSuccess 本地缓存修正 + invalidate 兜底（若未来引入多窗口需补事件）。
- DB 操作统一经 `run_dispatcher_db`（`agent/commands.rs:27`）`spawn_blocking`，例外见 🔧 标注（v2 复核：`agent/commands/*` 30 条中仅 `dispatcher_get_session_token_usage` 一条非 async）。

---

## 二、A 域：会话 / 消息 / Agent 运行（18 条）

三组联动闭环：
1. **运行入口**（3 个 send）→ `begin_run` 占用每会话运行槽位（RAII）→ Channel 流式事件 → 结束后 spawn 标题/关键字生成 → emit 会话事件 → 停止走 `dispatcher_stop_run`。
2. **消息回源**：`dispatcher_list_messages` 是三个 send + clear/truncate 之后前端重建消息视图的统一回源命令。
3. **级联清理**：clear / 删会话共享同一套清理清单——**v2 复核确认实为三处手工双写**（见下）。

### 命令清单

| 命令 | 实现位置 | 功能 | 前端调用 | 联动 |
|---|---|---|---|---|
| 🟢 `dispatcher_send_project_agent_message` | `agent/commands/run_commands.rs:3` | 项目模式 Agent 入口。校验 `project_path`（canonicalize + 受管项目），构建 OrchestratorAgent 执行一轮，返回 `AgentTurn`；结束后生成标题/关键字并 emit `dispatcher-session-updated` / `session-keywords-updated` | `dispatcher-chat/useDispatcherActions.ts:146` | Channel 事件由 `event-channel.ts` 消费；停止走 `dispatcher_stop_run` |
| 🟢 `dispatcher_send_chat_agent_message` | `run_commands.rs:54` | 纯聊天模式入口。无路径参数，Agent 叠加会话所属聊天分类的提示词与工具集（`build_plain_chat_agent`）；其余同 project 版 | `useDispatcherActions.ts:140` | 同上；会话懒创建依赖 `chat_create_session` |
| 🟢 `dispatcher_send_architecture_agent_message` | `agent/commands/architecture_commands.rs:10` | 架构画布视觉 Agent 入口。按视觉模型库条目构建 Agent；**跳过关键字生成**；运行中 `architecture_run` 工具 emit `architecture-run-request` | `architecture/chat/useArchitectureChat.ts:278` | `useArchRunListener` 执行画布程序 → `architecture_run_complete` 回传 |
| 🟢 `architecture_run_complete` | `architecture_commands.rs:56` | 画布执行报告回传桥（≤950 字符），解除 `architecture_run` 工具的 oneshot 等待；返回是否被消费 | `architecture/arch-run-listener.ts:69` | 与上者构成「后端登记 → 前端执行 → 回传」闭环 |
| 🟢 `dispatcher_list_messages` | `agent/commands/message_commands.rs:3` | 拉取会话全部可见消息（visible=1 正序） | `useChatMessages.ts:23,44`；`event-channel.ts:176`（finished 对账） | 消息视图统一回源 |
| 🟢 `dispatcher_get_tool_run_tree` | `message_commands.rs:15` | 按 `tool_call_id` 定位完整工具运行树（递归 CTA 深度优先，`db/tool_runs/tree.rs:113`） | `chat/tool-run-trace.tsx:41`，且被 `:26` `name === "run_tool_program"` 硬门控——**整条命令仅服务这一个工具** | 与 Channel 实时 `toolRunUpdated` 事件互补合并 |
| 🔧 `dispatcher_get_session_token_usage` | `message_commands.rs:29` | 按模型×来源聚合的会话 token 用量记录。**agent 命令中唯一同步命令**（`pub fn`，IPC 线程直查 SQLite） | `useDispatcherSessionTokenUsage.ts:15` | `runUsageUpdated`/`finished` 事件触发刷新 |
| 🟢 `dispatcher_clear_messages` | `message_commands.rs:40` | 清空全部消息及连带产物（单事务），刷新三张会话表 updated_at | `chat-page-v2.tsx:343`（清空按钮） | 与删会话级联清单几乎一致（差别仅不删会话行） |
| 🟢 `dispatcher_truncate_messages_from` | `message_commands.rs:52` | 删除指定消息及其后所有消息，连带回收产物/运行/trace/图计划；**有意保留** token 用量、关键字、图片文件（重发复用，`cleanup.rs:77-95` 注释详尽） | `chat-page-v2.tsx:173`（编辑重发）、`:286`（重新生成） | 截断后立即 send 重发；图计划被删触发关闭画布面板 |
| 🟢 `dispatcher_get_tool_artifact` | `message_commands.rs:65` | 按 artifact_id+workspace_id 查单个工具产物全文 | `artifact/artifact-panel.tsx:65` | 列表引用随 Channel 消息事件下发，本命令补全文 |
| 🟢 `chat_list_sessions` | `agent/commands/session_commands.rs:5` | 聊天会话 keyset 游标分页（`{updated_at,id}` JSON 游标 + id 决胜排序）；未指定 category 时排除内部分类 `arch-design` | `use-chat-queries.ts:52,69` | 事件合并 `useSessionListEventMerge` |
| 🟡 `chat_create_session`（可合并，收益中等） | `session_commands.rs:20` | 创建聊天会话（单事务双 INSERT 子表+统一表），emit `dispatcher-session-updated` | `use-chat-queries.ts:92`；`useArchitectureChat.ts:132`（arch-design 懒创建） | 会话 id 即后续所有命令的 workspaceId |
| 🟡 `chat_delete_session`（可合并，**推荐优先**） | `session_commands.rs:36` | 删除聊天会话：kind 校验 + 单事务级联清理全表 + 图片目录 + `forget_session` 内存台账 | `use-chat-queries.ts:111`；`useArchitectureChat.ts:325` | 与 `project_delete_session` 逐行镜像 |
| 🟢 `chat_set_session_category_v6` | `session_commands.rs:53` | 修改聊天会话分类（双表 UPDATE） | `use-chat-queries.ts:198` | 分类变更影响下次列表分组 |
| 🟢 `project_list_sessions` | `session_commands.rs:68` | 项目会话 offset 分页（与 chat 的 keyset 协议不同） | `use-session-queries.ts:299` → `SessionPanel.tsx:103` | 同事件合并机制 |
| 🟡 `project_create_session`（可合并） | `session_commands.rs:84` | 创建项目会话（同双 INSERT 模板），emit 事件 | `use-session-queries.ts:322` | 与 chat_create 仅子表/kind/category 缺省不同 |
| 🟡 `project_delete_session`（可合并） | `session_commands.rs:100` | 删除项目会话（与 chat 删除逐行镜像，kind 校验值不同） | `use-session-queries.ts:352` | 同上 |
| 🟢 `session_search_keywords` | `session_commands.rs:118` | 跨 chat/project 会话搜索（kind 参数化；标题打分 + 关键词加权；排除 arch-design） | `use-session-queries.ts:279` | 数据来自 send 命令副产品 `session_keywords` |

### 域内分析（v2 复核后）

**级联清理的真实形态——三处手工双写 + 两套机制拼接**（v1 只记录了两处）：

| 资源 | clear_messages<br>`db/messages/cleanup.rs:4-75` | delete_chat_session<br>`db/sessions.rs:373-441` | delete_project_session<br>`db/sessions.rs:577-645` | truncate<br>`cleanup.rs:96-216` |
|---|---|---|---|---|
| tool_artifacts / tool_runs / sub_agent_run_traces / graph_plans | 显式 DELETE | 显式 DELETE（逐条一致） | 显式 DELETE（逐条一致） | 显式 DELETE（按消息/时间范围） |
| graph_node_runs | 显式 DELETE | 显式 DELETE | 显式 DELETE | **FK 级联**（两种策略混用） |
| token_usage / session_keywords | 删 | 删 | 删 | **有意保留**（语义不同，非遗漏） |
| chat_images 行 + 图片文件 | 删 + 目录回收 | 删 + 目录回收 | 删 + 目录回收 | 行靠 FK 级联，**文件有意保留** |
| python_code_runs | FK 级联 | FK 级联 | FK 级联 | FK 级联 |
| command_history 内存台账 | **不清理（不一致点）** | 命令层 `forget_session` | 命令层 `forget_session` | 不清理 |

- 🟡 **`chat_delete_session` + `project_delete_session` → `session_delete(workspace_id)`**（推荐优先做）：级联清单、图片回收、forget_session 完全一致，仅 kind 校验值与子表不同；统一表 `dispatcher_sessions` 本就带 kind 列，命令内查 kind 分流即可。前端 3 个 invoke 点。风险低。
- 🟡 **`chat_create_session` + `project_create_session` → `session_create(kind, …)`**：模板重复；`ChatSession` 与 `ProjectSession` 仅差一个字段（`category` vs `projectId`，`types/chat.ts:364-382`），前端 mutation 缓存逻辑（扁平数组 vs InfiniteData）可原样保留、只换 invoke 名。顺手消掉 category 默认值 `"tech"` 的前后端双写（后端 `db/sessions.rs:337` + 前端 `use-chat-queries.ts:94`）。
- 🟢 **`chat_list_sessions` vs `project_list_sessions` 不合并**：分页协议（keyset vs offset）、数据源表、arch-design 排除逻辑均有实质差异。
- 🟢 **`dispatcher_send_*` 三胞胎不合并命令**，但 v2 复核量化了**抽共享骨架**的收益：三条命令各约 48 行中 35-40 行是同一序列（segments 存档 → 构建 agent → begin_run → title/keywords guard → `run_agent_turn` → finish → spawn 元数据生成），仅「构建 agent / kind / workspace_path / 是否生成关键字」四处差异。抽 `run_agent_turn_skeleton(...)` 后三个命令退化为差异声明（见 9.4-8）。
- 🟢 **`architecture_run_complete` 桥无泄漏**：三个出口（complete/20s 超时/取消）都清 oneshot 槽，`begin()` 的 `retain(!is_closed)` 被动回收被 abort 的死条目，run_id 为一次性 UUID。瑕疵：不带 workspace_id 范围校验，与 `dispatcher_get_tool_artifact` 等命令的域校验风格不一致（单窗口应用无实害，登记为债）。

**v2 新发现的域内问题**（v1 未记录，均登记入设计债务）：
1. `useChatSessionsQuery` 默认聊天列表 `pageSize: 100` 且**忽略 hasMore**（`use-chat-queries.ts:52-58`）——超 100 条会话被静默截断，而后端 keyset 分页与分类侧 infinite query 都是完备的。
2. `project_list_sessions` 排序无 id 决胜列（`db/sessions.rs:495`），等时间戳下分页不稳定；chat 侧为此修过并有专项测试（`db/sessions.rs:726-755`），project 侧无对等保障。前端还按已加载条数重算 offset（`use-session-queries.ts:310-313`），翻页间隙遇并发删除会跳/重。
3. `dispatcher_clear_messages` 不调 `forget_session`，与删除命令对「会话资源清理规范」（`command_history.rs:167`）的执行不一致——清空后内存命令台账残留。
4. `session_keywords` 有 FK ON DELETE CASCADE（`schema.rs:479`）又被显式 DELETE，冗余无害但说明清单是拼凑的。
5. `chat_set_session_category_v6` 的 `_v6` 为 schema v6 迁移期后缀滞留（commit 311b65a），全库唯一残留；仅改名收益。

---

## 三、B 域：聊天分类 / 应用设置 / 模型（12 条）

结构特征：薄命令层 + 厚 DB/State 层；`aha_save_settings_v2` 是 AppSettingsDialog 全部页签的唯一落库通道（前端 400ms debounce 自动保存）。

### 命令清单

| 命令 | 实现位置 | 功能 | 前端调用 | 联动 |
|---|---|---|---|---|
| 🟢 `chat_list_categories` | `agent/commands/category_commands.rs:3`（DB `db/categories.rs:41`） | 列出分类 + LEFT JOIN 聚合 session_count | `use-chat-queries.ts:129` → sidebar 分类列表 | CRUD 后 invalidate 同键 |
| 🟢 `chat_create_category` | `category_commands.rs:11` | 创建分类（同事务写默认智能体配置行） | `use-chat-queries.ts:137` → `ChatNewCategoryDialog` | 可带 allowed_tools/prompt 的创建期种子 |
| 🟢 `chat_update_category` | `category_commands.rs:33` | 只更新 name/icon/color（**不允许**改 tools/prompt） | `use-chat-queries.ts:160` | 行为字段归 aha_save_chat_category_agent_configs |
| 🟢 `chat_delete_category` | `category_commands.rs:53` | 事务内把两会话表的该分类置空（降级未分类）再删行；配置行靠 FK 级联 | `use-chat-queries.ts:182` | 依赖 `PRAGMA foreign_keys=ON` |
| 🟢 `aha_get_chat_category_agent_configs` | `category_commands.rs:65` | 读分类智能体配置（含 backfill/坏行自愈） | `settings/use-aha-settings.ts:129,262` | 运行时经 state 直接消费同一份配置 |
| 🟢 `aha_save_chat_category_agent_configs` | `category_commands.rs:77` | 全量保存配置数组（事务自愈 + 返回全量供回填） | `use-aha-settings.ts:180`（并联批量保存） | debounce 400ms 自动保存管线 |
| 🟢 `aha_get_settings_v2` | `agent/commands/settings_commands.rs:3`（DB `db/settings.rs:372`） | 读聚合设置 AhaSettingsV2，按 model_library 回填凭据（落库只存引用） | `App.tsx:53`（主题校准）；`use-aha-settings.ts:128`；`use-chat-queries.ts:218` | 后端运行时大量直接调 db 层，命令只是前端入口 |
| 🟢 `aha_save_settings_v2` | `settings_commands.rs:13` | 规范化后保存（active 唯一化、剥库引用凭据、theme 收敛），返回规范化结果 | `use-aha-settings.ts:179`；`use-chat-queries.ts:231` | ⚠️ 与 `useBindChatModel` 双写路径（见分析，v2 升级严重度） |
| 🟢 `aha_list_agent_tools` | `settings_commands.rs:24`（State `state/mod.rs:294`） | 按 AgentContext 现场构建 ToolRegistry 枚举（chat = plain_chat + 子智能体工具；project = orchestrator **retain 仅 4 工具**，`tools/mod.rs:26`）；MCP 动态工具不进清单 | `app-settings/aha/tools-tab.tsx:60`；`ChatNewCategoryDialog.tsx:56` | 与 ToolsTab 内 `mcp_global_status` 配合展示两折叠区 |
| 🟢 `dispatcher_stop_run` | `settings_commands.rs:34`（State `state/run.rs:121`） | 向活动 run 的 watch channel 发取消信号；随后无论结果关闭该会话浏览器 | `chat-page-v2.tsx:260`；`useArchitectureChat.ts:311` | 🔧 归组在 settings_commands 属代码组织问题，宜移 run_commands |
| 🟢 `dispatcher_fetch_models` | `agent/commands/model_commands.rs:3`（`llm/models.rs:8`） | GET /models 目录列举，兼容 OpenAI/DashScope（≤30 页）/纯数组，排序去重 | `settings/providers/ModelEntryCard.tsx:86` | 结果经 commitField 进入条目，随 aha_save_settings_v2 落库 |
| 🟢 `dispatcher_test_model` | `model_commands.rs:13` | 按 kind 真实连通性测试（chat/summary 流式 pong；vision 64px PNG 多模态；embedding 维度；asr 仅字段校验——刻意设计） | `ModelEntryCard.tsx:306` | 错误链串成单条中文信息直接展示 |

### 域内分析（v2 复核后）

- 🟢 **`chat_update_category` 的不对称（create 带 agent 参数、update 不带）验证为有意设计且是物理分离**：分类元数据（`chat_categories`）与 agent 配置（`chat_category_agent_configs`）两张表，create 的事务同时 INSERT 两表保证种子一致，此后行为字段只能走 configs 通道。前端也按此分工。
- 🔴→🟢 **`aha_list_agent_tools` + `sub_agent_list_tools` 合并建议撤销**（v1 列为低优先级可合并，v2 复核判定**不应合并**）：两者数据源有门禁语义差异——`sub_agent_list_tools` 返回的 plain_chat 工具集是 create/update `validate_allowed_tools` 校验集的**唯一同源**（`sub_agent/commands.rs:20-31` 注释明说不能取并集）；`aha_list_agent_tools("chat")` 会混入 `list_sub_agents/call_sub_agent`（`state/mod.rs:319-322`），若 UI 勾选它们保存必被拒，放行则出现设计明确防范的「保存成功但运行时静默缺工具」。合并只能新增第三条独立分支 = 仅仅换名，还给 `AgentContext`（承载会话语义）塞进非会话值。收益 < 成本。
- 🔧 **`useBindChatModel` 陈旧覆盖——v2 升级严重度：不限 400ms 窗口**。`use-chat-queries.ts:223-237` 是 get→改→save 整份写回；而设置 store 是模块级单例，`ensureLoaded` 首次加载后**永不重读**（`use-aha-settings.ts:124-148`，revision 守卫只检测本地编辑）。因此只要设置 store 已加载，任意时刻在聊天框绑定模型后，用户下一次在设置弹窗改任意字段触发保存，都会把绑定**静默回滚**——不限于 v1 记录的 400ms 并发窗口。修复属前端改造：让 `useBindChatModel` 走同一 store（store 上加 `bindChatModel(entry)` 动作）或 save 前强制重读合并。
- 🔧 **三命令并联保存非事务**（`use-aha-settings.ts:178-184`）：`aha_save_settings_v2` + `aha_save_chat_category_agent_configs` + `sub_agent_set_global_enabled` 任一失败（典型：弹窗打开期间别处删了子智能体，存在性校验 `sub_agent/commands.rs:190-192` 报错）则整体 toast「保存失败」，但其余两个写入已落库；且此后每次 autosave 都重复失败。建议：失败时按命令拆分提示，`sub_agent_set_global_enabled` 对「id 已不存在」改为过滤而非报错（删除语义下静默收敛更合理）。
- 🔧 `ToolCatalog`（`state/tool_catalog.rs`）自 G11-07 改为「读取即重建」后成为**直写式死缓存**——唯一读者是写入者自身。v1 文档「读预初始化静态目录」的描述已过时，应清理该结构或恢复真正的缓存语义。
- 🔧 `aha_list_agent_tools("project")` 的 4 工具隐性裁剪（`ORCHESTRATOR_RUNTIME_TOOL_NAMES` retain）在命令边界不可见，消费方无从得知清单被裁剪；建议返回体附带裁剪说明或文档化。
- 🟢 `dispatcher_fetch_models`（目录列举）vs `dispatcher_test_model`（语义连通性验证）职责正交，不合并。
- 🟢 get/save 成对（settings、category configs）均为刻意设计：save 返回规范化全量供前端回填，勿拆 per-field 命令。

---

## 四、C 域：子智能体 / 图编排（17 条）

子智能体命令走 `spawn_blocking` + SubAgentManager；图命令经 GraphStore async 封装。

### 命令清单

| 命令 | 实现位置 | 功能 | 前端调用 | 联动 |
|---|---|---|---|---|
| 🟢 `sub_agent_list` | `agent/sub_agent/commands.rs:39` | 全量查 `sub_agents` 表 | `SubAgentManagePanel.tsx:80`；`sub-agent-picker.tsx:20` | 各写命令成功后前端回源刷新 |
| 🟢 `sub_agent_create` | `commands.rs:55` | 校验 allowed_tools 合法性后落库 + 刷缓存 | `SubAgentManagePanel.tsx:98` | 校验集与 `sub_agent_list_tools` 必须同源 |
| 🟢 `sub_agent_update` | `commands.rs:84` | 同 create 校验；强制 config.agent_id 与路径 id 一致（禁改名） | `SubAgentManagePanel.tsx:100` | id 一致性保障缓存与 DB 不分叉 |
| 🟢 `sub_agent_delete` | `commands.rs:126` | 事务删 global_sub_agents 关联 + 本行，移出缓存 | `SubAgentManagePanel.tsx:116` | 影响会话可用子智能体集合 |
| 🟢 `sub_agent_seed_browser` | `commands.rs:142` | INSERT OR REPLACE 强制重建内置「浏览器助手」（恢复/重置入口） | `SubAgentManagePanel.tsx:139` | 与 schema 启动自动补种互补（后者不覆盖已改坏配置） |
| 🟢 `sub_agent_list_tools` | `commands.rs:158` | 读 plain-chat 工具集（读取即重建，见 B 域 ToolCatalog 分析） | `SubAgentEditorDialog.tsx:75`（**唯一**消费方；失败仅 console 降级，无 UI 线索） | 是 create/update 校验集的 UI 镜像，**不与 aha_list_agent_tools 合并**（见 B 域） |
| 🟢 `sub_agent_set_global_enabled` | `commands.rs:174` | 整份替换全局启用集（先校验存在性对抗 INSERT OR IGNORE 静默丢 id） | `use-aha-settings.ts:183`（并联保存） | 与分类级关联取并集决定会话可见集合 |
| 🟡 `sub_agent_get_global_enabled`（可折叠，低优先级） | `commands.rs:206` | 查全局启用代理（`global_sub_agents` 表 ∩ 自身 enabled） | `use-aha-settings.ts:130`（ensureLoaded，唯一消费方） | 折叠进 `sub_agent_list`（LEFT JOIN 输出 `globalEnabled` 字段）低风险；须保真 `enabled=1 ∩ 全局成员` 交集语义 |
| 🟢 `sub_agent_get_run_trace` | `commands.rs:224` | 按 (workspace, tool_call_id) 查持久化执行轨迹（历史回放数据源） | `chat/chat-shell.tsx:249` | 与实时 `sub-agent-event` 流互补 |
| 🟢 `graph_plan_get` | `agent/graph/commands.rs:19` | 按 plan_id 读计划（含最新 run 的 node_runs），面板回放唯一读路径 | `graph/graph-store.ts:318` | `graph-plan-updated` 事件触发回源 |
| 🟢 `graph_plan_latest_for_session` | `commands.rs:33` | 按会话取最近计划（Option 语义，会话可无计划） | `chat-page-v2/useGraphPanelController.ts:26` | 服务端 `graph_submit.rs:119` 创建计划后广播事件驱动 |
| 🟢 `graph_plan_update` | `commands.rs:46` | 仅 draft 态可编辑；normalize + harness 校验 + store 层条件更新（`WHERE status='draft'` + 影响行数）防与 run_start 竞态；广播事件 | `GraphNodeDrawer.tsx:123` | 校验目录与 harness catalog 同源（`catalog_for_workspace`） |
| 🟢 `graph_harness_catalog_get` | `commands.rs:272` | 图节点运行目录：模型可选 + aha/PI 扩展工具 + 诊断（MCP scope canonicalize 对齐缓存键） | `GraphNodeDrawer.tsx:72`（draft 态） | 保证编辑器可选值与运行期一致 |
| 🟢 `graph_run_get` | `commands.rs:280` | 按 run_id 读历史运行详情（run + 全部 node_runs + activities） | `GraphNodeDrawer.tsx:65` | plan 记录只含最新 run，历史 attempt 必须经此获取 |
| 🟢 `graph_run_start` | `commands.rs:109` | 确认执行/断点续跑（mode=full/resume，未知 mode 显式拒绝）；每计划唯一运行槽位防重入；catch_unwind 兜底；持续广播 `graph-run-event` | `GraphPanel.tsx:194` | 前端把事件流折叠进内存快照（100ms 节流），终态回源 |
| 🟢 `graph_run_cancel` | `commands.rs:230` | 发取消信号（PI sidecar 先 abort 超时杀进程组）；重启残留的 running 直接复位 | `GraphPanel.tsx:228` | 逐节点 nodeCancelled → runCancelled；🔧 残留自愈分支 `let _ =` 吞 DB 错误（:241-243） |
| 🟢 `graph_run_resume` | `commands.rs:253` | 恢复「高危写检查点」暂停中的**活运行**（mpsc 信号，不触 DB 运行记录；带 cancel 已置位拒绝 + 容量 1 去重双防护） | `GraphPanel.tsx:211` | 与 runPaused/runResumed 事件配对 |

### 域内分析（v2 复核后）
- 🟢 `graph_run_start(mode="resume")` vs `graph_run_resume` **极易混淆但完全不重叠，禁合并**：前者从 DB checkpoint **新建 attempt**（复制成功节点为 cached、继承共享 state），入口前提是计划已处终态 failed/cancelled；后者向**仍在内存运行**的执行发 mpsc 信号解除检查点暂停。两者前置状态互斥，合并不可能不破坏其中一个状态机。
- 🟢 `graph_plan_get` vs `graph_plan_latest_for_session`：查询维度与错误语义不同（缺失报错 vs Option 正常态）。
- 🟢 `graph_run_get` vs `graph_plan_get`：当前快照 vs 任意历史运行详情，互补非重复。
- 🟡 `sub_agent_get_global_enabled` 折叠进 `sub_agent_list` 可行（v2 确认唯一消费方、LEFT JOIN 改造低风险），但需改 `SubAgentRecord` 契约，属低优先级顺手项。
- 新文件 `sub-agent-model-picker.ts` 为纯函数层（从 modelLibrary 挑选 text/vision 条目回填 modelConfig），不涉命令，无合并影响。

---

## 五、D 域：Git 集成 / 项目管理（20 条）

Git 域共享底座 `scm/git/exec.rs`（`run_git` 系列走 `spawn_blocking` + 引用名白名单校验）。项目管理是「前端单一状态持有者 + 整表重写」注册表模式，删除被刻意强制走 `project_delete`。

### 命令清单

| 命令 | 实现位置 | 功能 | 前端调用 | 联动 |
|---|---|---|---|---|
| 🟢 `generate_commit_message` | `scm/git/commit_message.rs:20` | AI 生成提交信息：读 staged diff（截 50k 字符）+ 项目 `[git].commit_prompt`，LLM 15s 超时生成 | `GitChanges.tsx:104` | 结果只填可编辑 textarea（human-in-the-loop），由 `git_commit` 提交 |
| 🟢 `git_status` | `scm/git/queries.rs:19` | porcelain v1 解析为 staged/unstaged/untracked 三态列表 | `GitChanges.tsx:47` | 本域「状态中枢」，全部 mutation 后 refresh 回源 |
| 🟢 `git_list_branches` | `queries.rs:82` | `git branch -a` 解析本地/远端/当前分支 | `GitHistory.tsx:96`；`task-panel/BranchBar.tsx:232`（10s 轮询） | 驱动 checkout/push/log 的 branch 参数 |
| 🟢 `git_create_branch` | `mutations.rs:49` | `git checkout -b`（引用名校验） | `BranchBar.tsx:59` | 成功后刷新分支列表 |
| 🟢 `git_checkout_branch` | `mutations.rs:12` | 切换分支；远端分支自动 `--track` 建本地分支 | `BranchBar.tsx:278` | 影响后续 status/log 结果 |
| 🟢 `git_log` | `queries.rs:124` | 自定义格式 log（limit/grep/分支过滤） | `GitHistory.tsx:116` | 与 `git_remote_counts` 并行刷新 |
| 🟢 `git_commit_detail` | `queries.rs:212` | try_join! 并行 3 条 git 命令合并提交元信息+文件增删统计 | `GitHistory.tsx:168` | 详情文件点击开 `git_diff` commit-file 模式 |
| 🟢 `git_diff` | `scm/git/diffs.rs:30` | 统一 diff 命令，mode 三分（GitDiffMode 枚举）：commit / commit-file / file（含未跟踪回退 no-index；字节级截断） | `GitDiffViewer.tsx:175` | **域内三合一合并的成功先例** |
| 🟡 `git_stage`（可合并） | `mutations.rs:84` | `git add -- <file>` | `GitChanges.tsx:72` | 成功后 refresh → git_status |
| 🟡 `git_unstage`（可合并） | `mutations.rs:96` | `git restore --staged -- <file>` | `GitChanges.tsx:70` | 同上 |
| 🟡 `git_stage_all`（可合并） | `mutations.rs:108` | `git add -A` | `GitChanges.tsx:83` | 同上 |
| 🟡 `git_unstage_all`（可合并） | `mutations.rs:120` | `git restore --staged .` | `GitChanges.tsx:93` | 同上 |
| 🟢 `git_commit` | `mutations.rs:132` | `git commit -m` | `GitChanges.tsx:124` | 提交后清输入框 + refresh |
| 🟢 `git_push` | `mutations.rs:144` | `git push [origin <branch>]`（可选分支，支持推非当前分支） | `GitHistory.tsx:200`（selectedBranch 为历史视图筛选下拉，可推未 checkout 分支） | 成功后刷新 log/counts/branches |
| 🟢 `git_pull` | `mutations.rs:173` | 裸 `git pull`（无 branch 参数） | `GitHistory.tsx:187`（只传 projectPath） | **push/pull 不对称 v2 验证为合理**：`git pull origin B` 会把 B 合并进当前分支，语义危险；pull 只拉当前分支上游是刻意收窄 |
| 🟢 `git_remote_counts` | `queries.rs:342` | rev-list left-right 解析 ahead/behind（上游缺失容错返回 0,0） | `GitHistory.tsx:122` | 驱动推拉按钮徽标 |
| 🔧 `init_project_config` | `project/config.rs:79` | 确保项目 `.jkcodingagent/` + `mcp.json` + `config.toml` 模板（幂等不覆盖） | `App.tsx:94,108`（**每次打开/切换项目都调用**） | commit_prompt 被 generate_commit_message 消费；同步命令做多处 fs（create_dir_all + atomic_write + 读回解析），应 async 化 |
| 🟢 `load_projects` | `project/storage.rs:69` | projects 表全量读 | `App.tsx:69` | 注册表模式读端 |
| 🟢 `save_projects` | `project/storage.rs:78` | IMMEDIATE 事务整表重写；**载荷缺失现存 id 直接报错**（把删除强制导向 project_delete 防孤儿） | `App.tsx:35` | 注册表模式写端（排序/原子性由事务保证） |
| 🟢 `project_delete` | `project/storage.rs:101` | 单事务级联删除项目全部会话及产物/图片记录 + projects 行；提交后清理图片目录与仓库内运行目录（保留 config.toml/mcp.json）；返回 deletedSessionIds 供前端清内存 store | `App.tsx:147` | 前端据返回值清理 3 个模块级 store |

### 域内分析（v2 复核后）
- 🟡 **`git_stage`/`git_unstage`/`git_stage_all`/`git_unstage_all` → `git_stage(project_path, files: Option<Vec<String>>, unstage: bool)` 4→1**：四条命令是完全对称的单行 git 调用（仅参数不同），合计 ~50 行样板；`files=None` 即全量（`add -A` / `restore --staged .`），非空走 pathspec。与 `git_diff` 三合一（mode 枚举 + Option 参数）先例完全同型。前端仅 GitChanges.tsx 3 个 handler 4 处 invoke。风险低，**推荐 Tier 1**。
- 🟢 **`generate_commit_message` 不并入 `git_commit`**：资源画像不同（LLM 网络 vs 纯子进程），且 UI 是人工审校环节，合并会让模型失败连坐提交。
- 🟢 **`load/save_projects` 不拆单条 CRUD**：整表重写 + 防孤儿守卫是有意设计（有测试覆盖），拆散反而要每条命令重复防误删逻辑。
- 🔴 **`Project.branch` 死列（字段级）——v2 升级确定性**：前端任何路径都不写入（App.tsx 新建/透传均无 branch），唯一消费点 WelcomePage.tsx:224-231 的分支 pill 因此**恒显示兜底文案「本地」**，DB 列 `branch TEXT`（`db/projects.rs:19`）仅测试写过。属「已渲染但恒为死值」而非 v1 记录的「消费点条件永不成立」。删除需 schema 迁移，涉及 DDL/SELECT/INSERT/Rust struct/TS interface/pill 共 6 处；建议与下次 schema 变更同行，单独标记待清理。
- 🔧 `write_file_content` 非原子写且无大小上限（`fs.rs:314-321`），与库内已有 `project::storage::atomic_write` 风格不一致（详见 E 域）。

---

## 六、E 域：文件工作区 / Rope 编辑 / 聊天图片 / 通知（20 条）

文件查看双路径由 `get_file_meta` 分流：<2MB 文本 → `read_file_content` + Monaco 全量读写；≥2MB → rope 会话虚拟化编辑。聊天图片统一 `chat-image://{id}` 协议寻址。**通知域 v2 判定为整体死功能（见域内分析首位）。**

### 命令清单

| 命令 | 实现位置 | 功能 | 前端调用 | 联动 |
|---|---|---|---|---|
| 🟢 `read_dir_entries` | `workspace/fs.rs:173` | 目录一级条目（路径校验、过滤 IGNORED_DIRS、目录优先排序） | `file-explorer/useFileExplorerTree.ts:39`（8 并发限流递归） | 文件树唯一数据源 |
| 🟢 `read_file_content` | `fs.rs:221` | 读整个文本文件（>2MB 报错） | `file-viewer/FileTabPane.tsx:329` | 被 get_file_meta 门控分流 |
| 🟡 `read_image_preview`（可协议化替代，可选） | `fs.rs:253` | 工作区图片 → base64 data URL（扩展名白名单、10MB 上限） | `FileTabPane.tsx:81` | 与 chat-image:// 分属两信任域（此处须 validate_path_within，支持 bmp/svg）；10MB 图 base64 后 ~13.3MB 字符串走 JSON IPC 偏重，若注册放行项目根的自定义协议可删此命令（中等成本，非必须） |
| 🟡 `write_file_content`（与 rope_save 不可互替，自身保留） | `fs.rs:302` | 全量覆写小文件（900ms 防抖 + 前端串行队列防乱序） | `FileTabPane.tsx:277` | 与 rope_save 服务互斥尺寸区间；🔧 应改原子写（复用 atomic_write）并加尺寸上限 |
| 🟢 `move_fs_entry` | `fs.rs:323` | 移动/重命名（禁改项目根、目标存在报错、目标校验父目录） | `FileExplorer.tsx:162` | 前置确认已打开标签页未保存内容 |
| 🟢 `delete_fs_entry` | `fs.rs:372` | 删除条目（symlink_metadata 不跟随链接；目录递归） | `FileExplorer.tsx:131` | 同上 |
| 🟢 `get_file_meta` | `fs.rs:408` | {sizeBytes, lineCount, isText}，前端渲染路径分发器 | `FileTabPane.tsx:317` | 双路径（write vs rope）的枢纽 |
| 🟢 `rope_open` | `workspace/rope.rs:169` | 打开大文件会话（幂等；同路径复用） | `useLargeFileViewport.ts:49` | 与 rope_close 严格配对 |
| 🟢 `rope_read_lines` | `rope.rs:225` | 按行区间读取（虚拟滚动按需加载） | `useLargeFileViewport.ts:101`；`useLargeFileSelection.ts` | 与 rope_edit 的 affected 行范围配合做缓存失效 |
| 🟢 `rope_edit` | `rope.rs:263` | 字符偏移 splice 结构编辑（undo 快照 + revision）；**全钳位语义**（line/col/end 越界一律钳制） | `useLargeFileEditing.ts:152,178,195`；`useLargeFileSelection.ts:95` | 结构操作（Enter/并线/选区替换）位置天然易越界，宽容是刻意设计 |
| 🟡 `rope_replace_line`（理论可并入 rope_edit，**不建议**） | `rope.rs:332` | 整行替换：**越界硬报错** + 换行保护（删除区间收缩保留终止符）+ 同内容 no-op | `useLargeFileEditing.ts:63,110`（150ms 防抖打字热路径） | 严格/快速失败语义正是防打字路径把脏行号写坏文件的护栏，与 edit 的宽容语义刻意相反 |
| 🟢 `rope_save` | `rope.rs:412` | 会话落盘（复位 saved_revision/dirty） | `useLargeFileEditing.ts:252`（Cmd+S） | 若用 write_file_content 替代会话 dirty 永久失真 |
| 🟢 `rope_close` | `rope.rs:455` | 移除会话释放内存（不落盘） | `useLargeFileViewport.ts:60`（effect cleanup） | 删除会内存按标签页泄漏，必须保留 |
| 🟢 `rope_undo` | `rope.rs:460` | 弹 undo 栈顶整份 Rope 快照恢复（栈深 10，ropey 结构共享） | `useLargeFileEditing.ts:219`（Cmd+Z） | 快照栈语义无法用「再编辑一次」在服务端复现 |
| 🟢 `rope_redo` | `rope.rs:492` | 逆操作 | 同上（Cmd+Shift+Z，同一调用点三元切换） | 与 undo 合并收益趋零 |
| 🟢 `save_chat_image` | `chat_images.rs:369` | 全应用唯一图片落盘入口：base64 → id → >1.5MB 压缩 → 写会话目录 + DB 登记 | `chat-page-v2.tsx:225`；`useArchitectureChat.ts:174`；`arch-executor.ts:257` | 产物以 `chat-image://` 经自定义协议直出；工具产图共用 save_image |
| 🟢 `chat_images_validate` | `chat_images.rs:419` | 发送前校验 segments 引用的图片文件存在性 | `chat-page-v2.tsx:172,285`（先验后截断） | 与 run_loop 内二次校验构成纵深防御 |
| 🔴 `get_notifications`（**整域删除**） | `platform/notification.rs:277` | 读通知——远程拉取被硬编码禁用（`_NOTIFICATIONS_URL=""`，`fetch_remote()` 恒返回空，:270-273），items 恒空 | `NotificationBell.tsx:92`（唯一渲染结果是「暂无通知」空态） | 返回字段 `hasUnreadPopup`/`popup` 前端类型都未声明（死载荷） |
| 🔴 `mark_notification_read`（**整域删除**） | `notification.rs:342` | 单条标记已读——对恒空缓存做本地 JSON 变更，永远无效 | `NotificationBell.tsx:116` | 域死功能 |
| 🔴 `mark_all_notifications_read`（**整域删除**） | `notification.rs:363` | 全部标记已读——同上，实际 no-op | `NotificationBell.tsx:134` | 域死功能 |

### 域内分析（v2 复核后）

- 🔴 **通知域整体删除——本次复核最大的简化项**。v1 只建议合并两条 mark 命令；v2 下钻确认整条数据链是死的：`_NOTIFICATIONS_URL = ""` + `fetch_remote()` 硬编码返回空（"Remote notifications disabled for internal deployment"），因此 `get_notifications` 恒空、两条 mark 恒 no-op，NotificationBell 在现网唯一能渲染的是「暂无通知」空态。整套域还拖着 ~120 行无用配套（semver 比较、过期/版本过滤、sanitize、互斥锁 + spawn_blocking 存储层），**无任何 Rust 测试**。删除清单：`notification.rs`（386 行）+ 3 条命令注册（app/mod.rs:163-165）+ `NotificationBell.tsx`（231 行）+ `SidebarFooterActions.tsx` 挂载点 + `types/infrastructure.ts:163-179` 类型 + tailwind.css 27 处 `.ai-notification-*` 规则，合计 **650+ 行净删除、3 条命令、零功能损失**。依赖检查：`atomic_write` 另有 3 个消费者（agent/config.rs、mcp/project_file.rs、ssh_tool/memo.rs），不会孤儿化。
- 🟡 **`rope_replace_line` 不并入 `rope_edit`**（v2 将 v1 的「不建议优先」升级为「不建议」）：两者越界语义刻意相反（replace_line 硬报错防打字热路径写坏文件；edit 钳位容忍结构操作的自然越界），合并必须统一语义，等于拆掉其中一条护栏。前端 lineCache 挂起期间传错字符数会连换行一起删，是 bug 源。
- 🟢 `rope_undo/redo` 不可并入 `rope_edit`（服务端快照栈是虚拟化设计的前提）；二者互相合并收益趋零。
- 🟢 `rope_save` vs `write_file_content` **不可互替**：尺寸区间互斥 + 会话 dirty 状态耦合。
- 🔧 **三套图片服务机制并存**：chat-image:// 协议（仅 chat-images 目录）、asset://（scope 同样仅放行该目录）、read_image_preview（base64 IPC，服务项目工作区图片）。前两者同域冗余但无害；第三者信任域不同不可直接复用，可选优化为「注册放行当前项目根的自定义协议 + 前端 convertFileSrc」以删命令、免 13MB JSON IPC（中等成本，Tier 3）。
- 🔧 `get_file_meta` 行数公式与注释描述不符（实现 `size>0 则 +1` 恰与 ropey `len_lines()` 一致，注释却说 "if file doesn't end with newline"），无害但误导。

---

## 七、F 域：内嵌浏览器 / Python 运行器 / PTY Shell（21 条）

浏览器为每会话一个 Node sidecar（JSON 行协议），`manager.command()` 在会话不存在时**隐式启动**（这是导航类命令必须携带 project_path 的原因）；Python 运行器是「落库 + watch 通道 + 事件流」模型；PTY 输出经 16ms/64KB 批量合并推 `shell-output`。

### 命令清单

| 命令 | 实现位置 | 功能 | 前端调用 | 联动 |
|---|---|---|---|---|
| 🟢 `browser_start` | `browser/commands.rs:12` | 启动 sidecar（幂等；读全局浏览器配置 + 登录态 profile；失败回滚 kill） | `BrowserPanel.tsx:116,205` | 启动后 `browser-frame/-status/-log` 事件流；🔧 签名 `String` 落后于同文件惯例（其余 5 命令已是 `Option<project_path>` 回退），改 Option 后可吸收 plain_chat 版 |
| 🔴 `browser_start_plain_chat`（可删除，并入 start） | `commands.rs:22` | = `browser_start` + 固定 plain-chat 工作区路径的一行包装 | `BrowserPanel.tsx:120,209` | 域内 navigate/reload/go_back/click_at/import **5 个命令已用 `Option<project_path>` + 回退模式**，start 改可选参数即可覆盖 |
| 🟢 `browser_import_chrome_profile` | `commands.rs:34` | 导入 Chrome 登录态（stop → spawn_blocking 递归复制 → 下次 start 生效） | `BrowserPanel.tsx:195` | 与 stop+start 编排组合 |
| 🟢 `browser_list_chrome_profile_candidates` | `commands.rs:55` | 扫描本机 Chrome Profile 候选（纯只读） | `BrowserPanel.tsx:148` | 服务 import 的前置 UX（预填确认文案与目录选择器） |
| 🟢 `browser_stop` | `commands.rs:63` | 发 close 请求（10s）→ kill sidecar；EOF 后 reader 发最终 closed 状态 | `BrowserPanel.tsx:128`；`useBrowserSessionDock.ts:96` | closed 事件驱动停靠栏移除 |
| 🟢 `browser_click_at` | `commands.rs:71` | canvas 坐标 → sidecar click（30s） | `BrowserPanel.tsx:298` | 新 frame/status 事件刷新画面 |
| 🟢 `browser_go_back` | `commands.rs:97` | sidecar back（历史后退；隐式启动回退） | `BrowserPanel.tsx:135` | 结果经事件流体现 |
| 🟢 `browser_navigate` | `commands.rs:121` | URL 规范化（补协议头）→ sidecar open_url | `BrowserPanel.tsx:250`；`useBrowserSessionDock.ts:114`（Markdown 链接打开器） | 成功后自动展开面板 |
| 🟡 `browser_reload`（可合并，可选） | `commands.rs:147` | sidecar reload | `BrowserPanel.tsx:264` | 与 navigate/go_back 同为薄转发（normalize → Option 回退 → manager.command），仅 method/参数不同 |
| 🟢 `browser_get_status` | `commands.rs:171` | 纯内存读会话状态（未知会话返回 closed 占位） | `BrowserPanel.tsx:86` | 与事件流互补的拉取兜底 |
| 🟡 `browser_minimize`（可合并） | `commands.rs:179` | 最小化有头窗口 → minimized + emit `browser-status` | `useBrowserSessionDock.ts:83` | 停靠状态机：minimized → 收进 dock |
| 🟡 `browser_restore`（可合并） | `commands.rs:190` | 恢复**仍存在的**最小化窗口 | `useBrowserSessionDock.ts:88` | restore=「最小化→就绪」 |
| 🟡 `browser_reopen`（可合并） | `commands.rs:201` | 重建被用户关闭的窗口（page_closed 场景，15s，解析 headed） | `useBrowserSessionDock.ts:107` | reopen=「窗口销毁→重建」，语义差异须保留 |
| 🟢 `python_runner_list_results` | `python_runner/commands.rs:3` | 查 `python_code_runs` 表（按 workspace，可选 message 过滤） | `usePythonRunController.ts:22` | 与 `python-run-event` 互补（冷启动全量） |
| 🟢 `python_runner_start` | `commands.rs:46` | 落库 running 记录 + 注册 watch 取消通道 → spawn 后台 agent（uv/venv、60s 超时流式运行、失败进教学 agent 循环） | `usePythonRunController.ts:56`（代码块运行按钮） | 持续 emit `python-run-event` |
| 🟢 `python_runner_stop` | `commands.rs:34` | 按 run_id 发取消信号（未知 id 静默幂等） | `usePythonRunController.ts:75` | v2 验证幂等是**必要设计**：自然结束时 sender 已移除（:102），停止按钮与自然完成固有竞态，报错会产生大量伪失败 |
| 🟢 `python_runner_clear_result` | `commands.rs:18` | 按 workspace+message+code_block_index 三元组精确删记录 | `usePythonRunController.ts:84` | 与 list_results 同表读写两端，粒度不同非重叠 |
| 🟢 `send_input` | `task_runtime/pty.rs:148` | 向 PTY writer 写字节（shell 与 agent 任务共用 TaskManager） | `ShellTerminalPanel.tsx:64`（合批） | 与 `shell-output` 构成全双工；🔧 命名见域内分析 |
| 🟢 `resize_pty` | `pty.rs:158` | 调整 PTY cols/rows（未注册 id 静默成功防卸载竞态） | `ShellTerminalPanel.tsx:71,179` | 与 xterm fit 同步 |
| 🟢 `open_shell` | `pty.rs:181` | 幂等创建项目终端（先杀同 id 旧 shell 自带重启语义；reader 线程批量合并 emit） | `ShellTerminalPanel.tsx:77`（每项目单实例） | rAF 帧预算灌入 xterm.js |
| 🟢 `kill_shell` | `pty.rs:247` | kill 子进程并原子移除三类句柄（幂等） | `ShellTerminalPanel.tsx:169` | 与 open_shell 对偶；Drop 兜底全量回收 |

### 域内分析（v2 复核后）
- 🔴 **`browser_start_plain_chat` → 并入 `browser_start`（`project_path: Option<String>`）**：唯一真正「可删除」的包装命令；同文件 5 个命令已是 Option 回退形态，这是补齐惯例而非新模式。影响面 = 1 个 Rust 签名 + 1 行注册 + 前端 2 处三元（BrowserPanel.tsx:115-120、204-209）。agent 侧浏览器工具走 `context.workspace` 不经此命令，无回归面。
- 🟡 **`browser_minimize`/`restore`/`reopen` → `browser_window_action(session_id, action)` 3→1**：三条命令逐字符同构（查会话→manager 方法→emit），manager 层各自的状态改写逻辑不动，命令层分发表零逻辑损失；须按 action 保留超时差异（reopen 15s）与「restore 处理最小化、reopen 处理窗口销毁」语义边界。前端仅 useBrowserSessionDock.ts 3 处。风险低。反论：具名命令即 API 文档——属风格取舍，但同型先例（git_diff/browser Option 回退）已确立项目偏好。
- 🟡 （可选，低优先级）`browser_navigate`/`reload`/`go_back` → 通用 method 转发：仅省样板，navigate 的 URL 规范化是差异逻辑；为保 API 类型安全可不做。
- 🔧 **`resolve_project_path(Option<String>)` 内部 helper**：同一段「trim 空/None → plain-chat 工作区回退」match 在 commands.rs 重复出现 5 次（start 合并后仍有 5 处），抽 helper 顺手消除。
- 🟢 PTY 四命令是生命周期与 IO 的最小完备集合（open/send/resize/kill 恰好一一对应前端 4 个调用点，无 list/status 之需）。🔧 命名债：`send_input`/`resize_pty` 参数叫 `task_id`、`open_shell`/`kill_shell` 叫 `shell_id`，实为同一注册表；且是全局唯一**无命名空间前缀**的命令组（对比 browser_/python_runner_/ssh_tool_…），有撞名风险——下次触碰该域时改 `pty_write/pty_resize/pty_open/pty_kill`。
- 🔧 `python_runner_stop` 不校验 run_id 归属 workspace（本地单用户应用，风险低，登记）。

---

## 八、G 域：RAG / MCP / SSH（22 条）

RAG 为桌面端权威配置 + PyInstaller sidecar（HTTP 传输）；MCP 为「全局 DB 注册表 ∪ 项目 mcp.json」合并视图 + 探活缓存（300s）；SSH 为 russh 连接池 + TOFU 主机密钥 + AI 审查门禁 + 备忘录文件。

### 命令清单

| 命令 | 实现位置 | 功能 | 前端调用 | 联动 |
|---|---|---|---|---|
| 🟢 `rag_restart` | `rag/commands.rs:57` | 原子重启 sidecar（同把 spawn_lock 内 stop+spawn+握手+健康检查，防两次 invoke 间插队产生孤儿进程） | `useRagKbConfig.ts:134` | `rag_save_kb_config` 热更失败的手动兜底 |
| 🟢 `rag_status` | `commands.rs:85` | 纯读运行状态（不启动进程） | `useRagKbConfig.ts:69`（挂载轮询） | 应用启动自动 ensure_started 的观察窗口 |
| 🟢 `rag_get_kb_config` | `commands.rs:94` | 读知识库配置（内存快照优先，回源 DB app_config）。**含 qdrant/embedding api_key 明文回传**（doc 自认不脱敏）——见域内分析安全项 | `useRagKbConfig.ts:52` | 表单 dirty 基准 |
| 🟢 `rag_save_kb_config` | `commands.rs:107` | 写 DB → 更新内存 → sidecar 在运行则热推送 `/config/reload`（返回 reloadError 区分；reload 失败不连坐保存报错） | `useRagKbConfig.ts:100` | 测试/导入前前端也先 persistConfig |
| 🟡 `rag_test_qdrant`（可合并，收益最高） | `commands.rs:155` | **内部先 save_rag_config 落库** → 确保运行 → POST /test/qdrant | `useRagKbConfig.ts:157` | 🔧 内部 save 与前端 persistConfig 双重落库（同一次点击写 DB 两遍）；且 save 无热推送，与 rag_save_kb_config 行为分叉 |
| 🟡 `rag_test_embedding`（可合并） | `commands.rs:189` | 与上者逐行同构，仅端点不同 | `useRagKbConfig.ts:157`（同一 `runTest(target)` 代码路径） | 合并为 `rag_test_connection(target)` 零逻辑损耗，顺带修双重落库 |
| 🟢 `rag_logs_snapshot` | `commands.rs:221` | 内存滚动日志（上限 2000 条）一次性快照 | `RagSidecarLogPanel.tsx:100` | 快照 + `rag-log` 事件订阅 = 初始全量 + 增量 |
| 🟢 `rag_logs_clear` | `commands.rs:227` | 清空内存日志窗口 | `RagSidecarLogPanel.tsx:167` | 与 snapshot 语义不同（取数 vs 破坏性操作），不合并 |
| 🟢 `rag_ingest_files` | `commands.rs:234` | 校验文件（绝对路径 + canonicalize 限项目内 + 必须是文件）→ 启动导入任务返回 jobId；**不做内部落库**（保存责任归调用方——与 test 命令不对称） | `useRagKbConfig.ts:209` | 与 job_status 构成「提交+轮询」异步任务对 |
| 🟢 `rag_ingest_job_status` | `commands.rs:272` | 查导入任务状态（sidecar 未运行直接报错不懒启动） | `useRagKbConfig.ts:216`（1.2s 轮询） | 合并会引入误拉起 sidecar 的副作用 |
| 🟡 `mcp_project_status`（可合并，收益中） | `mcp/commands.rs:18` | 项目作用域状态（路径校验后**无条件全量探活**——项目页低频打开且开关后需真实状态） | `use-mcp-status.ts:34` → ProjectPage 指示灯 | 开关命令返回的新状态直接回填 |
| 🟡 `mcp_global_status`（可合并） | `commands.rs:37` | 全局作用域状态（默认走 300s 缓存 `ensure_recent`，`force_refresh` 强刷；设置页工具清单不传参用缓存窗口，避免每次打开都拉起全部服务器进程） | `use-mcp-status.ts:83`（恒 forceRefresh）；`tools-tab.tsx:77`（走缓存） | 可统一为 `mcp_status(project_path?, force_refresh?)` |
| 🟢 `mcp_project_set_server_enabled` | `commands.rs:53` | 启停项目服务器：直接改 mcp.json（canonicalize + 受管项目校验），全局条目 copy-on-write 拷入项目，返回刷新后的状态 | `use-mcp-status.ts:47` | 不写全局 DB 故无需 invalidate_all |
| 🟢 `mcp_global_config_get` | `commands.rs:97` | 纯读全局注册表（不触发探活） | `McpServersPage.tsx:162` | 与 status 职责分离：配置 vs 探活状态 |
| 🟢 `mcp_global_config_save` | `commands.rs:108` | 整表替换保存 → `invalidate_all` 清全部作用域缓存 → 回读规范化结果 | `McpServersPage.tsx:184`（400ms 防抖） | 🔧 见域内分析：全局开关复选框走整表保存偏重 |
| 🟢 `ssh_tool_load_config` | `ssh_tool/commands.rs:10` | 读全部服务器配置（**含 password/private_key_passphrase 明文**供表单回显）——见域内分析安全项 | `SshServersPage.tsx:87` | 与 load_audit 永远 `Promise.all` 成对调用 |
| 🟡 `ssh_tool_load_audit`（可合并，收益中） | `commands.rs:17` | 读 AI 审查审计记录 | `SshServersPage.tsx:88` | 可合并为 `ssh_tool_load_settings → {config, audit}` 省一次 IPC |
| 🟢 `ssh_tool_save_config` | `commands.rs:24`（核心 `mod.rs:125`） | 校验（全有或全无）→ 事务整表替换（级联删主机密钥/审计行）→ **丢弃连接池全部连接**（凭据可能已变）→ best-effort 删备忘录文件 | `SshServersPage.tsx:110`（400ms 防抖） | 连接池失效是关键副作用 |
| 🟢 `ssh_tool_test_server_config` | `commands.rs:32` | 测草稿连接（不必先存）；`reset_host_key` 是唯一的 TOFU 重置通道 | `SshServerCard.tsx:60`（含「信任新指纹并重测」恢复入口） | 与 save 互补非重复 |
| 🟢 `ssh_tool_get_memo` | `commands.rs:55` | 读运维备忘录（无文件返回空非错误；require_server_exists 不限 enabled——禁用服务器的历史知识仍可读写） | `SshMemoDialog.tsx:34` | 与 Agent ssh_memo_* 工具同文件同上限约束 |
| 🟢 `ssh_tool_save_memo` | `commands.rs:68` | 整文保存（fail-closed 上限：全文 8000/单段 4000/标题 40；空内容=清空） | `SshMemoDialog.tsx:55` | Option 语义区分读写会与「空串清空」设计冲突，不合并 |
| 🟢 `ssh_tool_import_ssh_config` | `ssh_tool/config_import.rs:10` | 只读解析 `~/.ssh/config` 为草稿（不落库不导凭据；正确处理先命中优先/`Host *` 默认块/通配符；7 个单测覆盖） | `SshServersPage.tsx:191` | 「导入→勾选→补凭据→统一保存」草稿流的生产者 |

### 域内分析（v2 复核后）
- 🟡 **`rag_test_qdrant` + `rag_test_embedding` → `rag_test_connection(config, target)`**：本域收益最高的合并——Rust 实现逐行同构（仅 transport 方法与错误 context 不同），前端本就是同一条 `runTest(target)` 代码路径。**顺带修复双重落库**：test 命令去掉内部 `save_rag_config`，保存责任归调用方（与 `rag_ingest_files` 对齐，同文件内责任自洽）；前端 `runTest` 已先 `persistConfig`（含热推送），行为不降级。风险极低，**推荐 Tier 1**。
- 🔒 **SSH/RAG 凭据明文回传渲染层——v2 新登记的安全双标**：面向 Agent 的 `list_servers_async` 刻意投影 `SshServerSummary` 剥离全部凭据（`ssh_tool/mod.rs:153-177`），而面向设置页的 `ssh_tool_load_config` 原样返回 `password`/`private_key_passphrase`（`types.rs:39,45`），且整份含明文密码的配置长期驻留 React state、每次自动保存原样回传；`rag_get_kb_config` 的 api_key 同病。风险链：渲染进程被恶意 npm 依赖/XSS 攻陷即可取走全部凭据。缓解：password/passphrase 改 write-only（load 返回掩码占位，save 时空/掩码值表示不变、后端合并旧值）——会破坏「整表替换」的简单模型，属较大改造，**建议单独立项**（见 9.5-D1）。
- 🔧 **MCP 全局启停路径偏重**：复选框改一个布尔 → `mcp_global_config_save` 整表重写 + `invalidate_all()` 清全部作用域缓存 → 下次任何状态查询全量重探所有服务器；而项目版开关只写一个文件、只刷单作用域。语义正确无丢失风险，v2 判定为轻度设计债而非缺口；补 `mcp_global_set_server_enabled` 会引入「部分更新 vs 整表保存」两套写入语义并存，**不建议补**，登记即可。
- 🟡 **`ssh_tool_load_config` + `ssh_tool_load_audit` → `ssh_tool_load_settings`**：前端唯一消费点（SshServersPage.tsx:86-89）永远同一 `Promise.all` 拉取；合并省一次往返。若未来审计需独立分页再拆回。
- 🟢 `rag_logs_snapshot/clear`、`ssh_tool_get/save_memo`、`rag_status/restart`、读写对（rag/mcp config）均不合并：语义（取数 vs 破坏性、读 vs 写含空串清空、纯读 vs 重副作用）差异大于省一条注册的收益。

---

## 九、目标命令面重设计与实施分级（v2 核心结论）

### 9.1 设计原则

1. **同型才合并**：仅当多条命令是「同一模板的参数变体」（git stage 家族、browser window 三件套、rag test 双子、session create/delete 镜像对）才合并；语义特例（越界策略相反、状态机互斥、信任域不同）一律保留并在文档标注。
2. **死功能直接删域，不做临终关怀**：通知域 3 条命令服务于恒空数据源，合并/修缮都是浪费——整域移除。
3. **合并命令时顺带修隐性缺陷**：rag test 合并修双重落库；session delete 合并抽共享级联 helper；browser start 合并抽 resolve_project_path。
4. **命令数不是目的，维护面才是**：三处手工双写的级联清单、5 处重复的路径回退 match、3×40 行的 send 骨架，这些「行数债」比注册表条目更值得消除。

### 9.2 🔴 删除（4 条命令 + 2 处字段级死代码）

| 对象 | 方案 | 收益 / 风险 |
|---|---|---|
| **通知域 3 条**（`get_notifications` / `mark_notification_read` / `mark_all_notifications_read`） | 整域删除：`notification.rs`（386 行）+ NotificationBell.tsx（231 行）+ 类型 + 27 处样式 + 注册 3 行 | **约 650+ 行净删除、零功能损失**（远程拉取硬编码禁用，UI 恒空态）/ 极低 |
| `browser_start_plain_chat` | 并入 `browser_start`（`project_path: Option<String>`，与同文件 5 个命令的既有回退惯例对齐） | 1 命令 + 前端 2 处三元 / 极低 |
| `Project.branch` 死列 | 前端恒不写入、WelcomePage 分支 pill 恒显示「本地」；删除涉 DDL/struct/TS/pill 共 6 处，需 schema 迁移 | 假数据展示 / 低（与下次 schema 变更同行） |
| `hasUnreadPopup` / `popup` 死载荷 | 随通知域删除一并消失 | — |

### 9.3 🟡 合并（按优先级排序）

| 优先级 | 合并组 | 目标签名 | 收益 / 风险 |
|---|---|---|---|
| ⭐⭐⭐ | `rag_test_qdrant` + `rag_test_embedding` | `rag_test_connection(config, target: "qdrant"\|"embedding")`，**去掉内部 save** | 2→1 + 修双重落库 + 同文件保存责任自洽 / 极低 |
| ⭐⭐⭐ | `chat_delete_session` + `project_delete_session` | `session_delete(workspace_id)`（查统一表 kind 分流，保留禁跨类误删校验）；**同时抽共享级联 helper**（见 9.4-4） | 2→1 + 消除三处双写维护 / 低 |
| ⭐⭐ | `git_stage`/`unstage`/`stage_all`/`unstage_all` | `git_stage(project_path, files: Option<Vec<String>>, unstage: bool)`（files=None 即全量）；`git_diff` 三合一同型先例 | 4→1（~50 行样板 → ~25 行）/ 低 |
| ⭐⭐ | `browser_minimize`/`restore`/`reopen` | `browser_window_action(session_id, action)`（按 action 保留超时差异与 restore/reopen 语义边界） | 3→1 / 低 |
| ⭐⭐ | `ssh_tool_load_config` + `ssh_tool_load_audit` | `ssh_tool_load_settings() → {servers, audit}` | 2→1；前端本就成对调用 / 低 |
| ⭐⭐ | `chat_create_session` + `project_create_session` | `session_create(kind, title, category?, project_id?)`；顺手统一 category 默认值到后端 | 2→1；前端 Record 改联合类型 / 中低 |
| ⭐ | `mcp_project_status` + `mcp_global_status` | `mcp_status(project_path?, force_refresh?)`（force_refresh 已承载双语义） | 2→1；hook 保留薄封装 / 中 |
| ⭐ | `sub_agent_get_global_enabled` 折叠进 `sub_agent_list` | list 返回体加 `globalEnabled`（LEFT JOIN，保真 `enabled=1 ∩ 全局成员` 交集） | 1 命令；改 SubAgentRecord 契约 / 低 |
| ⭐（可选） | `browser_navigate`/`reload`/`go_back` | 通用 method 转发 | 仅省样板；为类型安全可不做 / 中 |
| ⭐（可选） | `read_image_preview` → 自定义协议 | 注册放行项目根的协议 + 前端 convertFileSrc | 免 13MB base64 JSON IPC / 中成本 |

**执行后命令数**：Tier 全做 130 → **115**（-15：通知域 -3、start_plain_chat -1、session 对 -2、git -3、window -2、rag -1、ssh -1、mcp -1、sub_agent -1）。
其中 ⭐⭐⭐ 合并 + 全部删除项（Tier 1）即达 **124**（-6），且已包含全部「顺带修真实缺陷」的合并（rag 双重落库、级联三处双写的共享 helper）；再把 ⭐⭐ 的 `git_stage` 家族（-3，风险同为低）纳入即 **121**。
> **v3 执行注记（2026-09-06）**：上表全部非可选项已实施（含 `session_create`、`browser_window_action`、`ssh_tool_load_settings`、`mcp_status`、`sub_agent_list` 折叠），两个 ⭐（可选）项（navigate 通用转发、read_image_preview 协议化）按本文档自评「可不做 / 非必须」暂缓。

### 9.4 🔧 工程修缮清单（不减命令数，但消除行数债/规范债）

1. **`dispatcher_get_session_token_usage` 改 async + `run_dispatcher_db`**：agent 命令中唯一同步命令，一行改动。
2. **`dispatcher_stop_run` 移出 settings_commands.rs → run_commands.rs**：纯代码组织（它还是唯一额外注入 BrowserManager State 的 agent 命令，注释里写明原因）。
3. **`dispatcher_clear_messages` 补 `forget_session`**：与删除命令对「会话资源清理规范」的执行对齐。
4. **会话级联清理抽共享 helper**：`purge_session_resources_tx(tx, workspace_id)` 统一 clear / delete_chat / delete_project 三处手工双写的 8 条 DELETE + 图片回收；truncate 的「有意保留」语义单独保留。新增 workspace 键控表时只改一处。
5. **三个 send 抽 `run_agent_turn_skeleton`**：公共序列（begin_run → guards → run_agent_turn → finish → spawn 元数据）约 35-40 行 × 3 收敛为一份，命令退化为差异声明（agent 构建 / kind / workspace_path / with_keywords）。
6. **browser 命令组抽 `resolve_project_path(Option<String>)`**：消除 5 处重复的 plain-chat 回退 match。
7. **`init_project_config` 改 async + spawn_blocking**：当前每次项目切换都在主线程同步做 create_dir_all + atomic_write + 读回解析。
8. **`write_file_content` 改原子写 + 尺寸上限**：复用 `project::storage::atomic_write`，与 rope 路径的 2MB 分流上限对齐。
9. **`ToolCatalog` 死缓存清理**：G11-07 后「读取即重建」，缓存永不命中，删除该结构或恢复真缓存语义。
10. **`graph_run_cancel` 残留自愈分支不再吞 DB 错误**（`graph/commands.rs:241-243` 的 `let _ =`）。
11. **PTY 命令组命名统一**：`task_id`/`shell_id` 参数名归一 + 补 `pty_` 前缀（下次触碰该域时顺带）。
12. **`chat_set_session_category_v6` 摘除 `_v6` 后缀**（全库唯一版本后缀残留）。
13. **`architecture_run_complete` 补 workspace 范围校验**（与 get_tool_artifact 等的域校验风格对齐）。

### 9.5 设计债务登记（v2 新发现，超出命令合并维度）

**A. 前端正确性**
- A1 `useBindChatModel` 陈旧覆盖**不限 400ms**（设置 store 单例永不重读，任意时刻绑定模型后下次设置保存即静默回滚）——让绑定走同一 store 或 save 前重读合并。**优先级最高的前端债**。
- A2 `useChatSessionsQuery` 默认 `pageSize: 100` 且忽略 hasMore——超 100 条聊天会话被静默截断。
- A3 设置三命令并联保存非事务 + `sub_agent_set_global_enabled` 存在性校验毒化后续每次 autosave（建议对已删 id 过滤而非报错，失败提示按命令拆分）。
- A4 `project_list_sessions` 无 id 决胜列（等时间戳分页不稳定，chat 侧修过有测试）+ 前端按已加载数重算 offset（并发删除会跳/重）。
- A5 `SubAgentEditorDialog` 工具清单加载失败仅 console 降级，空列表会被「至少选一个工具」拦下且无提示。

**B. 后端一致性**
- B1 会话级联清理三处手工双写（→ 9.4-4）。
- B2 rag test/save 与 ingest/不save 的保存责任不对称 + `save_rag_config` 无热推送与 `rag_save_kb_config` 行为分叉（→ 9.3 Tier1 合并时修复）。
- B3 `aha_list_agent_tools("project")` 的 4 工具隐性裁剪在命令边界不可见。
- B4 `get_file_meta` 行数公式注释与实现不符（无害但误导）。

**C. 安全（单独立项）**
- C1 **SSH/RAG 凭据明文回传渲染层**：与 Agent 侧 `SshServerSummary` 脱敏投影双标。缓解方向 write-only（load 掩码 + save 空值=不变后端合并），破坏整表替换简单模型，需专门设计。
- C2 `python_runner_stop` / `architecture_run_complete` 无 workspace 归属校验（单窗口本地应用风险低，统一风格即可）。

### 9.6 明确不合并的「易混淆对」（审阅时重点，v2 全部经代码实证）

| 命令对 | 不合并理由（实证依据） |
|---|---|
| `graph_run_start(mode="resume")` vs `graph_run_resume` | DB checkpoint 新建 attempt（前置：计划终态）vs 内存活运行 mpsc 信号（前置：running+槽位存在）——前置状态互斥，两个独立状态机 |
| `rope_replace_line` vs `rope_edit` | 越界语义刻意相反：硬报错（打字热路径防脏行号写坏文件）vs 钳位（结构操作天然易越界）；合并 = 拆护栏 |
| `rope_save` vs `write_file_content` | 尺寸区间互斥（2MB 分流）+ rope 会话 dirty 状态耦合，互替会永久失真 |
| `generate_commit_message` vs `git_commit` | LLM 网络调用 vs 纯子进程；生成结果经人工审校，合并让模型失败连坐提交 |
| `chat_list_sessions` vs `project_list_sessions` | keyset（JSON 游标 + id 决胜）vs offset（无决胜列），数据源表、arch-design 排除逻辑均实质不同 |
| `dispatcher_send_*` 三胞胎 | Agent 构建链、路径安全校验（project 版 canonicalize + 受管项目）、元数据策略（架构跳过关键字）均有语义特例；但应抽共享骨架（9.4-5） |
| `aha_list_agent_tools` vs `sub_agent_list_tools` | 后者是 create/update 校验集的唯一同源镜像；前者 chat 分支混入 `list_sub_agents/call_sub_agent`，合并要么保存被拒要么破坏「保存成功↔运行时真有此工具」保障 |
| `git_push(branch)` vs `git_pull`（无 branch） | push 非当前分支安全（历史视图筛选下拉）；`pull origin B` 会把 B 合并进当前分支，语义危险——不对称是刻意收窄 |
| `rag_status` vs `rag_restart` | 纯读被挂载高频轮询，绝不能附带重启类副作用 |
| `ssh_tool_get_memo` vs `ssh_tool_save_memo` | 「不传参=读」与「空串=清空」的 Option 语义冲突是显式设计 |
| `rag_logs_snapshot` vs `rag_logs_clear` | 取数 vs 破坏性操作；合并让签名承担双职责 |
| `sub_agent_get_run_trace` vs 实时事件流 | 历史回放持久化数据源 vs 运行中事件流，互补设计 |
| `browser_restore` vs `browser_reopen` | 「最小化→就绪」vs「窗口销毁→重建」两个停靠状态机分支（但可合入 `browser_window_action` 分发表保留差异） |

---

## 附：命令 → 前端调用点速查

完整调用点已在各域表格「前端调用」列给出。全局事件与消费方对照：

| 事件 | 生产者（命令/工具） | 前端消费方 |
|---|---|---|
| `dispatcher-session-updated` | 三个 send（标题）、create_session、运行收尾 | `useSessionListEventMerge`、`useChatMessages` |
| `session-keywords-updated` | 三个 send（关键字生成） | `useSessionListEventMerge` |
| `sub-agent-event` | 子智能体运行时（`call_sub_agent` 工具） | `subAgentEventStore`、SubAgentExecutionView |
| `graph-plan-updated` | `graph_plan_update`、服务端 `graph_submit`、run 生命周期 | `useGraphPanelController`、graph-store |
| `graph-run-event` | 图执行引擎（14 种子事件） | `graph-store`（100ms 节流折叠） |
| `architecture-run-request` | `architecture_run` 工具 | `useArchRunListener` |
| `python-run-event` | `python_runner_start` spawn 的后台 agent | `usePythonRunController` |
| `shell-output` | `open_shell` reader 线程（16ms/64KB 批量） | `ShellTerminalPanel`（rAF 帧预算） |
| `browser-frame` / `browser-status` / `browser-log` | sidecar stdout reader | `BrowserPanel`、`useBrowserSessionDock`、`useLiveSessionState` |
| `rag-log` | RagLogStore | `RagSidecarLogPanel` |

> 本文档为静态快照，新增/下线命令时请同步更新总览表、所在域小节与第九节分级表。执行第九节任何合并/删除前，先跑 `pnpm contract:check` 确认前后端契约对齐。
>
> **v3 执行状态**：第九节方案已于 2026-09-06 实施完毕（命令面 130 → 115），逐项状态与新旧命令映射见〇节「执行状态」；第二~八节域表保留 v2 复核快照原貌供溯源。
