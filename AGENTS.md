# JKCodingAgent — AGENTS.md

## 项目概述

JKCodingAgent 是面向 AI 智能体的桌面应用：以「会话（session）」为核心，内置 dispatcher 智能体运行时（多轮工具调用、子智能体、命令审查门禁；项目 Agent 为图编排器——产出执行图 DAG，图节点经 ACP 子进程（claude-agent-acp）执行）、RAG 知识库、嵌入式 Shell / 浏览器 / Python 运行器、文件浏览器、Git 集成与用量记录，外壳 Tauri 2。

**技术栈：** React 19 + TypeScript + Vite（前端） · Tauri 2 + Rust（桌面壳） · Tailwind CSS + shadcn 风格组件 + CSS 变量主题（UI） · Zustand + React Query（状态/数据） · xterm.js（终端） · Shiki（语法高亮） · rusqlite（持久化）

---

## 开发命令

```bash
pnpm dev            # 启动 Vite 开发服务器（端口 1420）
pnpm build          # tsc 类型检查 + Vite 打包
pnpm lint           # ESLint（--max-warnings 0）
pnpm test           # Vitest（前端纯函数/归一化）
pnpm contract:check # Tauri 命令双向契约检查（后端注册 ↔ 前端调用）
pnpm styles:report  # .ai-* 类定义/引用双向 fail-closed 报告
pnpm tauri dev      # 启动完整桌面应用（自动启动开发服务器）
pnpm tauri build    # 构建生产环境桌面二进制包
```

> 新增功能应同步补单元测试（优先纯函数：路径处理、token 统计、segments 解析、分组/归一化等）；Vitest 用例放在被测模块旁的 `*.test.ts(x)`。Rust 后端位于 `src-tauri/`，修改后需重启 `tauri dev`。

---

## 架构设计

### 前端（`src/`）

| 路径 | 职责 |
|------|------|
| `App.tsx` | 根组件：挂载 WelcomePage / ProjectPage，跨视图状态与 Tauri 事件监听 |
| `types.ts` / `types/` | 契约类型统一导出面；权威定义在 `types/` 子模块（infrastructure / chat / sub-agent / graph / rag），改数据结构先改子模块 |
| `styles/tailwind.css` | Tailwind v4 CSS-first 入口（`@theme inline` + `@custom-variant dark`）+ `.ai-*` 组件类集中地 |
| `App.css` | 设计令牌：颜色/间距/字体/动效的 CSS 自定义属性 |
| `lib/cn.ts` | `cn()` = clsx + tailwind-merge，className 合并走它 |
| `components/ui/` | shadcn 风格 headless 基础组件（button / card / input / dropdown-menu / tabs / scroll-area …） |
| `stores/` | Zustand 全局 store（如 `ui-store.ts`） |
| `components/providers/` | React Query / Tooltip 等全局 Provider |

**主视图结构（简化）：**
```
App
├── WelcomePage（主页：聊天 / 项目 / 架构 三视图，AppRail 导航）
│   ├── HomeChatPage（独立聊天工作区）
│   ├── 项目网格（打开 / 删除本地仓库）
│   └── ArchitectureView（Excalidraw 画布 + 助手聊天）
└── ProjectPage（项目工作区）
    ├── ContextNav（会话 / 文件 / 变更 / 历史四页签）
    ├── SessionPanel（会话列表：搜索 / 新建 / 分页）
    ├── 聊天工作台 chat-page-v2（dispatcher 消息流、工具调用、子智能体、图编排入口）
    ├── 图编排 UI components/graph（GraphPlanCard / GraphPanel / GraphNodeDrawer / GraphPlanListView——每会话单例图标签的两级视图：列表态 ↔ 详情态，`graph_plan_list_for_session` 拉会话全部计划轻量摘要）
    ├── SubAgentExecutionView（子智能体执行卡片）
    ├── 文件浏览器 file-explorer（FileViewer / LargeFileViewer / 图片预览）
    ├── GitChanges / GitHistory（变更 / 提交 / 差异）
    ├── ShellTerminalPanel（嵌入式 Shell，xterm.js）
    ├── BrowserPanel（内嵌浏览器，主区单例标签）
    ├── StatusDockBar（底部状态栏：终端 / 浏览器 dock 开关）
    └── AppSettingsDialog（应用设置）
```

