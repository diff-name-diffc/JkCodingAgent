# JKCodingAgent — AGENTS.md

## 项目概述

JKCodingAgent 是一款面向 AI 智能体的现代桌面应用：以「会话（session）」为核心，内置 dispatcher 智能体运行时（多轮工具调用、子智能体、命令审查门禁；项目 Agent 为图编排器——产出执行图 DAG 并调度子智能体 / claude / codex 节点执行）、RAG 知识库、嵌入式 Shell / 浏览器 / Python 运行器、文件浏览器、Git 集成与用量分析，外壳为 Tauri 2。

**技术栈：** React 19 + TypeScript + Vite（前端） · Tauri 2 + Rust（桌面壳） · **Tailwind CSS + shadcn 风格组件 + CSS 变量主题**（UI） · Zustand + React Query（状态/数据） · xterm.js（终端） · Shiki（语法高亮） · rusqlite（持久化）

---

## 开发命令

```bash
pnpm dev            # 启动 Vite 开发服务器（端口 1420）
pnpm build          # tsc 类型检查 + Vite 打包
pnpm lint           # 运行 ESLint（--max-warnings 0）
pnpm tauri dev      # 启动完整桌面应用（自动启动开发服务器）
pnpm tauri build    # 构建生产环境桌面二进制包
```

> ⚠️ **测试待重建。** 旧的 Vitest 测试套件已在重构中移除，`pnpm test` 暂不可用。新增功能时应同步补回单元测试（优先纯函数：路径处理、token 统计、segments 解析等）。

Rust 后端位于 `src-tauri/`，修改后需重启 `tauri dev`。

---

## 架构设计

### 前端（`src/`）

| 路径 | 职责 |
|------|------|
| `App.tsx` | 根组件：挂载 WelcomePage / ProjectPage，持有跨视图状态与 Tauri 事件监听 |
| `types.ts` | TypeScript 接口权威定义——修改数据结构时优先编辑此文件 |
| `styles/tailwind.css` | Tailwind 入口（`@tailwind base/components/utilities`）+ `.ai-*` 组件类集中地 |
| `App.css` | 设计令牌（Design Tokens）：颜色/间距/字体/动效的 CSS 自定义属性，供 Tailwind 与组件类引用 |
| `tailwind.config.js` | Tailwind 配置：`preflight: false`，颜色全部 alias 到 `App.css` 的 CSS 变量 |
| `lib/cn.ts` | `cn()` = `clsx` + `tailwind-merge`，所有 className 合并走它 |
| `components/ui/` | shadcn 风格 headless 基础组件（button / card / input / dropdown-menu / tabs / scroll-area …） |
| `stores/` | Zustand 全局 store（如 `ui-store.ts` 的侧边栏折叠等 UI 状态） |
| `components/providers/` | React Query / Tooltip 等全局 Provider |

**主视图结构（简化）：**
```
App
├── WelcomePage                      — 主页（聊天 / 项目 / 分析 三视图切换）
│   ├── HomeChatPage                 — 独立聊天工作区（不绑定项目）
│   ├── 项目网格                      — 打开 / 删除本地仓库
│   └── AnalyticsDashboard           — token / 工具调用用量图表
└── ProjectPage                      — 项目工作区
    ├── ProjectRail                  — 左侧项目切换栏
    ├── SessionPanel                 — 会话列表（搜索 / 新建 / 分页）
    ├── 聊天工作台 (chat-page-v2)      — dispatcher 消息流、工具调用、子智能体、图编排入口
    ├── 图编排 UI (components/graph)    — GraphPlanCard 内联卡片 / GraphPanel 执行图画布 / GraphNodeDrawer 节点详情
    ├── SubAgentExecutionView         — 子智能体执行卡片（阶段/时间线/统计）
    ├── 文件浏览器 (file-explorer)     — FileViewer / LargeFileViewer / 图片预览
    ├── GitChanges / GitHistory       — 变更 / 提交 / 差异
    ├── ShellTerminalPanel            — 嵌入式交互 Shell（xterm.js）
    ├── BrowserPanel / BrowserDock    — 内嵌浏览器
    └── AppSettingsDialog             — 应用设置（智能体 / RAG / SSH / 子智能体配置）
```

