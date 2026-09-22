# rig 迁移重构任务清单（2026-09-22）

> 目标：移除 `src-tauri/src/agent/` 下自实现的 Agent 运行时（LLM provider、运行循环、
> 工具注册表/执行管线），全面改用 [rig](https://github.com/0xPlaygrounds/rig)
> **rig-core v0.42.0**（当前最新稳定版，Rust 2024 edition，MIT）。
> 禁止兼容层/双实现并存——旧实现删除，功能以 rig 推荐方案重建。

## 0. 现状盘点（zg 探索结论）

**规模**：`src-tauri/src/agent/` 共 173 个 .rs 文件、51,630 行。

**自实现 Agent 运行时（本次移除对象）**：

| 模块 | 职责 | rig 替代方案 |
|------|------|--------------|
| `agent/llm/`（provider/protocol/request/models，约 1,700 行） | reqwest + SSE 自解析的 OpenAI 兼容流式客户端 `OpenAiCompatProvider` | `rig::providers::openai::CompletionsClient`（builder 自定义 `base_url`），或实现 `CompletionModel` |
| `agent/run_loop/`（core/agent_loop/types，约 1,100 行） | `RunLoopAgent` trait + 多轮工具循环 + 流式分发 | rig `Agent` + `stream_prompt().multi_turn(n)` + `PromptHook` |
| `agent/tools/registry.rs`（1,058 行）+ `spec.rs`（1,105 行）+ `broker.rs`/`capability.rs`/`surface.rs` | `AgentTool` trait、注册表、schema 校验、能力仲裁 | `rig::tool::Tool` trait + `ToolSet`/ToolServer；校验由 rig/serde 接管 |
| `agent/agents/`（plain_chat / project / architecture） | 三类 RunLoopAgent 实现 | rig `AgentBuilder` 工厂函数 + 共享 PromptHook |
| `agent/sub_agent/runtime*` | 子智能体自有循环 | rig Agent 嵌套（call_sub_agent 工具内部构建子 rig Agent） |
| 附属调用点：`summary.rs`/`summary/`、`ssh_review.rs`、`graph/verifier.rs`、`scm/git/commit_message.rs`、`python_runner.rs`、`commands/model_commands.rs`、`tools/builtin/analyze_image.rs` | 直接用 provider 发一次性请求 | rig completion 请求（`model.completion(...).send()` / Extractor） |

**保留不动（非 Agent 框架职责，rig 无等价物）**：
- `agent/db/`（SQLite 持久化；仅把 `to_llm_message()` 等出口从 `llm::ChatMessage` 改为 rig `Message`）
- `agent/commands/` + `AgentEvent` 事件契约（前端零改动；事件由新 PromptHook 发出）
- `agent/graph/` 图编排（节点 = claude-agent-acp 外部子进程，不经进程内运行时；仅 `graph/verifier.rs` 的一次性 LLM 调用随 Phase 4 迁移）
- `mcp/` 作用域注册表（保留；动态工具经桥接进入 rig Agent）
- PTY / browser / russh 传输层

**关键耦合点**：
- `OpenAiCompatProvider` 引用约 30 文件；`agent::llm::` 导入 102 处。
- `db/messages.rs:64 to_llm_message()`、`db/token_usage.rs`（`LlmUsage`）依赖 llm 类型。
- 前端契约 `AgentEvent`（`run_loop/types.rs`）：Started/UserMessage/AssistantStarted/AssistantDelta/AssistantThinkingDelta/ModelSwitched/ToolPlanned/ToolStarted/ToolFinished/ToolRunUpdated/RunUsageUpdated/Finished/Failed——**必须保持逐字段不变**。

**rig v0.42 API 要点（docs.rs 核实）**：
- `openai::Client` builder 支持自定义 base_url；对话补全走 `CompletionsClient`（Chat Completions API，适配 DashScope 等兼容网关）。
- `Agent`：`client.agent(model).preamble(..).tool(..).temperature(..).additional_params(..).build()`；`prompt/chat/completion` + `stream_prompt().multi_turn(n)`。
- `rig::tool::Tool`：关联类型 `Args: Deserialize` / `Output: Serialize` / `Error`，`definition()` + `call(args)`；非 dyn-safe，用 `ToolSet`/`ToolDyn` 收纳；`#[tool]` 宏可选。
- `PromptHook<M>`：`on_completion_call/response`、`on_tool_call`（可 `Skip{reason}`——审查门禁拒绝入口）、`on_tool_result`（落库+压缩入口）、`on_text_delta`/`on_tool_call_delta`（流式 → Channel 事件）、`on_stream_completion_response_finish`。
- `rig::tool::rmcp`（feature `rmcp`）：MCP 工具接入；`tool::server` ToolServer。
- features：default = derive + reqwest + rustls；本项目启用 `rmcp`（复用现有 rmcp 1.8）。
- 待子智能体在 `~/.cargo/registry` 源码中核实：reasoning/thinking delta 的 openai 兼容支持（前端 `AssistantThinkingDelta` 依赖）；vision 图片 content part；取消语义（HookAction 变体）。

## 1. 任务清单

### Phase 0 — 依赖基线
- [ ] **T0.1** `src-tauri/Cargo.toml` 加入 `rig-core = "0.42"`（features `["rmcp"]`，default 保留）；`cargo check` 全绿（仅加依赖，不动代码）。提交基线。

### Phase 1 — rig 适配层（新模块 `agent/rig_ext/`，此阶段不删旧代码）
- [ ] **T1.1** provider 工厂：模型库条目/用途槽位（chat/vision/summary/review/image）→ rig `CompletionsClient` + `CompletionModel`。替换 `resolve_chat_provider`/`resolve_summary_provider` 等解析逻辑的产出物（解析规则不变，产出 rig 类型）。核实 openai builder 自定义 base_url 与 `additional_params` 传 max_tokens 的方式。
- [ ] **T1.2** 消息桥：`db::DispatcherMessageRecord` ↔ rig `completion::Message` 双向转换（替代 `to_llm_message`）；`chat-image://` 图片段 → rig vision content part（上限 3 张语义保持）。
- [ ] **T1.3** 事件桥 `RigEventHook`：实现 `PromptHook`，把流式 delta/工具事件/用量/取消桥到 `Channel<AgentEvent>` + DB 落库 + UsageTracker + cancel_rx。含 seq 计数、ToolPlanned/Started/Finished 配对、工具结果压缩入口（`persist_tool_result_with_compression` 逻辑迁入 `on_tool_result`）。

### Phase 2 — 工具层迁移（每个任务一组文件，可并行）
- [ ] **T2.1** 文件系统工具组 → rig Tool：`tools/builtin/filesystem*`（read/write/edit/list_dir）、`search*`（含 grep_fallback）、`working_directory.rs`。工具结构体持有 workspace/白名单依赖（原 `ToolContext` 字段下沉为构造参数）。
- [ ] **T2.2** 命令执行工具组：`local_zsh*`/`shell.rs`、`ssh.rs`、`ssh_memo.rs`、`sync_directory.rs`。SSH 审查门禁从 broker 迁移到 Hook `on_tool_call`（拒绝 = `ToolCallHookAction::Skip{reason}`）。
- [ ] **T2.3** 多媒体/其他工具：`browser*`、`fetch_image.rs`、`image_generation.rs`、`image_edit.rs`、`analyze_image.rs`（改走 T1.1 vision 模型）、`run_tool_program.rs`+`tools/program/*`、`architecture_run.rs`、`graph_plan_report.rs`、`submit_graph.rs`。
- [ ] **T2.4** MCP 桥接：`mcp/` 注册表动态工具 → rig（优先 `rig::tool::rmcp` 原生集成；不匹配则用动态 Tool 包装器）。保持 Global/Project 作用域合并语义与 TOCTOU 复核。

### Phase 3 — Agent 运行时迁移（删 `run_loop/` 与旧 agents 壳）
- [ ] **T3.1** plain_chat：迁移至 rig Agent 工厂 + T1.3 Hook；保留 vision 槽位切换（含图消息 → vision 模型）、tool_batch 语义、压缩阈值策略。
- [ ] **T3.2** project 编排器：迁移；`submit_graph` 协议动作（Hook 内拦截 → GraphSubmitted 收口）。
- [ ] **T3.3** architecture：迁移；program schema/validate 逻辑保留，挂在 rig 工具/Hooks 上。
- [ ] **T3.4** sub_agent runtime：迁移为 rig Agent 嵌套执行；`call_sub_agent` 成为 rig 工具；trace 事件/滑窗裁剪语义保留（窗口预算仍由库条目 contextWindow 驱动）。

### Phase 4 — 附属 LLM 调用点（可并行）
- [ ] **T4.1** summary 体系：`summary.rs`/`summary/tool_summary.rs`（工具压缩 15s 超时 + 规则兜底）、`commands/session_metadata.rs`（标题/关键字）→ rig 一次性 completion。
- [ ] **T4.2** 其余：`ssh_review.rs`、`scm/git/commit_message.rs`、`python_runner.rs`、`graph/verifier.rs`、`commands/model_commands.rs`（连通性测试）。

### Phase 5 — 清理收口
- [ ] **T5.1** 删除 `agent/llm/`、`agent/run_loop/`、`tools/registry.rs`、`tools/spec.rs`、`tools/broker*`、`capability.rs`、`surface.rs` 及全部旧引用；全仓 `OpenAiCompatProvider`/`AgentTool`/`RunLoopAgent` 零命中（`zg query --rg` 穷尽验证）。
- [ ] **T5.2** 全量验证：`cargo check` / `cargo test` / `cargo clippy` 绿；`pnpm contract:check`、`pnpm lint`、`pnpm test`、`pnpm build` 绿；更新 `AGENTS.md`（架构表、新增工具流程、数据模型章节同步为 rig 方案）。

## 2. 进度记录

| 任务 | 状态 | 执行者 | 完成时间 | 备注 |
|------|------|--------|----------|------|
| T0.1 | 未开始 | - | - | - |
| T1.1 | 未开始 | - | - | - |
| T1.2 | 未开始 | - | - | - |
| T1.3 | 未开始 | - | - | - |
| T2.1 | 未开始 | - | - | - |
| T2.2 | 未开始 | - | - | - |
| T2.3 | 未开始 | - | - | - |
| T2.4 | 未开始 | - | - | - |
| T3.1 | 未开始 | - | - | - |
| T3.2 | 未开始 | - | - | - |
| T3.3 | 未开始 | - | - | - |
| T3.4 | 未开始 | - | - | - |
| T4.1 | 未开始 | - | - | - |
| T4.2 | 未开始 | - | - | - |
| T5.1 | 未开始 | - | - | - |
| T5.2 | 未开始 | - | - | - |