异步状态由 Tauri 事件驱动（`@tauri-apps/api/event` 的 `listen()`）。事件类型契约在 `src-tauri/src/agent/rig_ext/events.rs`（`AgentEvent` / `AgentTurn`，字段为前端契约，改动即破坏前端）。在用事件：`dispatcher-session-updated`（会话记录变更）、`sub-agent-event`（子智能体事件流）、`graph-plan-updated`（图计划登记/状态流转，收到后重新 `graph_plan_get`）、`graph-run-event`（节点开始/输出增量/完成/失败/state 更新/收尾）、`python-run-event`、`shell-output`（PTY 字节流）、`browser-frame` / `browser-log` / `browser-status`、`rag-log`。

### 后端（`src-tauri/src/`）

命令注册集中在 `app/mod.rs` 的 `invoke_handler!`，业务逻辑按领域拆分：

| 模块 | 职责 |
|------|------|
| `agent/` | dispatcher 智能体核心（rig-core 0.42 portable contracts）。`rig_ext/`：model（用途槽位→rig 模型）、message（消息桥）、loop/（多轮工具循环+三段式执行+台账+协议拦截）、context（上下文整形）、tool_result（结果落盘/压缩）、summary（标题/关键字）、events（前端事件契约）、agents/（Agent 装配+协议工具+画布 DSL）、sub_agent/（子智能体运行时）、tools/（fs/exec/media/program/mcp/spec 策略表）、review（命令审查）、models；`graph/`（图编排：定义/校验/执行引擎/acp_exec）、`db/`（schema 读写+contract.rs 契约）、`commands/`、`config.rs`（配置+`~/.jkcodingagent` 初始化）、`ssh_review.rs`（命令安全审查）、`sub_agent/{config,manager,db,commands}` |
| `task_runtime/` | `pty.rs`（PTY 创建/读写） |
| `project/` | `storage.rs`（受管项目/会话存储）、`config.rs`（项目配置） |
| `mcp/` | `McpScope{Global, Project}` 作用域：Global = `mcp_servers` 全局注册表（聊天共享单一快照）；Project = 全局 ∪ 项目 `.jkcodingagent/mcp.json`（同名覆盖）。`registry.rs`（缓存/合并/工具执行）、`transport.rs`（stdio/streamable_http/unix_socket_http + 诊断）、`project_file.rs`、`commands.rs`（路径校验） |
| `scm/git.rs` | Git：状态、分支、日志、差异、暂存、提交、推送、拉取 |
| `workspace/` | `fs.rs`（读写/列举）、`rope.rs`（大文件切片） |
| `platform/` | `app_settings.rs`（应用级键值配置） |
| `rag/` | RAG sidecar 传输与管理 |
| `ssh_tool/` | SSH 执行 + AI 安全审查门禁。russh 传输；连接池按 `server_id+session_id` 复用 `Handle`，并发命令各走独立 channel；瞬态网络错误（errno 集合见 `connection.rs`）间隔 1.5s 重试一次，认证/密钥失败不重试；TOFU 指纹 = key blob 的 SHA-256 hex。`memo.rs` 每服务器备忘录 `~/.jkcodingagent/ssh-memos/{server_id}.md`（全文 8000 / 单段 4000 字符硬上限），工具 `ssh_memo_read/upsert/delete`；删服务器随 `save_servers` 级联清理（同事务清主机密钥/审计行，提交后删备忘录文件） |
| `browser.rs` / `chat_images.rs` / `python_runner.rs` | 内嵌浏览器宿主 / 聊天图片存储 / Python 运行器 |

核心约束：
- 所有接受路径参数的命令必须校验路径位于工作区内，防止目录遍历。
- 重型/阻塞操作（文件 I/O、进程、网络、Git）必须 `tokio::task::spawn_blocking`，绝不阻塞 Tauri 主线程。
- 持锁（`parking_lot::Mutex`）期间禁止 I/O——先 clone/取出资源再释放锁。
- 优先用 `tauri::Emitter` 推事件，而非从命令返回大体积数据。
- **assistant 的 `tool_calls` 与 tool 结果必须成对进入 LLM 上下文**（每个 `tool_call_id` 须有紧随其后的 tool 消息，否则整轮 400）。写侧：运行循环在取消/致命失败中止本批时补占位结果（`rig_ext/loop/batch.rs::persist_skipped_tool_results`）；读侧：`agent/common/message.rs::repair_tool_call_pairing` 在历史装配前补齐缺失结果、剔除孤儿结果。

