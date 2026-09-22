# JKCodingAgent — AGENTS.md

## 项目概述

JKCodingAgent 是一款面向 AI 智能体的现代桌面应用：以「会话（session）」为核心，内置 dispatcher 智能体运行时（多轮工具调用、子智能体、命令审查门禁；项目 Agent 为图编排器——产出执行图 DAG，图节点经 ACP 子进程（claude-agent-acp）执行）、RAG 知识库、嵌入式 Shell / 浏览器 / Python 运行器、文件浏览器、Git 集成与用量记录，外壳为 Tauri 2。

**技术栈：** React 19 + TypeScript + Vite（前端） · Tauri 2 + Rust（桌面壳） · **Tailwind CSS + shadcn 风格组件 + CSS 变量主题**（UI） · Zustand + React Query（状态/数据） · xterm.js（终端） · Shiki（语法高亮） · rusqlite（持久化）

---

## 开发命令

```bash
pnpm dev            # 启动 Vite 开发服务器（端口 1420）
pnpm build          # tsc 类型检查 + Vite 打包
pnpm lint           # 运行 ESLint（--max-warnings 0）
pnpm test           # Vitest（前端纯函数/归一化）
pnpm contract:check # Tauri 命令双向契约检查（后端注册 ↔ 前端调用）
pnpm styles:report  # .ai-* 类定义/引用双向 fail-closed 报告
pnpm tauri dev      # 启动完整桌面应用（自动启动开发服务器）
pnpm tauri build    # 构建生产环境桌面二进制包
```

> 新增功能应同步补单元测试（优先纯函数：路径处理、token 统计、segments 解析、分组/归一化等）；Vitest 用例放在被测模块旁的 `*.test.ts(x)`。

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
├── WelcomePage                      — 主页（聊天 / 项目 / 架构 三视图切换，AppRail 导航）
│   ├── HomeChatPage                 — 独立聊天工作区（不绑定项目）
│   ├── 项目网格                      — 打开 / 删除本地仓库
│   └── ArchitectureView             — 架构画布（Excalidraw）+ 助手聊天
└── ProjectPage                      — 项目工作区
    ├── ContextNav                   — 会话 / 文件 / 变更 / 历史 四页签上下文导航
    ├── SessionPanel                 — 会话列表（搜索 / 新建 / 分页）
    ├── 聊天工作台 (chat-page-v2)      — dispatcher 消息流、工具调用、子智能体、图编排入口
    ├── 图编排 UI (components/graph)    — GraphPlanCard 内联卡片 / GraphPanel 执行图画布 / GraphNodeDrawer 节点详情
    ├── SubAgentExecutionView         — 子智能体执行卡片（阶段/时间线/统计）
    ├── 文件浏览器 (file-explorer)     — FileViewer / LargeFileViewer / 图片预览
    ├── GitChanges / GitHistory       — 变更 / 提交 / 差异
    ├── ShellTerminalPanel            — 嵌入式交互 Shell（xterm.js）
    ├── BrowserPanel                  — 内嵌浏览器（主区单例标签）
    ├── StatusDockBar                 — 底部状态栏（终端 / 浏览器 dock 开关）
    └── AppSettingsDialog             — 应用设置（智能体 / RAG / SSH / 子智能体配置）