异步状态由 Tauri 事件驱动（`@tauri-apps/api/event` 的 `listen()`），当前在用的事件：
- `dispatcher-session-updated` — 会话记录变更（消息、标题、用量等）
- `sub-agent-event` — 子智能体执行事件流
- `graph-plan-updated` — 图编排计划登记/状态流转（收到后重新 `graph_plan_get`）
- `graph-run-event` — 图执行进展（节点开始/输出增量/完成/失败/共享 state 更新/运行收尾）
- `python-run-event` — Python 运行器事件
- `shell-output` — 嵌入式 Shell 的 PTY 字节流
- `browser-frame` / `browser-log` / `browser-status` — 内嵌浏览器
- `rag-log` — RAG sidecar 日志

### 后端（`src-tauri/src/`）

命令注册集中在 `app/mod.rs` 的 `invoke_handler!`，业务逻辑按领域拆分到模块：

| 模块 | 职责 |
|------|------|
| `agent/` | dispatcher 智能体核心：`run_loop/`（运行循环）、`llm.rs`（模型调用）、`tools/`（工具注册表 + builtin 工具）、`summary.rs`（工具输出分类/摘要）、`sub_agent/`（子智能体）、`graph/`（图编排：定义/校验/执行引擎/命令）、`db/`（SQLite schema 与读写）、`commands.rs`（Tauri 命令）、`config.rs`（智能体配置 + `~/.jkcodingagent` 初始化） |
| `task_runtime/` | `pty.rs`（PTY 创建/读写）、`session.rs`（会话/输出兜底） |
| `project/` | `storage.rs`、`config.rs`（项目配置）、`analytics.rs` |
| `mcp/` | MCP 子系统：`McpScope{Global, Project}` 显式作用域模型——`Global`（`mcp_servers` 全局注册表，所有聊天共享单一快照）与 `Project`（全局 ∪ 项目 `.jkcodingagent/mcp.json`，同名项目覆盖）；`registry.rs`（作用域缓存/合并/工具执行）、`transport.rs`（stdio/streamable_http/unix_socket_http + 诊断）、`project_file.rs`（项目文件读写）、`commands.rs`（Tauri 命令，项目命令前置路径校验） |
| `scm/git.rs` | Git 集成：状态、分支、日志、差异、暂存、提交、推送、拉取 |
| `workspace/` | `fs.rs`（文件读写/列举）、`rope.rs`（大文件切片） |
| `platform/` | `app_settings.rs`、`notification.rs`、`usage.rs` |
| `rag/` | RAG sidecar 传输与管理 |
| `ssh_tool/` | SSH 命令执行 + AI 安全审查门禁。传输层为 russh（纯 Rust 异步，无 libssh2/OpenSSL 依赖）；连接池按 `server_id+session_id` 复用 russh `Handle`，并发命令各走独立 channel（协议级隔离，无逐命令互斥锁）；主机密钥 TOFU 指纹为 key blob 的 SHA-256 hex |
| `browser.rs` | 内嵌浏览器宿主 |
| `chat_images.rs` | 聊天图片存储 |
| `python_runner.rs` | Python 运行器 |
| `tools/image_generator.rs` | 图像生成 |

核心约束：
- 所有接受路径参数的命令必须校验路径位于工作区内，防止目录遍历。
- 重型/阻塞操作（文件 I/O、进程、网络、Git）必须走 `tokio::task::spawn_blocking`，绝不阻塞 Tauri 主线程。
- 持锁（`parking_lot::Mutex`）期间禁止做 I/O——先 clone/取出资源再释放锁。
- 优先用 `tauri::Emitter` 向前端推事件，而非从命令返回大体积数据。