---

## 数据模型

核心契约类型：`Project`、`DispatcherSession`、`ProjectSession`、`GraphPlanRecord` 等；`Task` 为已下线 dispatch 的遗留记录类型，勿再新增。

**持久化：SQLite（rusqlite）**
- 数据库 `~/.jkcodingagent/jkbot.sqlite3`；资源目录 `~/.jkcodingagent/`（`memory/`、`skills/`、`local_env/zsh/`、`chat-images/` 等）。
- **应用配置权威源是全局库**（应用生命周期配置全局一份，随项目变化才放项目目录）：SSH 服务器/主机密钥/审计、`projects`、`mcp_servers`（与项目级 mcp.json 并存、同名覆盖）、`app_config`（浏览器选项/RAG 配置）；主题偏好 `dispatcher_settings.theme`，随 `AhaSettingsV2` 经 `aha_get_settings_v2` / `aha_save_settings_v2` 存取。
- 主要表：`dispatcher_settings`、`ssh_servers`/`ssh_host_keys`/`ssh_audit_log`、`projects`、`mcp_servers`、`app_config`、`sub_agents`、`dispatcher_sessions`、`dispatcher_messages`、`dispatcher_session_token_usage`、`dispatcher_session_summaries`（滚动摘要）、`dispatcher_tool_artifacts`、`chat_images`、`graph_plans`、`graph_node_runs`、分类、关键字索引、python 运行记录等（schema 见 `agent/db/schema.rs`）。
- **模型配置唯一权威 `dispatcher_settings.model_library`**（设置中心是唯一配置源）：用途槽位以 `libraryId` 引用条目，落库只留引用（剥离凭据与容量），读取由条目回填。容量参数同以条目为数据源：
  - `maxTokens`：未配置 → 请求体省略 max_tokens，服务端默认预算接管；
  - `contextWindow`：未配置 → 回退 `DEFAULT_CONTEXT_WINDOW_CAPACITY_TOKENS` = 1M；驱动容量展示、占用告警与历史整形预算（`agent/rig_ext/context.rs::context_budget_chars` = 窗口 × 3.5 字符/token × 0.6 安全系数）；
  - 主对话：每轮请求前 `compact_history` 保头保尾 + 配对安全滑窗，被裁中段折叠为【前情摘要】滚动摘要（摘要模型缺省/失败回退零 LLM 规则抽取），经 `dispatcher_session_summaries` 跨 run 持久化、装配时 `message::apply_stored_session_summary` 前插；上下文 400 超限时预算减半重试 ≤2 次；reasoning 均不回灌；
  - 子智能体：同层 `compact_history_offline` 规则兜底折叠，不消耗摘要模型，保护头部 2 条（system + 首轮任务）。

**存储 schema 版本策略（桌面应用基线 + 前向迁移）**
- 当前 **v11 基线**（`agent/db/schema.rs` 的 `SCHEMA_VERSION`）；`init()` 支持全新建库 / 同版本直开 / 低版本逐级前向迁移（v1→v11 迁移块明细见 `schema.rs`）。
- 每次 schema 变更必须同时：① 更新基线 DDL（新装库直接得到新形态）；② 递增 `SCHEMA_VERSION` 并在 `init()` 迁移挂载点追加 `if current_version < N` 事务块（DDL/回填与 `user_version` 同事务、幂等可重试）。**禁止改写或删除历史迁移块**——已发布版本用户升级的唯一路径。
- 破坏性迁移（DROP/清空数据）前必须整库快照（`VACUUM INTO`），保留「备份失败留痕」兜底。
- 领域自管表（sub_agent / ssh / projects / mcp_servers / app_config）DDL 放在各领域 `ensure_*_tx` 助手，由 `create_baseline` 统一调用，单一出处。

> 修改数据结构时，**必须同步更新 `src/types/` 契约类型与对应 Rust 结构体/SQL schema**——否则新字段序列化时被静默丢弃。

---

## 项目配置

应用级/项目级配置、智能体系统提示词/工具集、SSH/RAG/子智能体设置统一在 `AppSettingsDialog` 编辑，存全局库。项目目录只留随仓库共享的配置：`.jkcodingagent/config.toml`（`[git].commit_prompt`）与项目级 MCP（`.jkcodingagent/mcp.json`，同名覆盖全局注册表）。

