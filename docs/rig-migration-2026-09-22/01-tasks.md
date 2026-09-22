# rig 迁移重构任务清单（2026-09-22）

> 目标：移除 `src-tauri/src/agent/` 下自实现的 Agent 运行时（LLM provider、运行循环、
> 工具注册表/执行管线），全面改用 [rig](https://github.com/0xPlaygrounds/rig)
> **rig-core v0.42.0**（crates.io 当前最新稳定版，Rust 2024 edition，MIT）。
> 禁止兼容层/双实现并存——旧实现删除，功能以 rig 推荐方案重建。

## 0. rig-core v0.42.0 API 核实结论（以 `~/.cargo/registry/src/*/rig-core-0.42.0/` 源码为准）

**注意：docs.rs 缓存页展示的 `agent`/`AgentBuilder`/`PromptHook`/`multi_turn` 是旧版 API，
0.42.0 已整体移除高层 Agent——官方哲学改为 "Portable contracts for agent runtimes"：
rig 提供契约（模型/消息/工具/流式/内存），运行时循环由应用基于契约组合。**

已核实的真实 API（rig 0.42）：

| 能力 | rig 0.42 类型/入口 | 源码位置 |
|------|--------------------|----------|
| OpenAI 兼容客户端（自定义 base_url） | `providers::openai::CompletionsClient::builder().api_key(k).base_url(u).build()` → `client.completion_model(name)` | `providers/openai/client.rs`、`client/mod.rs`（`ClientBuilder` 有 `api_key/base_url/http_client/build`） |
| 模型调用契约 | `completion::CompletionModel`：`completion(request)` + `stream(request)`；`completion_request(prompt)` 得 `CompletionRequestBuilder`（preamble/messages/tools/temperature/max_tokens/additional_params/tool_choice） | `completion/request.rs:659` |
| 消息 | `Message::System{content}` / `User{Vec<UserContent>}` / `Assistant{id, Vec<AssistantContent>}`；`UserContent::{Text,ToolResult,Image,Audio,Video,Document}`；`AssistantContent::{Text,ToolCall,Reasoning,Image}` | `completion/message.rs` |
| 流式 | `StreamedAssistantContent::{Text, ToolCall{internal_call_id}, ToolCallDelta, Reasoning, ReasoningDelta, Final(StreamFinal), Unknown}`；`StreamedUserContent::ToolResult`；openai completions 流解析 `reasoning_content`/`reasoning` 增量（满足前端 `AssistantThinkingDelta`） | `streaming/mod.rs`、`providers/openai/completion/streaming.rs` |
| 用量 | `completion::Usage`（input/output/total/cached/cache_creation/reasoning tokens，`Add/AddAssign`） | `completion/request.rs:536` |
| 工具契约 | `tool::PortableTool`（`NAME`/`Args`/`Output`/`Error`，`description()`+`parameters()`+`call(args)`，context-free） | `tool/portable.rs` |
| 动态工具 | `tool::PortableDynamicTool::new(name, desc, params, callback)`（Arc 闭包，dyn 友好——MCP 桥与内置工具的统一可执行形态）；`portable_tool_definition(&tool)` 取定义 | `tool/portable.rs` |
| 工具错误 | `tool::ToolExecutionError`（`refused()`=审查门禁拒绝、`with_retryable()`、`with_model_feedback()`） | `tool/result.rs` |
| 工具输出 | `tool::ToolOutput::text/json/content` + `IntoToolOutput` | `tool/output.rs` |
| 内存（可选） | `memory::{ConversationMemory, InMemoryConversationMemory, Compactor}` | `memory.rs` |
| 其他 | `rig_derive::rig_tool` 宏（feature `derive`，默认开）；crate 内**无** ToolSet/Agent/Hook | `lib.rs` |

**迁移设计（rig 推荐方案映射）**：
- LLM I/O 全部走 `CompletionModel`（删除 `llm/` 的 reqwest+SSE 自解析）。
- 工具按 `PortableTool` 编写（类型化、context-free、可测试），经一个局部
  `erase()` 适配为 `PortableDynamicTool` 组成工具面；MCP 动态工具直接构造
  `PortableDynamicTool`（rmcp 调用包进回调）。删除 registry/spec/broker。
- 运行时循环（事件分发/落库/审查门禁/压缩/取消/vision 切换）是我们基于 rig
  契约组合的 runtime——`StreamedAssistantContent` 增量 → `AgentEvent`（前端契约不变）；
  审查门禁拒绝 = 工具结果 `ToolExecutionError::refused` 回灌；vision 切换 =
  自定义 `CompletionModel` 委托包装（rig 文档化扩展点），按请求是否含图选 chat/vision 模型。
- `max_tokens`/temperature/附加参数走 `CompletionRequestBuilder`；`enable_thinking`
  等 OpenAI 方言参数走 `additional_params`。

## 1. 现状盘点（zg 探索结论）

**规模**：`src-tauri/src/agent/` 共 173 个 .rs 文件、51,630 行。
`OpenAiCompatProvider` 引用约 30 文件；`agent::llm::` 导入 102 处。

**自实现 Agent 运行时（本次移除对象）**：

| 模块 | 职责 | rig 替代方案 |
|------|------|--------------|
| `agent/llm/`（provider/protocol/request/models） | reqwest+SSE 自解析 OpenAI 兼容流式客户端 | `openai::CompletionsClient` + `CompletionModel` |
| `agent/run_loop/`（core/agent_loop/types） | `RunLoopAgent` trait + 多轮工具循环 + 流式分发 | 新 `agent/runtime/` 薄循环：消费 rig 流、执行 rig 工具、发 `AgentEvent`（types.rs 事件契约保留） |
| `agent/tools/registry.rs`、`spec.rs`、`broker.rs`、`capability.rs`、`surface.rs` | 注册表/schema 校验/能力仲裁 | `PortableTool`/`PortableDynamicTool` 工具面；路径规范化/审查门禁保留为 runtime 策略 |
| `agent/agents/`（plain_chat/project/architecture） | 三类 RunLoopAgent | 三个工厂：装配 (CompletionModel, 工具面, preamble) 交给统一 runtime |
| `agent/sub_agent/runtime*` | 子智能体循环 | 同一 runtime 嵌套；`call_sub_agent` 为 PortableDynamicTool |
| 附属调用点：`summary*`、`ssh_review.rs`、`graph/verifier.rs`、`scm/git/commit_message.rs`、`python_runner.rs`、`commands/model_commands.rs`、`tools/builtin/analyze_image.rs` | 一次性 LLM 请求 | `model.completion_request(..).build()` + `model.completion(..)` |

**保留不动**：`agent/db/`（仅出口类型换 rig `Message`/`Usage`）；`agent/commands/` + `AgentEvent`
事件契约（前端零改动）；`agent/graph/`（ACP 外部子进程编排，rig 无等价物；仅 verifier 的
LLM 调用迁移）；`mcp/` 注册表（桥接入 rig 工具面）；PTY/browser/russh。

**关键耦合**：`db/messages.rs:64 to_llm_message()`、`db/token_usage.rs`（`LlmUsage`）依赖 llm 类型。

## 2. 任务清单

### Phase 0 — 依赖基线
- [x] **T0.1** `src-tauri/Cargo.toml` 加入 `rig-core = "0.42"`；`cargo check` 绿。✅ 2026-09-22（注：0.42 无 `rmcp` feature，MCP 桥用 `PortableDynamicTool` 自包）

### Phase 1 — rig 适配层（新模块 `agent/rig_ext/`，此阶段不删旧代码）
- [ ] **T1.1** 模型工厂 `rig_ext/model.rs`：用途槽位（chat/vision/summary/review）解析 →
  `CompletionsClient`+模型；`PurposeSwitchingModel`（实现 rig `CompletionModel`，按请求
  是否含图片委托 chat/vision 模型）；请求构建助手（preamble/messages/tools/max_tokens/
  additional_params 注入 enable_thinking 等方言参数）。参照 `agents/plain_chat/mod.rs:146 apply_settings_v2` 的解析语义（库条目权威、凭据回退规则保持不变）。
- [ ] **T1.2** 消息桥 `rig_ext/message.rs`：`db::DispatcherMessageRecord` ↔ rig `Message`
  （替代 `db/messages.rs:64 to_llm_message`）；`chat-image://` 段 → `UserContent::Image`
  （读取 `chat_images` 落盘文件转 base64，保持「上限 3 张、跨迭代去重」语义，见 `llm.rs attach_turn_tool_images`）。
- [ ] **T1.3** 运行时循环 `rig_ext/loop.rs`：消费 `model.stream(request)` 的
  `StreamedAssistantContent` → `Channel<AgentEvent>`（seq 计数、ToolPlanned/Started/Finished
  配对、思考增量）；工具结果落库 + 压缩（迁入 `common::persist_tool_result_with_compression`
  逻辑）；`Usage` 聚合 → UsageTracker；cancel_rx 协作取消；循环上限错误。

### Phase 2 — 工具层迁移（每组独立文件，可并行；工具 authored as `PortableTool`，deps 下沉为构造参数）
- [ ] **T2.1** 文件系统组：`tools/builtin/filesystem*`、`search*`（含 grep_fallback）、`working_directory.rs`。原 `ToolContext` 的 workspace/白名单/规范化下沉为构造参数。
- [ ] **T2.2** 命令执行组：`local_zsh*`/`shell.rs`、`ssh.rs`、`ssh_memo.rs`、`sync_directory.rs`。SSH 审查门禁保留在 runtime 执行入口（拒绝 → `ToolExecutionError::refused`）。
- [ ] **T2.3** 多媒体/杂项：`browser*`、`fetch_image.rs`、`image_generation.rs`、`image_edit.rs`、`analyze_image.rs`（改走 T1.1 vision 模型）、`run_tool_program.rs`+`tools/program/*`、`architecture_run.rs`、`graph_plan_report.rs`、`submit_graph.rs`。
- [ ] **T2.4** MCP 桥：`mcp/` 注册表动态工具 → `PortableDynamicTool` 回调包装（Global/Project 作用域合并与 TOCTOU 复核语义不变）。

### Phase 3 — Agent 运行时迁移（删 `run_loop/` 与旧 agents 壳）
- [ ] **T3.1** plain_chat：工厂装配 + T1.3 循环；vision 槽位切换（PurposeSwitchingModel）、tool_batch 语义、分类级 allowed_tools/系统提示过滤保留。
- [ ] **T3.2** project 编排器：迁移；`submit_graph` 协议动作保留（工具回显 + runtime 拦截收口 GraphSubmitted）。
- [ ] **T3.3** architecture：迁移；program schema/validate 逻辑挂到 rig 工具与 runtime。
- [ ] **T3.4** sub_agent runtime：同一循环嵌套执行；trace 事件/滑窗裁剪（窗口预算 = contextWindow×4×1/2）保留。

### Phase 4 — 附属 LLM 调用点（可并行）
- [ ] **T4.1** summary 体系：`summary.rs`/`summary/tool_summary.rs`（15s 超时+规则兜底）、`commands/session_metadata.rs`（标题/关键字）。
- [ ] **T4.2** 其余：`ssh_review.rs`、`scm/git/commit_message.rs`、`python_runner.rs`、`graph/verifier.rs`、`commands/model_commands.rs`（连通性测试）。

### Phase 5 — 清理收口
- [ ] **T5.1** 删除 `agent/llm/`、`agent/run_loop/`（types.rs 的 AgentEvent/AgentTurn 挪入 runtime 保留）、`tools/registry.rs`、`spec.rs`、`broker*`、`capability.rs`、`surface.rs` 及全部旧引用；全仓 `OpenAiCompatProvider`/`AgentTool`/`RunLoopAgent`/`ChatMessage` 零命中（`zg query --rg` 穷尽验证）。
- [ ] **T5.2** 全量验证：`cargo check`/`cargo test`/`cargo clippy` 绿；`pnpm contract:check`、`pnpm lint`、`pnpm test`、`pnpm build` 绿；更新 `AGENTS.md`（架构表/新增工具流程/schema 策略章节同步 rig 方案）。

## 3. Phase 2 合并备注（子智能体回报的偏差与遗留）

**合并提交**：T2.1=`rig/t21`(3fc1084) T2.2=`rig/t22`(869b548) T2.3a=`rig/t23a`(7385447) T2.3b=`rig/t23b`(21b8052) T2.4=`rig/t24`(756eb4f)，文件零冲突。

**有意的语义偏差（Phase 3 承接）**：
1. 旧 `ToolResult::success_data` 的结构化 data 载荷不再产出——rig `ToolOutput` 文本/JSON 为唯一通道；产物落库由 `rig_ext/tool_result` 承担。
2. **SSH/命令审查门禁（ssh_review 链路）整体未随工具迁移**——按设计移入 Phase 3 runtime `ToolExecutionPolicy`（拒绝 = `ToolExecutionError::refused`）。T3 必须重建：AI 审查调用、未配置审查即拦截、服务器豁免、拦截审计/台账、with_confirm_guidance、MCP 工具的审查覆盖。
3. 工具结果策略需在 Phase 3 组装工具面时挂载：命令类工具 `COMMAND_FORCE_COMPRESS_AFTER_CHARS`=12000，MCP 工具（`mcp__` 前缀）default_compress=true + 5000。
4. `run_tool_program` 数据面由编排器工厂注入（`program_tool(deps, data_plane)`）；并行只读表迁移为静态表。
5. analyze_image/browser_visual_analyze 只用 vision 槽位规格（凭据回退规则已在 `resolve_purpose_specs` 内置，语义等价旧回退链）。
6. 取消映射：`ToolExecutionError::cancelled`；可恢复错误保持 `Ok(ToolOutput::text("错误：…"))` 或 `other().with_retryable(true)`。
7. `sync_directory` 进度事件 `toolCallId` 暂为 null，待 Phase 3 策略层接线。

## 4. Phase 3 进度（T3 基建已完成，agent 装配待续）

**已完成（提交 05b3de9 / 7e91863，`cargo check`+`cargo test --lib`(692)+`clippy` 全绿）**：
1. **审查门禁就地恢复**（Phase 2 留白已闭合）：新增 `rig_ext/review.rs`
   （`RigReviewContext`：审查配置 + 会话标题 + 用户任务 + 执行者任务 + 对话
   上下文；`build_payload` 对齐旧 `review_context`）；`RigToolDeps.review` 字段；
   local_zsh / ssh_exec / sync_directory / MCP 桥四处恢复 fail-closed 审查
   （未配置即拒、服务器豁免、审查异常拦截、拦截写审计与命令台账、
   `with_confirm_guidance` 文案、审计条目 review 字段）。
2. **工具调用台账**：`rig_ext/tools/run_record.rs`（`dispatcher_tool_runs`
   创建/启动/收尾 + ToolRunUpdated 广播 + 策略元数据；未收录工具名标记
   `registered=false`），并迁移参数准备（schema 默认值注入 + Draft 2020-12
   校验 + 错误摘要，对齐旧 `ToolRegistry::prepare_input`）。
3. **执行策略三段式**：`ToolExecutionPolicy` 扩展为
   `before_call`（门禁 + 台账开始）/`execute`/`after_call`（台账收尾）；
   新增 `rig_ext/loop/app_policy.rs::AppToolExecutionPolicy`：台账 →
   取消检查 → 参数校验 → Dangerous 拒绝 → ReviewRequired 通用审查 →
   统一超时（`unified_timeout=false` 跳过的语义保留）。
   循环侧错误分类对齐旧 `ToolStatus` 词表（succeeded/recoverable_error/
   fatal_error/cancelled），致命语义经 `with_code("fatal")` 声明并在结果
   落库后中止 run（子智能体委派失败将用之）。

**待续（下一轮）**：
- **T3.4 子智能体**：rig 循环版 `SubAgentRuntime`（复用 `model.stream()` +
  工具面 + 策略；保留 trace 事件、滑窗裁剪 `context_window×4×1/2`、
  重试升级 force_final、整体超时与并行只读批）；`call_sub_agent` /
  `list_sub_agents` / `notify_user_progress` 迁为 PortableDynamicTool。
- **T3.1 plain_chat 装配**：新 agent（槽位规格 → 模型 / 工具面组装 +
  allowed_tools 过滤 / 系统提示 + 子智能体快照 / 历史加载 / 跑循环）+
  `state/mod.rs`、`commands/run_commands.rs` 接线 + 删除旧 PlainChatAgent。
- 备注：`agent/tools/spec.rs`（策略表）被新运行时代码引用，Phase 5 需
  将其迁入 `rig_ext/tools/`；`#![allow(dead_code)]` 在接入后移除。

## 5. T3.1 / T3.4 完成记录（提交 b3dfc07 / f25809f）

**已完成**：
- **T3.4 子智能体**：`rig_ext/sub_agent/{runner,context,events,tools}.rs`
  ——rig `CompletionModel::stream()` 驱动的独立循环（滑窗裁剪、整体超时+父取消
  转发（run 级取消通道在工具面构建前建立，工具在构造期捕获接收端）、单次请求
  120s 超时、失败重试升级 `force_final_response`、并行只读批、
  结果 32k 头尾截断、`SubAgentEvent`/轨迹缓冲）；`call_sub_agent` /
  `list_sub_agents` / `notify_user_progress` 为 `PortableDynamicTool`。
  旧 `sub_agent/{tool,runtime*}` 已删除。
- **T3.1 普通聊天**：`rig_ext/agents/{mod,plain_chat}.rs` ——槽位规格 → 模型、
  工具面（exec+media+MCP+子智能体工具，按允许列表过滤）、系统提示（配置+分类+
  系统时间（逐轮）+子智能体/MCP 清单+备忘录纪律+运行工作目录块）、历史加载、
  `run_rig_loop` 执行；`state::build_plain_chat_agent` / 新
  `run_chat_turn_skeleton` / `dispatcher_send_chat_agent_message` 接线；
  工具清单枚举切到新工具面（`tool_catalog`/`static_tool_catalog`）。
  旧 `agents/plain_chat/` 已删除。
- **端到端验证**（`rig_ext/loop/tests.rs`，rig 官方 mock 模型 + 真实临时 DB +
  `Channel` 事件通道）：流式增量事件序列、工具 Planned/Started/Finished 配对、
  三轮消息落库形状（assistant(工具调用)/tool/assistant）、用量落库（primary）、
  未注册工具回灌可恢复错误而非中断。
- **迁移期 allow**：`agent/mod.rs` 顶部 `#![allow(dead_code)]`（旧执行路径在
  Phase 5 删除前产生死代码告警）。**Phase 5 必须删除本 allow**。

**有意偏差（需用户知悉）**：
1. 主循环工具执行为**串行**（旧聊天对只读工具做 ≤4 并发批）。只读并发批在
   子智能体运行时保留；聊天路径并发批待 Phase 5 前补齐或评估。
2. 父聊天工具面不含 `notify_user_progress`（旧注册表把它并入 chat 面，实际
   调用恒失败——该工具只在子智能体上下文有意义）；子智能体面自带该工具。
3. 空响应错误文案改用 rig 运行时默认诊断（model/finish_reason/思考字符数/
   completion_tokens），不再回显原始 SSE 响应体（旧实现回显 4000 字符原文）。
4. 系统提示除「系统时间」外不再逐轮重建（配置/分类/子智能体清单/MCP 清单在
   本轮装配期快照；旧实现逐轮重建）。子智能体清单本就在 run 入口预热一次，
   MCP 清单刷新改为 run 级。

## 6. T3.2 完成记录（提交 d1aa279 / 7b679ec）

- **协议工具宿主拦截**（`rig_ext/loop/protocol.rs`）：`ProtocolToolHandler`
  注入 `RigLoopHooks`；循环在批量执行前拦截协议工具（命中则不跑壳工具回调），
  批量结束后按「协议动作 > 可重试错误 > 最终答复」收口（对齐旧
  `resolve_loop_outcome` 的优先级），收口文案由处理器 `render_outcome` 合成。
- **编排器**（`rig_ext/agents/{project,project_prompt,project_tools,project_submit,project_report}.rs`）：
  提示词/Harness 目录/节点统计、工作区边界校验、四个模型可见入口
  （run_tool_program + message/submit_graph/graph_plan_report 壳）、
  `submit_graph` 全流程（解析→继承→校验→落 graph_plans→广播）、
  `graph_plan_report` 报告、`message` 最终答复；数据面 grant = 策略过滤后的
  read_file/list_dir/glob/grep，经 `program_tool` 的 granted 文案告知模型。
- **接线**：`state::build_run_agent`、`dispatcher_send_project_agent_message`
  → 新 `run_orchestrator_turn_skeleton`；设置页项目工具清单改由
  `static_runtime_tool_catalog` 枚举（与 grant 同源）；旧 `agents/project/`
  整目录删除。
- **顺带迁移**：`scm/git/commit_message.rs` 改走 rig 模型工厂
  （`resolve_purpose_specs` + `completion`，15s 超时 + 关闭思考链），
  不再依赖旧 provider 解析函数。
- **测试**：新增协议收口端到端测试（动作优先收口 / 可重试错误继续循环）；
  全套 695 测试绿、clippy 0 告警、Tauri 命令契约通过。

## 7. Phase 5 完成记录（提交 e8b735f）

**已删除（旧自实现 Agent 运行时）**：`agent/llm.rs` + `agent/llm/`（自研
OpenAI 兼容客户端/SSE 解析/请求构造）、`agent/run_loop/`（运行骨架与
`RunLoopAgent`/`AgentRunAdapter`）、`agent/agents/`（旧三类 Agent 残留）、
`agent/tools/`（注册表/broker/capability/surface/context/result/runtime/
旧 MCP 桥/builtin 工具）、`src/tools/image_generator.rs`、
`agent/prompt/runtime_workspace.rs`、`agent/state/tool_catalog.rs`，
以及迁移期 `agent/mod.rs` 的 `#![allow(dead_code)]`。

**契约类型归位（存活必需）**：
- `agent/db/contract.rs`：落库 JSON 契约（`ChatMessage` 及其 parts/图片源、
  `OutboundToolCall`/`FunctionCall`、`LlmUsage`/`LlmPromptTokensDetails`、
  `MAX_TURN_TOOL_IMAGE_ATTACHMENTS`、`messages_contain_images`）——库中既有
  数据的读写形状，与「模型调用」无关。
- `agent/rig_ext/events.rs`：前端事件与收口契约（`AgentEvent`/`AgentTurn`）。
- `agent/rig_ext/tools/spec.rs`：工具策略表（台账元数据/审查门禁/超时/结果
  策略的唯一来源）+ 上限常量（`MAX_TOOL_CALLS_PER_BATCH` 等）。
- `agent/rig_ext/models.rs`：模型列表拉取（`/models` 两种方言）。

**语义等价性说明（有意偏差，均已记录）**：
1. 主循环工具执行串行（旧聊天对只读工具有 ≤4 并发批；子智能体路径保留并发批）。
2. `UsageTracker` 去掉「子智能体调用期间暂停计时」（token/秒 在
   `call_sub_agent` 期间会偏低，不再人为剔除暂停时长）。
3. 空响应诊断文案改用 rig 运行时默认（不再回显原始 SSE 原文）。
4. 系统提示快照化（除系统时间逐轮刷新）。
5. 父聊天工具面不含旧的 `notify_user_progress`（该工具只在子智能体上下文有意义）。

**验证**：`cargo check` / `cargo test`（537 通过）/ `cargo clippy --all-targets`
（0 告警）；`pnpm lint` / `pnpm test`（569 通过）/ `pnpm build` /
`pnpm contract:check`（115 命令）/ `pnpm styles:report`（0 无引用）全绿；
全仓 `agent::llm` / `OpenAiCompatProvider` / `AgentTool` / `ToolRegistry` /
`CapabilityBroker` 零引用（仅注释中的历史说明）。

## 8. 进度记录

| 任务 | 状态 | 执行者 | 完成时间 | 备注 |
|------|------|--------|----------|------|
| T0.1 | ✅ 完成 | 主智能体 | 2026-09-22 | rig-core 0.42.0，无 rmcp feature；commit 70b68f3 |
| T1.1 | ✅ 完成 | agent-4 | 2026-09-22 | rig_ext/model.rs；a84f9ee |
| T1.2 | ✅ 完成 | agent-4 | 2026-09-22 | rig_ext/message.rs；a84f9ee |
| T1.3 | ✅ 完成 | agent-4 | 2026-09-22 | rig_ext/loop.rs+tool_result.rs；a84f9ee |
| T2.1 | ✅ 完成 | agent-5 | 2026-09-22 | fs/（read_file/list_dir/glob/grep）；24 测试绿 |
| T2.2 | ✅ 完成 | agent-6 | 2026-09-22 | exec/（local_zsh/ssh_*/ssh_memo/sync_directory）；审查门禁移交 T3 |
| T2.3 | ✅ 完成 | agent-7（media）+ agent-8（program） | 2026-09-22 | media/ 12 工具 + program/ DSL 执行器 |
| T2.4 | ✅ 完成 | agent-9 | 2026-09-22 | mcp.rs 桥（执行期重解析替代 spec hash 复核） |
| T3.1 | ✅ 完成 | 主智能体 | 2026-09-22 | b3dfc07；旧 PlainChatAgent 已删除，端到端测试绿 |
| T3.2 | ✅ 完成 | 主智能体 | 2026-09-22 | d1aa279；旧 agents/project 已删除 |
| T3.3 | ✅ 完成 | 主智能体 | 2026-09-22 | e3c42a3；画布 DSL 随迁，旧 ArchitectureAgent 已删除 |
| T3.4 | ✅ 完成 | 主智能体 | 2026-09-22 | 04eedff/b3dfc07；旧 sub_agent runtime/tool 已删除 |
| T4.1 | ✅ 完成 | 主智能体 | 2026-09-22 | af8eb31；summary 迁入 rig_ext（旧 summary/ 已删） |
| T4.2 | ✅ 完成 | 主智能体 | 2026-09-22 | af8eb31/72bf00e；审查/验收/Python/连通性/提交信息全部迁移 |
| T5.1 | ✅ 完成 | 主智能体 | 2026-09-22 | e8b735f；旧运行时零残留 |
| T5.2 | ✅ 完成 | 主智能体 | 2026-09-22 | cargo+pnpm 全量验证通过；AGENTS.md 已更新 |