---

## 数据模型

会话为中心的核心类型定义在 `types.ts`：`Project`、`DispatcherSession`、`ProjectSession`、`GraphPlanRecord`（图编排计划）等；`Task` 为旧 dispatch 子进程的历史记录类型（dispatch 已下线，不再新增）。

**持久化：SQLite（rusqlite）**
- 数据库文件：`~/.jkcodingagent/jkbot.sqlite3`
- 资源目录：`~/.jkcodingagent/`（含 `memory/`、`skills/`、`local_env/zsh/`、`chat-images/` 等）
- **应用配置的权威源是全局库**（分层原则：应用生命周期配置一律全局一份；只有随项目变化之物放项目目录）：SSH 服务器/主机密钥/审计（`ssh_servers` 等表）、受管项目注册表（`projects` 表）、MCP 全局注册表（`mcp_servers` 表，与项目级 `mcp.json` 并存、同名项目覆盖）、应用级键值配置（`app_config` 表：theme/全局浏览器选项/RAG 配置）。
- 主要表：`dispatcher_settings`、`ssh_servers`/`ssh_host_keys`/`ssh_audit_log`、`projects`、`mcp_servers`、`app_config`、`sub_agents`、`dispatcher_sessions`、`dispatcher_messages`、`dispatcher_session_token_usage`、`dispatcher_tool_artifacts`、`chat_images`、`graph_plans`、`graph_node_runs`、分类、关键字索引、python 运行记录等（schema 见 `agent/db/schema.rs`）
- 模型配置：`dispatcher_settings.model_library` 为唯一权威；用途槽位以 `libraryId` 引用库条目（保存剥离凭据、读取回填）。环境变量回退（DASHSCOPE_*/MODEL_NAME 等）默认关闭，仅 `AHA_ALLOW_ENV_PROVIDER=1` 显式开启。

**存储 schema 版本策略（桌面应用基线 + 前向迁移）**

- 当前为 **v1 基线**（`agent/db/schema.rs` 的 `SCHEMA_VERSION`）：应用开发阶段无存量用户，历史 v0→v33 迁移链已按产品决策清除；`init()` 只有「全新建库到当前形态」与「同版本直开」两条路径，低于基线的旧开发库直接报错并引导运行 `scripts/reset-dev-data.sh`。
- 后续每次 schema 变更必须同时做两件事：① 更新 `schema.rs` 的基线 DDL（新装库直接得到新形态）；② 递增 `SCHEMA_VERSION` 并在 `init()` 迁移挂载点追加 `if current_version < N` 的事务块（DDL/回填与 `user_version` 推进同事务、幂等可重试）。**禁止改写或删除历史迁移块**——它们是已发布版本用户升级的唯一路径。
- 破坏性迁移（DROP/清空数据）前必须做整库快照备份（参考 `VACUUM INTO` 方案），并保留「备份失败留痕」的兜底。
- 领域模块自管的表（sub_agent / ssh / projects / mcp_servers / app_config）的 DDL 放在各领域的 `ensure_*_tx` 助手中，由 `create_baseline` 统一调用，保持单一出处。

> 修改数据结构时，**必须同步更新 `types.ts`（TS）与对应的 Rust 结构体/SQL schema**——否则新字段在序列化时会被静默丢弃。

---

## 项目配置

应用级与项目级配置、智能体系统提示词/工具集、SSH/RAG/子智能体设置统一在 `AppSettingsDialog` 中编辑，存全局库。项目目录下仅保留随仓库共享的配置（`.jkcodingagent/config.toml` 的 `[git].commit_prompt`）与项目级 MCP（`.jkcodingagent/mcp.json`，同名覆盖全局注册表）。聊天图片保存至 `~/.jkcodingagent/chat-images/{session-title-slug}/`，路径写入 `chat_images` 表供智能体读取。