**聊天图片统一走 `chat-image://{image_id}` 协议**：唯一保存入口 `chat_images::save_image` 落盘 `~/.jkcodingagent/chat-images/{workspace_id}/{image_id}.{ext}` 并登记 `chat_images` 表（用户粘贴、generate_image/edit_image 产物、fetch_image 下载共用）；`<img>` 经 `convertFileSrc(id, "chat-image")` 直出，asset 协议仅兜底旧消息绝对路径。LLM 侧：`rig_ext::message::attach_turn_tool_images` 每次请求前把本轮消息引用的 `chat-image://` 附加为用户消息视觉输入（上限 3 张、跨迭代去重）；`rig_ext::model::PurposeSwitchingModel` 按请求是否含图在 chat/vision 槽位间委托。

**设置中心：** 外壳 `components/AppSettingsDialog.tsx`（左侧栏单层导航），页面与共享组件在 `components/settings/`：
- `use-aha-settings.ts` — Aha 设置统一 store + 自动保存管线（debounce 400ms → `aha_save_settings_v2`），经 React Context 提供各页。
- `GeneralPage.tsx` — 外观主题（system/light/dark），即点即生效，存 `AhaSettingsV2.theme`。
- `providers/` — 「模型服务」（`ProvidersPage` + `ModelEntryCard`）按对话/视觉/图片生成/图片编辑/语音识别/语音合成/向量分标签维护 `AhaSettingsV2.modelLibrary`（条目独立持有 url/apiKey/model/别名/启停用；对话/视觉另有容量字段 maxTokens/contextWindow——失焦提交、留空缺省；纯函数层 `model-library.ts`）；「模型用途」（`PurposesPage` + `PurposeSelect`）选项来自库条目，`provider-registry.ts::bindPurpose` 写 `libraryId` 引用绑定——落库只留引用、读取回填、库更新后用途自动跟随。无存储字段的 UI 偏好存 localStorage（`provider-prefs.ts`）。
- `ssh/` — 服务器页（状态点 + 自动保存 + 删除二次确认）：`id` 为系统生成的机器标识（不展示/不可编辑），展示 `name`（支持中文）；`SshImportDialog` 从 `~/.ssh/config` 导入 Host 条目（`ssh_tool_import_ssh_config`，纯解析不落库、凭据不导入）。
- 其他设置页：`ToolsPage` / `GraphPage` / `SubAgentsPage` / `mcp/`。
- 共享组件：`ConfirmDialog`、`TestButton`（spinner / ✓ms / 错误展开三态）、`ApiKeyInput`（明文切换）、`StatusBadge`、`EmptyState`、`FieldLabel`（术语 tooltip）、`Section`。通知统一走全局 `toast`（`components/Toast.tsx` 命令式 API，非渲染路径可调；同文案去重、success 2.5s/其余 5s、上限 4 条；`ToastProvider` 挂应用根部，点击 toast 不误关设置弹窗）。
- 样式类 `.ai-set-*` 前缀（`styles/tailwind.css` 的 `@layer components` 末尾）。

---

## 开发规范

### 样式（Tailwind 设计系统）

- **样式来源三层，按优先级**：① shadcn 基础组件（`components/ui/`）优先直接复用；② Tailwind 工具类（布局/间距/排版），合并走 `cn()`；③ `.ai-*` 组件类（`styles/tailwind.css` 的 `@layer components`）——可复用业务级视觉单元（如 `.ai-project-session-row`、`.ai-home-shell`）。
- **设计令牌是 `App.css` 的 CSS 自定义属性**（`--bg-*`、`--text-*`、`--border-*`、`--accent` 等），Tailwind 在 `@theme inline` 块 alias 到它们（v4 CSS-first，无 tailwind.config.js）。暗色令牌在 `App.css` 的 `.dark` 块，`lib/theme.ts` 切换根节点 `.dark` 类并经 `AhaSettingsV2.theme` 持久化，`dark:` 变体由 `@custom-variant dark` 走 class 策略。新增颜色必须同时维护亮/暗两套令牌，不硬编码色值。
- **`preflight` 已禁用**——不依赖 Tailwind 全局 reset，基线样式由 `App.css` 提供。
- **cascade layer 红线（Tailwind v4）**：全部工具类与 `.ai-*` 类都在 cascade layer 里，**未分层样式优先级高于一切分层样式**——未分层的元素级 reset 会把全应用间距工具类静默清零。全局/元素级 reset 只能进 `@layer base`（层序由 `styles/tailwind.css` 顶部 `@layer theme, base, components, utilities` 声明锚定）。
- 局部一次性样式可用行内 `style={{}}`；**不要**新建独立业务 `.css` 文件，也**不要**引入 CSS-in-JS 对象模块；新视觉单元在 `@layer components` 追加 `.ai-*` 类，命名与现有一致。