```

异步状态由 Tauri 事件驱动（`@tauri-apps/api/event` 的 `listen()`）。Agent 运行事件的类型契约在
`src-tauri/src/agent/rig_ext/events.rs`（`AgentEvent` / `AgentTurn`，字段为前端契约，改动即破坏前端），
由 `rig_ext::r#loop` 在消费 rig 流式事件时发出。当前在用的事件：
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
| `agent/` | dispatcher 智能体核心（基于 **rig-core 0.42 portable contracts**）：`rig_ext/`（运行时：`model`（用途槽位→rig 模型）/`message`（消息桥）/`r#loop`（多轮工具循环 + 三段式执行策略 + 工具台账 + 协议拦截）/`tool_result`（结果落盘与压缩）/`summary`（标题/关键字）/`events`（前端事件契约）/`agents`（三类 Agent 装配 + 协议工具 + 画布 DSL + 提示词）/`sub_agent`（子智能体运行时与工具）/`tools`（rig 工具面：fs/exec/media/program/mcp/spec 策略表 + 参数校验/台账）/`review`（命令审查上下文）/`models`（模型列表拉取））、`graph/`（图编排：定义/校验/执行引擎/`acp_exec` ACP 节点执行器/命令）、`db/`（SQLite schema 与读写 + `contract.rs` 落库 JSON 契约）、`commands/`（Tauri 命令）、`config.rs`（智能体配置 + `~/.jkcodingagent` 初始化）、`ssh_review.rs`（命令安全审查，rig 模型）、`sub_agent/{config,manager,db,commands}.rs`（子智能体配置与持久化） |
| `task_runtime/` | `pty.rs`（PTY 创建/读写）、`session.rs`（会话/输出兜底） |
| `project/` | `storage.rs`（受管项目/会话存储）、`config.rs`（项目配置）、`mcp.rs`（项目级 MCP） |
| `mcp/` | MCP 子系统：`McpScope{Global, Project}` 显式作用域模型——`Global`（`mcp_servers` 全局注册表，所有聊天共享单一快照）与 `Project`（全局 ∪ 项目 `.jkcodingagent/mcp.json`，同名项目覆盖）；`registry.rs`（作用域缓存/合并/工具执行）、`transport.rs`（stdio/streamable_http/unix_socket_http + 诊断）、`project_file.rs`（项目文件读写）、`commands.rs`（Tauri 命令，项目命令前置路径校验） |
| `scm/git.rs` | Git 集成：状态、分支、日志、差异、暂存、提交、推送、拉取 |
| `workspace/` | `fs.rs`（文件读写/列举）、`rope.rs`（大文件切片） |
| `platform/` | `app_settings.rs`（应用级键值配置） |
| `rag/` | RAG sidecar 传输与管理 |
| `ssh_tool/` | SSH 命令执行 + AI 安全审查门禁。传输层为 russh（纯 Rust 异步，无 libssh2/OpenSSL 依赖）；连接池按 `server_id+session_id` 复用 russh `Handle`，并发命令各走独立 channel（协议级隔离，无逐命令互斥锁）；全新建连对网络类瞬态错误（EHOSTUNREACH / ETIMEDOUT / ECONNRESET 等 errno 集合见 `connection.rs`）间隔 1.5s 自动重试一次，认证/密钥类确定性失败不重试；主机密钥 TOFU 指纹为 key blob 的 SHA-256 hex。`memo.rs` 为每台服务器维护运维备忘录文件（`~/.jkcodingagent/ssh-memos/{server_id}.md`，段落式 Markdown，全文 8000 / 单段 4000 字符硬上限，超限拒绝写入），供 `ssh_memo_read` / `ssh_memo_upsert` / `ssh_memo_delete` 工具与设置页读写；服务器删除时随 `save_servers` 级联清理（同事务清主机密钥/审计行 + 提交后删备忘录文件） |
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
- **应用配置的权威源是全局库**（分层原则：应用生命周期配置一律全局一份；只有随项目变化之物放项目目录）：SSH 服务器/主机密钥/审计（`ssh_servers` 等表）、受管项目注册表（`projects` 表）、MCP 全局注册表（`mcp_servers` 表，与项目级 `mcp.json` 并存、同名项目覆盖）、应用级键值配置（`app_config` 表：全局浏览器选项/RAG 配置）。外观主题偏好（system/light/dark）为 `dispatcher_settings.theme`，随 `AhaSettingsV2` 统一经 `aha_get_settings_v2` / `aha_save_settings_v2` 存取。
- 主要表：`dispatcher_settings`、`ssh_servers`/`ssh_host_keys`/`ssh_audit_log`、`projects`、`mcp_servers`、`app_config`、`sub_agents`、`dispatcher_sessions`、`dispatcher_messages`、`dispatcher_session_token_usage`、`dispatcher_tool_artifacts`、`chat_images`、`graph_plans`、`graph_node_runs`、分类、关键字索引、python 运行记录等（schema 见 `agent/db/schema.rs`）
- 模型配置：`dispatcher_settings.model_library` 为唯一权威；用途槽位以 `libraryId` 引用库条目（保存剥离凭据**与容量**、读取回填）。**容量参数同样以库条目为统一数据源**：`maxTokens`（输出预算，未配置 → 请求体省略 max_tokens、由服务端默认预算接管，历史硬编码 8192 已删除）与 `contextWindow`（上下文窗口 tokens，未配置 → 回退 `DEFAULT_CONTEXT_WINDOW_CAPACITY_TOKENS` = 1M；驱动会话容量展示、上下文占用告警与子智能体滑窗裁剪阈值——字符预算 = 窗口 × 4 字符/token × 1/2，见 `agent/rig_ext/sub_agent/context.rs` 的 `context_budget_chars`）。环境变量回退（DASHSCOPE_*/MODEL_NAME 等）默认关闭，仅 `AHA_ALLOW_ENV_PROVIDER=1` 显式开启。