**设置中心结构（2025 重构后）：** 外壳 `components/AppSettingsDialog.tsx`（左侧栏单层导航 + 内容区两层结构），页面与共享组件在 `components/settings/`：
- `use-aha-settings.ts` — Aha 设置的统一 store + 失焦/变更自动保存管线（debounce 400ms 整体调用 `aha_save_settings_v2`），通过 React Context 提供给各设置页。
- `providers/` — 「模型服务」与「模型用途」页。「模型服务」页（`ProvidersPage.tsx` + `ModelEntryCard.tsx`）按模型调用方式分标签（对话/视觉/图片生成/图片编辑/语音识别/语音合成/向量）维护**分类模型库**（`AhaSettingsV2.modelLibrary`，每条目独立持有 url/apiKey/model/别名/启停用，纯函数层在 `model-library.ts`）；「模型用途」页（`PurposesPage.tsx` + `PurposeSelect.tsx`）的下拉选项来自对应分类的库条目，选中后由 `provider-registry.ts` 的 `bindPurpose` 写入携带 `libraryId` 的引用绑定——落库只保留引用（后端剥离 url/apiKey/model），读取时由库条目回填凭据，库更新后用途自动跟随。最近测试结果等无存储字段的 UI 偏好存 localStorage（`provider-prefs.ts`）。旧用户首次打开设置时按分类从已有用途配置播种模型库（`use-aha-settings.ts` + `seedModelLibrary`）。
- `ssh/` — SSH 服务器页（状态点 + 自动保存 + 删除二次确认）。服务器 `id` 为机器标识（系统自动生成，不展示/不可编辑），界面展示 `name`（支持中文）；`SshImportDialog` 支持从本机 `~/.ssh/config` 解析导入 Host 条目（后端 `ssh_tool_import_ssh_config`，纯解析不落库，凭据不导入）。
- 共享组件：`ConfirmDialog`（删除二次确认）、`TestButton`（测试三态：spinner / ✓ms / 错误展开）、`ApiKeyInput`（字段级明文切换）、`StatusBadge`、`EmptyState`、`FieldLabel`（术语 tooltip）、`Section`、`toast.ts` + `Toaster`。
- 设置中心样式类统一 `.ai-set-*` 前缀（`styles/tailwind.css` 的 `@layer components` 末尾）。

---

## 开发规范

### 样式（Tailwind 设计系统）

- **样式来源有三层，按优先级使用：**
  1. **shadcn 基础组件**（`components/ui/`）——按钮、输入框、卡片、下拉等优先直接复用。
  2. **Tailwind 工具类**——布局、间距、排版等用 `className="flex gap-2 …"`，合并走 `cn()`。
  3. **`.ai-*` 组件类**（集中在 `styles/tailwind.css` 的 `@layer components`）——可复用的业务级视觉单元（如 `.ai-project-session-row`、`.ai-home-shell`）。
- **设计令牌是 `App.css` 中的 CSS 自定义属性**（`--bg-*`、`--text-*`、`--border-*`、`--accent` 等），Tailwind 颜色在 `tailwind.config.js` 中 alias 到这些变量。应用仅提供亮色主题，不设主题切换。新增颜色优先扩展现有令牌，不要硬编码色值。
- **`preflight` 已禁用**——不要依赖 Tailwind 的全局 reset；基线样式由 `App.css` 提供。
- 组件局部、真正一次性的样式可用行内 `style={{}}`；但**不要**新建独立业务 `.css` 文件，也**不要**重新引入 CSS-in-JS 对象模块（旧的 `styles/*.ts` 已全部移除）。
- 新增可复用视觉单元时，在 `tailwind.css` 的 `@layer components` 内追加 `.ai-*` 类，并保持与现有命名一致。

### 状态管理

- 全局 UI 状态用 **Zustand**（`stores/`）；服务端/异步数据用 **React Query**（`components/providers/query-provider`）。
- 跨视图的会话级状态仍可通过 `App.tsx` props 下传 + Tauri 事件上抛；组件内部短生命周期状态保留在组件内。
- 不要再引入第二套全局状态库。