### 状态管理

- 全局 UI 状态用 Zustand（`stores/`）；服务端/异步数据用 React Query（`components/providers/query-provider`）。
- 跨视图会话级状态可经 `App.tsx` props 下传 + Tauri 事件上抛；组件内短生命周期状态留在组件内。
- 不引入第二套全局状态库。

### TypeScript

- 严格模式开启（`tsconfig.json`）。避免 `any`，扩展 `src/types/` 契约类型。
- Tauri 命令用 `invoke<ReturnType>()` 类型化——新命令记得加泛型。
- **导出面不代表公共 API**：`src/lib/` 纯函数与组件内纯逻辑模块可为同目录 Vitest 用例导出内部符号（export-for-test 不算导出面污染）；`src/types/` 契约层整体导出。组件导出的 `XxxProps`、内部常量可能只是实现细节——跨模块复用前先确认归属层次。不引入 knip/ts-prune 类导出守护。

### Rust

- 新增 Tauri 命令按领域归入对应模块，并在 `app/mod.rs` 的 `invoke_handler!` 注册。
- 重型操作一律 `spawn_blocking`；锁作用域尽量短；用 `Emitter` 推事件而非返回大数据。

---

## 新增 Agent 工具流程（rig 形态）

工具是 rig `PortableDynamicTool`（`rig_ext/tools/` 下的构造器返回 `Vec<PortableDynamicTool>`），业务逻辑写在同目录文件中。

### 1. 实现工具 — `src-tauri/src/agent/rig_ext/tools/<组>.rs`

```rust
pub(crate) fn my_tool(deps: &RigToolDeps) -> PortableDynamicTool {
    let parameters = with_compression_parameters(json!({ /* JSON Schema */ }), false,
        COMMAND_FORCE_COMPRESS_AFTER_CHARS, "何时值得压缩的文案");
    let workspace = deps.workspace.clone();
    PortableDynamicTool::new("my_tool", "工具用途描述（逐字写清约束，模型行为依赖它）", parameters,
        move |args| {
            let workspace = workspace.clone();
            Box::pin(async move {
                // 参数提取：super::common::{string_arg, usize_arg, boolish_arg, ...}
                // 路径沙箱：super::common::resolve_path(&workspace, restrict, &extra, raw)
                // 阻塞 I/O：spawn_blocking；取消：deps.cancel_rx
                // Ok(ToolOutput::text(...)) / Err(ToolExecutionError::{invalid_args,refused,timeout,other})
                //   错误以「错误：」开头；可重试 .with_retryable(true)；致命 .with_code("fatal")
            })
        })
}
```

要点：
- 构造期依赖来自 `RigToolDeps`（`rig_ext/tools/deps.rs`）：workspace/白名单、MCP 作用域、DB、SSH、子智能体管理器、取消信号、视觉/图像凭据、审查上下文（`review`）。逐次调用身份由 `loop::invocation::ToolInvocationContext` 在 rig 回调边界注入；入口取得 owned clone 显式传递，禁止共享可变调用槽。
- 有命令执行/外部效应的工具**自带 fail-closed 审查**（`deps.review` + `ssh_review::review_shell_command`），与 local_zsh / ssh_exec / sync_directory / MCP 桥一致。
- 压缩阈值与内联上限取自 `rig_ext/tool_result.rs`（命令类 12000，默认 5000）；schema 文案必须与运行时策略一致。

### 2. 挂进工具面 — 相应组的入口