**存储 schema 版本策略（桌面应用基线 + 前向迁移）**

- 当前为 **v5 基线**（`agent/db/schema.rs` 的 `SCHEMA_VERSION`）：应用开发阶段无存量用户，历史 v0→v33 迁移链已按产品决策清除；`init()` 路径为「全新建库到当前形态」「同版本直开」与「存在迁移块的低版本逐级前向迁移」（v1→v2：chat_images 的 message_id 改可空并删除两个未用列，事务内重建表 + 数据全量保留；v2→v3：dispatcher_settings 新增 `theme` 列，并把旧 `app_config` 中 `app_settings` 键的主题偏好搬移进 `AhaSettingsV2.theme`；v3→v4：删除 projects 表死列 `branch`；v4→v5：sub_agent_run_traces 新增可空 `model` 列〔子智能体轨迹记录真实模型，老行 NULL 前端「未记录」兜底〕；各迁移均在迁移前 `VACUUM INTO` 整库快照）。更早的旧开发库直接报错并引导运行 `scripts/reset-dev-data.sh`。
- 后续每次 schema 变更必须同时做两件事：① 更新 `schema.rs` 的基线 DDL（新装库直接得到新形态）；② 递增 `SCHEMA_VERSION` 并在 `init()` 迁移挂载点追加 `if current_version < N` 的事务块（DDL/回填与 `user_version` 推进同事务、幂等可重试）。**禁止改写或删除历史迁移块**——它们是已发布版本用户升级的唯一路径。
- 破坏性迁移（DROP/清空数据）前必须做整库快照备份（参考 `VACUUM INTO` 方案），并保留「备份失败留痕」的兜底。
- 领域模块自管的表（sub_agent / ssh / projects / mcp_servers / app_config）的 DDL 放在各领域的 `ensure_*_tx` 助手中，由 `create_baseline` 统一调用，保持单一出处。

> 修改数据结构时，**必须同步更新 `types.ts`（TS）与对应的 Rust 结构体/SQL schema**——否则新字段在序列化时会被静默丢弃。

---

## 项目配置

应用级与项目级配置、智能体系统提示词/工具集、SSH/RAG/子智能体设置统一在 `AppSettingsDialog` 中编辑，存全局库。项目目录下仅保留随仓库共享的配置（`.jkcodingagent/config.toml` 的 `[git].commit_prompt`）与项目级 MCP（`.jkcodingagent/mcp.json`，同名覆盖全局注册表）。聊天图片统一走 `chat-image://{image_id}` 协议：唯一保存入口 `chat_images::save_image` 落盘 `~/.jkcodingagent/chat-images/{workspace_id}/{image_id}.{ext}` 并登记 `chat_images` 表（用户粘贴、generate_image/edit_image 产物与 fetch_image 下载的 URL 图片共用）；前端 `<img>` 经自定义 `chat-image` URI scheme 直出（`convertFileSrc(id, "chat-image")`），asset 协议仅兜底旧消息里的绝对路径 markdown。LLM 侧：`rig_ext::message::attach_turn_tool_images` 在每次请求前把本轮 assistant/tool 消息文本里引用的 `chat-image://` 附加为当前用户消息的视觉输入（上限 3 张、跨迭代去重），主模型直看工具产图并自动触发 vision 槽位切换（`rig_ext::model::PurposeSwitchingModel` 按请求是否含图在 chat/vision 槽位间委托）。