### TypeScript

- 严格模式已开启（`tsconfig.json`）。避免 `any`，应扩展 `types.ts`。
- Tauri 命令使用 `invoke<ReturnType>()` 类型化——添加新命令时记得加泛型。

### Rust

- 新增 Tauri 命令按领域归入对应模块，并在 `app/mod.rs` 的 `invoke_handler!` 列表注册。
- 重型操作一律 `spawn_blocking`；锁作用域尽量短；用 `Emitter` 推事件而非返回大数据。

---

## 新增 Agent 工具流程

新增工具涉及 3 个文件（必选）+ 1 个文件（可选），前端无需修改。

### 1. 实现工具 — `src-tauri/src/agent/tools/builtin/<tool_name>.rs`

创建新文件，实现 `AgentTool` trait（定义在 `registry.rs`）：

```rust
pub(super) fn my_tool() -> Box<dyn AgentTool> { Box::new(MyTool) }
struct MyTool;
#[async_trait]
impl AgentTool for MyTool {
    fn name(&self) -> &'static str { "my_tool" }
    fn description(&self) -> &'static str { "工具用途描述" }
    fn parameters(&self) -> Value { json!({ "type": "object", "properties": { ... } }) }
    async fn execute(&self, args: &Value, ctx: &ToolContext) -> String { ... }
}
```

- 参数提取用 `common.rs` 中的 `string_arg` / `boolish_arg` 等辅助函数
- 路径参数必须通过 `resolve_path(ctx, raw)` 确保不越界
- 错误消息以 `"错误："` 开头
- 如需 `result_mode` 参数，调用 `with_result_mode_parameter(schema, default, guidance)`

### 2. 注册工具 — `src-tauri/src/agent/tools/builtin/mod.rs`

两处：顶部 `mod my_tool;` + `builtin_tools()` 函数中添加 `my_tool::my_tool()`。

### 3. 工具输出压缩（可选）— `src-tauri/src/agent/summary.rs`

工具结果压缩是阈值驱动、无需注册：超过 `FORCED_COMPRESS_THRESHOLD`（1000 字符，`agent/common.rs`）的结果由 `persist_tool_result_with_compression` 自动调用摘要模型压缩（模型也可用 `compress`/`compress_intent` 参数自声明压缩意图）。摘要模型失败时回退到零 LLM 的规则抽取 `extract_structured_summary(tool_name, raw_output)`（`summary.rs`）——新工具如需定制兜底摘要，在该函数的 `match tool_name` 中加一个分支即可。

### 4. 添加配置（可选）— `src-tauri/src/agent/config.rs`

如工具需要 API Key / URL 等配置：在 `DispatcherAgentConfig` 加字段 → `load()` 中从环境变量读取 → 在构建 `ToolContext` 处传入（项目编排器：`agents/project/iteration.rs`；聊天 Agent：`agents/plain_chat/mod.rs`）。

> 特例：`submit_graph`（图编排收口工具）只注册进编排器专用注册表（`ToolRegistry::orchestrator_tools`），不进通用 `builtin_tools()`，避免污染聊天上下文与设置页工具清单。

---

## 已知技术债务与防劣化规则

> 新增代码**必须遵守**，存量代码逐步修复。

### 前端性能

- **组件必须控制渲染范围**——列表行组件用 `memo`，接收大量 props 的容器组件继续收敛。
- **高频事件回调中避免 `setState`**——PTY 输出等高频事件用 buffer/ref 批处理，不要逐条触发全局重渲染。
- **长列表必须虚拟化**——消息流、文件列表在数千条时会卡顿，新增类似列表必须考虑虚拟滚动。
- **大文本禁止同步 `marked()`**——单条消息超过 10KB 时用异步渲染或 memoize。
- **语言包按需加载**——Shiki / CodeMirror / Monaco 语言包必须动态 `import()`，避免主包膨胀（当前 monaco-vendor 已达 4MB+，需持续治理）。
- **@提及 / 搜索必须防抖**——万级文件项目中的过滤应加 ~200ms 防抖或用 `startTransition`。