- 普通聊天 → `rig_ext/tools/exec/`（+ `media/`）→ `rig_ext/agents/plain_chat.rs::build_surface` 汇总；
- 编排器数据面（read_file/list_dir/glob/grep）→ `rig_ext/tools/fs/`，并登记 `rig_ext/tools/mod.rs` 的 `ORCHESTRATOR_RUNTIME_TOOL_NAMES`；
- 子智能体 → `rig_ext/sub_agent/runner.rs::build`（继承普通聊天 profile）；
- 协议壳（submit_graph / graph_plan_report / message / graph_get / graph_node_{update,add,delete}）→ `rig_ext/agents/project_tools.rs`（fail-closed 壳，回调只报错）；真实动作由 `RigOrchestratorProtocol` 拦截（`rig_ext/agents/project.rs` 实现 `rig_ext::r#loop::ProtocolToolHandler`，submit 拦截在 `project_submit.rs`，图感知 graph_get 与 draft 图节点级 CRUD（update 定点改字段 / add 含 insertBefore 边接管中间插入 / delete 含 force 下游闭包级联）在 `project_graph_ops.rs`——三者复用 `graph/commands.rs::apply_draft_definition_update`（前端 `graph_plan_update` 命令同源：draft 双检 + 整图校验 + 条件更新 + 广播））。

### 3. 登记策略表 — `src-tauri/src/agent/rig_ext/tools/spec.rs`

在 `TOOL_POLICY_TABLE` 补一行（category/access/safety/timeout/compress/parallel/self-managed）。该表是**台账元数据、审查门禁判定、统一超时与结果策略的唯一来源**；未收录工具名走 fail-closed 兜底（只读 + 需审查 + 串行）。

**超时契约（`ToolExecutionPolicy`）**：
- `unified_timeout=true`：策略层包统一超时；到点发取消 → 宽限收敛（`SETTLE_CEILING`）→ 仍不收敛则移交后台并按「结算未确认」收口（绝不 drop future）。
- `unified_timeout=false`（自管超时，当前 6 个：`local_zsh`/`ssh_exec`/`sync_directory`/`analyze_image`/`call_sub_agent`/`run_tool_program`）：执行预算工具自管（分阶段超时、交互/静默容忍、优雅终止：杀进程树 / 关 channel / 写审计与台账）；仍以 `settle_ceiling_secs`（= 最坏合法预算，登记在 `self_managed_settle_ceiling_secs`）作最后防线，到点走统一超时同一收口。`cancellable=false` 时兜底到点不发取消、直接交接后台。新增自管工具**必须**登记 settle ceiling（`spec.rs` 的 `self_managed_tools_declare_settle_ceiling` 测试守护）。

### 4. 工具输出压缩（可选）

「显式声明（`compress=true`）+ 阈值」双条件驱动：声明且超阈值时 `rig_ext/tool_result.rs` 调用摘要模型（15s 超时），失败/超时回退零 LLM 的 `extract_structured_summary` 规则抽取；未摘要的超长结果按内联上限确定性截断，完整原文进工具产物。阈值随策略表声明（默认 5000，命令类 12000）。

### 5. 配置（可选）

需要 API Key / URL 等配置时：`config.rs` 的 `DispatcherAgentConfig` 加字段 → `load()` 读取 → 在 `rig_ext/agents/*.rs` 构造 `RigToolDeps` 时传入。

---

## 已知技术债务与防劣化规则

> 新增代码**必须遵守**，存量代码逐步修复。

### 前端性能

- **组件控制渲染范围**——列表行组件用 `memo`，大 props 容器组件继续收敛。
- **高频事件回调避免 `setState`**——PTY 输出等用 buffer/ref 批处理，不逐条触发全局重渲染。
- **长列表必须虚拟化**——消息流、文件列表数千条会卡顿，新增类似列表必须考虑虚拟滚动。
- **大文本禁止同步 `marked()`**——单条消息超 10KB 用异步渲染或 memoize。
- **语言包按需加载**——Shiki / CodeMirror / Monaco 语言包必须动态 `import()`，避免主包膨胀。
- **@提及 / 搜索必须防抖**——万级文件项目的过滤加 ~200ms 防抖或用 `startTransition`。

### 后端性能

- **Tauri async 命令内禁止直接阻塞**——文件 I/O、进程、网络必须 `spawn_blocking`。
- **PTY 读取缓冲区 ≥ 32KB**——避免大量输出产生上万次事件。
- **持锁期间禁止 I/O**——先取出资源再释放锁。
- **会话消息禁止全文件一次性加载**——流式读取或分页。

### 安全

- **路径参数命令必须校验合法性**（位于工作区内、合法绝对路径），避免目录遍历。
- **Mutex 获取禁止裸 `.unwrap()`**——继续收敛中毒风险点。
- **命令执行门禁**——SSH / local_zsh 等命令工具走 AI 审查 + fail-closed 门禁，新增可执行命令的工具必须接入同一审查链路。