**设置中心结构（2025 重构后）：** 外壳 `components/AppSettingsDialog.tsx`（左侧栏单层导航 + 内容区两层结构），页面与共享组件在 `components/settings/`：
- `use-aha-settings.ts` — Aha 设置的统一 store + 失焦/变更自动保存管线（debounce 400ms 整体调用 `aha_save_settings_v2`），通过 React Context 提供给各设置页。
- `GeneralPage.tsx` — 「通用」页：外观主题（跟随系统/浅色/深色），即点即生效（立即预览 + 自动保存），存 `AhaSettingsV2.theme`。
- `providers/` — 「模型服务」与「模型用途」页。「模型服务」页（`ProvidersPage.tsx` + `ModelEntryCard.tsx`）按模型调用方式分标签（对话/视觉/图片生成/图片编辑/语音识别/语音合成/向量）维护**分类模型库**（`AhaSettingsV2.modelLibrary`，每条目独立持有 url/apiKey/model/别名/启停用，对话/视觉类目另有容量字段 maxTokens/contextWindow——数字输入失焦提交、留空即缺省，纯函数层在 `model-library.ts`）；「模型用途」页（`PurposesPage.tsx` + `PurposeSelect.tsx`）的下拉选项来自对应分类的库条目，选中后由 `provider-registry.ts` 的 `bindPurpose` 写入携带 `libraryId` 的引用绑定——落库只保留引用（后端剥离 url/apiKey/model 与容量），读取时由库条目回填凭据与容量，库更新后用途自动跟随。最近测试结果等无存储字段的 UI 偏好存 localStorage（`provider-prefs.ts`）。
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
- **设计令牌是 `App.css` 中的 CSS 自定义属性**（`--bg-*`、`--text-*`、`--border-*`、`--accent` 等），Tailwind 颜色在 `tailwind.config.js` 中 alias 到这些变量。应用支持亮色/暗色主题切换（偏好 system/light/dark，暗色令牌由 `App.css` 的 `.dark` 块承载，前端 `lib/theme.ts` 据此切换根节点 `.dark` 类并经 `AhaSettingsV2.theme` 持久化）。新增颜色必须同时维护亮/暗两套令牌值，不要硬编码色值。
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

## 新增 Agent 工具流程（rig 形态）

工具是 rig `PortableDynamicTool`（`rig_ext/tools/` 下的构造器返回 `Vec<PortableDynamicTool>`），
业务逻辑写在同目录文件中，**不再有自实现的工具 trait/注册表**。

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
                // 阻塞 I/O：tokio::task::spawn_blocking；取消：deps.cancel_rx
                // 成功：Ok(ToolOutput::text(...))；失败：Err(ToolExecutionError::{invalid_args,refused,timeout,other})
                //   —— 错误消息以「错误：」开头；可恢复重试用 .with_retryable(true)；
                //      致命（如委派失败，父循环据此中止）用 .with_code("fatal")
            })
        })
}
```

要点：
- 工具的**构造期依赖**来自 `RigToolDeps`（见 `rig_ext/tools/deps.rs`）：workspace/白名单、MCP 作用域、
  DB、SSH、子智能体管理器、取消信号、视觉/图像凭据、审查上下文（`review`）。逐次调用注入的
  `tool_call_id` 用 `deps.tool_call_id` 槽位（`ToolCallSlot`）。
- 需要命令执行/外部效应的工具**自己带 fail-closed 审查**（`deps.review` + `ssh_review::review_shell_command`），
  与 local_zsh / ssh_exec / sync_directory / MCP 桥一致。
- 压缩阈值与内联上限取自 `rig_ext/tool_result.rs`（命令类 12000，默认 5000）；schema 文案必须与
  运行时策略一致（`with_compression_parameters` 的阈值参数）。

### 2. 挂进工具面 — 相应组的入口

按工具的可见面加入对应函数返回值：
- 普通聊天 → `rig_ext/tools/exec/`（+ `media/`）→ 由 `rig_ext/agents/plain_chat.rs` 的 `build_surface` 汇总；
- 编排器数据面（read_file/list_dir/glob/grep）→ `rig_ext/tools/fs/`，并登记进
  `rig_ext/tools/mod.rs` 的 `ORCHESTRATOR_RUNTIME_TOOL_NAMES`；
- 子智能体 → `rig_ext/sub_agent/runner.rs` 的 `build`（继承普通聊天 profile）；
- 协议壳（submit_graph / graph_plan_report / message）→ `rig_ext/agents/project_tools.rs`，
  真实动作在 `RigOrchestratorProtocol`（实现 `rig_ext::r#loop::ProtocolToolHandler`）中拦截。