### 后端性能

- **Tauri async 命令内禁止直接阻塞**——凡涉及文件 I/O、进程、网络，必须 `spawn_blocking`。
- **PTY 读取缓冲区 ≥ 32KB**——避免大量输出产生上万次事件。
- **持锁期间禁止 I/O**——先取出资源再释放锁。
- **会话消息禁止全文件一次性加载**——流式读取或分页。

### 安全

- **所有路径参数命令必须校验合法性**（位于工作区内、合法绝对路径），避免目录遍历。
- **Mutex 获取禁止裸 `.unwrap()`**——继续收敛中毒风险点。
- **命令执行门禁**——SSH / local_zsh 等命令工具走 AI 审查 + fail-closed 门禁，新增可执行命令的工具必须接入同一审查链路。

### 组件规模

- **单个组件文件不应超过 400 行。** 当前仍超标的大文件（按现状持续拆分）：`task_runtime/session.rs`（~1500）、`agent/commands.rs`（~1480）、`browser.rs`（~1240）、`file-viewer/LargeFileViewer.tsx`（~1120）、`components/ProjectPage.tsx`（~680）等。新增功能若落在这些文件，优先拆分再扩展。

---

## 禁止事项

- **不要重新引入 CSS-in-JS 样式模块或竞争性样式方案。** 项目已统一到 Tailwind + `.ai-*` 组件类 + shadcn `ui/` 组件 + `App.css` 令牌；样式变更在此体系内进行。
- **不要引入第二套全局状态库**——UI 状态用 Zustand，异步数据用 React Query。
- **交互式 UI 原语优先用组件库而非原生元素**——下拉、对话框、提示框等用 Radix（已装 `@radix-ui/*`）或 `components/ui/`，而非 `<select>`/`<dialog>` 或自行实现。图标用 `lucide-react`。
- **`read_file_content` 不要读取超过 2 MB 的文件**（Rust 侧强制）。
- **修改存储 schema 必须遵循「基线 + 前向迁移」规范**（见上文「存储 schema 版本策略」）：更新基线 DDL、递增 `SCHEMA_VERSION`、追加事务化迁移块，三者缺一不可。开发阶段重置本地数据用 `scripts/reset-dev-data.sh`，不要手删 `~/.jkcodingagent/jkbot.sqlite3`（会留下 WAL/迁移残留）。
- **不要阻塞 Tauri 主线程**——重型操作一律 `spawn_blocking`。

---

## 会话与项目资源清理规范

**删除会话或清空消息时，必须同步清理其绑定的所有关联资源**——不得仅依赖数据库级联。

1. **图片文件**：`chat_images` 表记录通过外键 `ON DELETE CASCADE` 随 `dispatcher_messages` 删除，但 `~/.jkcodingagent/chat-images/{session-title-slug}/` 下的**实际文件不会被自动删除**。删除/清空会话前，必须先按 `workspace_id` 查询全部图片路径并删除文件，再删数据库记录。
2. **工具产物文件**：`dispatcher_tool_artifacts` 指向的产物文件同样需显式清理。
3. **项目删除**：`project_delete` 命令（`project/storage.rs`）在同一事务内遍历该项目全部会话执行与 `delete_project_session` 相同的级联清理，并删除项目行；提交后 best-effort 清理聊天图片文件与项目仓库内应用自有目录（`.jkcodingagent/browser-profile/`、`.jkcodingagent/local_env/`）。config.toml / mcp.json 可能随仓库共享给团队，保留不删。
4. **通用约定**：任何与会话绑定的文件资源（图片、附件、缓存），在会话删除/清空时必须同步清理文件系统，不能只清 DB。