### 组件规模

- **单个生产文件不应超过 500 行**（前端组件建议 ≤400 行）。超限文件按「变化原因与状态所有权」拆分（入口薄壳 + 领域子模块 + 独立测试模块），不机械按行切片。新增功能落在超限文件时优先拆分再扩展。

---

## 禁止事项

- **不要重新引入 CSS-in-JS 样式模块或竞争性样式方案。** 项目统一于 Tailwind + `.ai-*` 组件类 + shadcn `ui/` 组件 + `App.css` 令牌；样式变更在此体系内进行。
- **不要引入第二套全局状态库**——UI 状态用 Zustand，异步数据用 React Query。
- **交互式 UI 原语优先用组件库而非原生元素**——下拉、对话框、提示框用 Radix（已装 `@radix-ui/*`）或 `components/ui/`，而非 `<select>`/`<dialog>` 或自行实现。图标用 `lucide-react`。
- **`read_file_content` 不要读取超过 2 MB 的文件**（Rust 侧强制）。
- **修改存储 schema 必须遵循「基线 + 前向迁移」规范**：更新基线 DDL、递增 `SCHEMA_VERSION`、追加事务化迁移块，三者缺一不可。开发阶段重置本地数据用 `scripts/reset-dev-data.sh`，不要手删 `~/.jkcodingagent/jkbot.sqlite3`（会留下 WAL/迁移残留）。
- **不要阻塞 Tauri 主线程**——重型操作一律 `spawn_blocking`。

---

## 会话与项目资源清理规范

**删除会话或清空消息时，必须同步清理其绑定的所有关联资源**——不得仅依赖数据库级联。

**0. 运行中会话 fail-closed 守卫（先于一切删除/清空/截断）**：`session_delete`、`dispatcher_clear_messages`、`dispatcher_truncate_messages_from`、`project_delete` 在会话（或项目任一会话）有活动 run 时直接拒绝（`DispatcherState::session_run_is_active`，底座 `ActiveRunStore`）——运行方仍持有消息/用量写入路径，放行会导致幽灵写入。run 收尾后异步标题/关键字生成同样校验存在性（`update_session_title` 不存在行返回 None 不广播；`apply_keyword_actions` 不存在会话返回 false 不回插）。配套 `dispatcher_active_runs` 命令 + 前端 `run-state-reconciliation.ts`：webview 重载后 App 挂载时对账仍在跑的会话（补后台运行状态、可停止、轮询收尾拉全量消息）。新增会话破坏性命令必须接入同一守卫。

1. **图片文件**：按会话目录 `~/.jkcodingagent/chat-images/{workspace_id}/` 布局，删除/清空会话时随 `chat_images` 记录整目录回收（`delete_chat_image_resources` + `remove_chat_image_dir`）。`truncate_messages_from`（regenerate/edit 前置）**有意不删图片文件**——重发复用同一批 image_id；发送前有 `chat_images_validate` 校验兜底。
2. **工具产物文件**：`dispatcher_tool_artifacts` 指向的产物文件需显式清理。
3. **项目删除**：`project_delete`（`project/storage.rs`）在同一事务内遍历项目全部会话执行与 `delete_project_session` 相同的级联清理并删除项目行；提交后 best-effort 清理聊天图片与项目仓库内应用自有目录（`.jkcodingagent/browser-profile/`、`.jkcodingagent/local_env/`）。config.toml / mcp.json 可能随仓库共享给团队，保留不删。
4. **通用约定**：与会话绑定的文件资源（图片、附件、缓存）在会话删除/清空时必须同步清理文件系统，不能只清 DB。
5. **消息截断（regenerate / 编辑重发）**：`truncate_messages_from` 除删除消息、工具产物与工具运行外，还须回收被删轮次副作用——子智能体 trace（`sub_agent_run_traces`，按被删工具运行的 `tool_call_id` 精确匹配）、图编排产物（`graph_plans` 按计划创建时间 ≥ 目标消息时刻截断，`graph_runs`/`graph_node_runs`/`graph_node_activities` 随外键级联）与滚动摘要（`dispatcher_session_summaries`：锚点在被删范围的精确删除，早于截断点的仍有效、有意保留）；`python_code_runs`/`chat_images` 行由外键级联。有意保留：token 用量（真实消耗记录）、会话关键字（重发后自然覆盖）、图片文件（重发复用 image_id）。