### 3. 登记策略表 — `src-tauri/src/agent/rig_ext/tools/spec.rs`

在 `TOOL_POLICY_TABLE` 补一行（category/access/safety/timeout/compress/parallel/self-managed）。
该表是**台账元数据、审查门禁判定、统一超时与结果策略的唯一来源**；未收录的工具名走 fail-closed 兜底
（只读+需审查+串行）。

### 4. 工具输出压缩（可选）

压缩是「显式声明（`compress=true`）+ 阈值」双条件驱动：只有声明且原文超过该工具阈值时
`rig_ext/tool_result.rs` 才调用摘要模型（15s 超时），失败/超时回退零 LLM 的
`extract_structured_summary` 规则抽取；未摘要的超长结果按内联上限确定性截断，完整原文进工具产物。
阈值随策略表声明（默认 5000，命令类 12000）。

### 5. 配置（可选）

需要 API Key / URL 等配置时：`config.rs` 的 `DispatcherAgentConfig` 加字段 → `load()` 读取 →
在 `rig_ext/agents/*.rs` 构造 `RigToolDeps` 时传入。

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

- **单个生产文件不应超过 500 行**（前端组件建议 ≤400 行）。超限文件按「变化原因与状态所有权」拆分（入口薄壳 + 领域子模块 + 独立测试模块），不机械按行切片；拆分历史与在账清单（次级超限待拆文件）见 `docs/ui-redesign-2026-09-06/03-tasks.md` 第 16 节「防劣化切片」记录。新增功能若落在超限文件，优先拆分再扩展。

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

**0. 运行中会话的 fail-closed 守卫（先于一切删除/清空/截断）**：`session_delete`、`dispatcher_clear_messages`、`dispatcher_truncate_messages_from`、`project_delete` 在该会话（或项目任一会话）有活动 run 时直接拒绝（`DispatcherState::session_run_is_active`，底座 `ActiveRunStore`）——运行方仍持有消息/用量写入路径，放行会导致对已清理 workspace 的幽灵写入。run 收尾后异步 spawn 的标题/关键字生成同样有存在性校验：`update_session_title` 对不存在行返回 None 不广播；`apply_keyword_actions` 对不存在会话返回 false 不回插。配套 `dispatcher_active_runs` 命令 + 前端 `run-state-reconciliation.ts`：webview 重载后事件通道失联，App 挂载时对账后端仍在跑的会话（补「后台运行中」状态、可停止、轮询至收尾拉全量消息）。新增会话破坏性命令时必须接入同一守卫。

1. **图片文件**：图片按会话目录 `~/.jkcodingagent/chat-images/{workspace_id}/` 布局，删除/清空会话时随 `chat_images` 表记录一起整目录回收（`delete_chat_image_resources` + `remove_chat_image_dir`）。注意：`truncate_messages_from`（regenerate/edit 前置）**有意不删图片文件**——重发复用同一批 image_id；发送前有 `chat_images_validate` 存在性校验兜底。
2. **工具产物文件**：`dispatcher_tool_artifacts` 指向的产物文件同样需显式清理。
3. **项目删除**：`project_delete` 命令（`project/storage.rs`）在同一事务内遍历该项目全部会话执行与 `delete_project_session` 相同的级联清理，并删除项目行；提交后 best-effort 清理聊天图片文件与项目仓库内应用自有目录（`.jkcodingagent/browser-profile/`、`.jkcodingagent/local_env/`）。config.toml / mcp.json 可能随仓库共享给团队，保留不删。
4. **通用约定**：任何与会话绑定的文件资源（图片、附件、缓存），在会话删除/清空时必须同步清理文件系统，不能只清 DB。
5. **消息截断（regenerate / 编辑重发）**：`truncate_messages_from` 除删除消息、工具产物与工具运行外，还须回收被删轮次的副作用——子智能体 trace（`sub_agent_run_traces`，按被删工具运行的 `tool_call_id` 精确匹配）与图编排产物（`graph_plans` 按计划创建时间 ≥ 目标消息时刻截断，`graph_runs`/`graph_node_runs`/`graph_node_activities` 随外键级联）；`python_code_runs`/`chat_images` 行由消息外键级联。有意保留：token 用量（真实消耗记录，回退会让用量分析失真）、会话关键字（聚合权重无消息关联，重发后自然覆盖）、图片文件（重发复用 image_id）。
