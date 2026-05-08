# Nezha Agent 执行链路文档

> 本文档梳理了 Aha Coding (Nezha) 项目中，从用户创建任务到 Agent 执行完毕的完整链路，涵盖前端 UI 交互、Tauri 命令调用、Rust 后端 PTY 进程管理、输出流回传、会话发现与状态同步等关键环节。

---

## 目录

1. [架构总览](#1-架构总览)
2. [执行路径一：直接任务执行](#2-执行路径一直接任务执行)
3. [执行路径二：Dispatcher 调度执行](#3-执行路径二dispatcher-调度执行)
4. [核心数据结构](#4-核心数据结构)
5. [事件系统](#5-事件系统)
6. [关键常量与配置](#6-关键常量与配置)
7. [文件索引](#7-文件索引)

---

## 1. 架构总览

```
┌─────────────────────────────────────────────────────────────────┐
│                        前端 (React 19 + TypeScript)               │
│                                                                   │
│  App.tsx  ──→  ProjectPage  ──→  DispatcherChat / NewTaskView    │
│    │                  │                                            │
│    │          ┌───────┴────────┐                                   │
│    │          │                │                                   │
│    ▼          ▼                ▼                                   │
│  useTerminalManager    TerminalView (xterm.js)                    │
│  (agent-output 监听)    (PTY 输出渲染)                              │
└───────────────────────┬─────────────────────────────────────────┘
                        │  invoke() + listen()
                        │  Tauri IPC
┌───────────────────────┴─────────────────────────────────────────┐
│                      后端 (Tauri 2 + Rust)                        │
│                                                                   │
│  app/mod.rs  ── 命令注册中枢                                      │
│    │                                                               │
│    ├── task_runtime/pty.rs  ── run_task, resume_task, cancel_task │
│    │       │                                                       │
│    │       ├── 创建 PTY (portable_pty)                             │
│    │       ├── 构建命令行 (Claude CLI / Codex CLI)                  │
│    │       ├── spawn_pty_reader() ── 后台读取 PTY 输出              │
│    │       ├── spawn_exit_monitor() ── 轮询子进程退出状态           │
│    │       └── session watcher ── 监视 JSONL 会话文件              │
│    │                                                               │
│    ├── task_runtime/session.rs ── 会话发现与状态解析               │
│    │                                                               │
│    ├── agent/commands.rs ── Dispatcher 对话与子进程管理            │
│    ├── agent/runtime.rs ── DispatcherAgent 核心调度逻辑            │
│    ├── agent/llm.rs ── OpenAI 兼容 LLM 流式调用                    │
│    ├── agent/config.rs ── Agent 配置与 Prompt 模板                 │
│    └── agent/tools/ ── 工具注册表 (内置 + 委派 + MCP)              │
│                                                                   │
│  shared/state.rs ── TaskManager (PTY 句柄 + 会话映射)              │
└─────────────────────────────────────────────────────────────────┘
```

项目有**两条主要执行路径**：
- **路径一**：用户在 UI 中直接创建任务 → 调用 Claude/Codex CLI → PTY 输出流回前端
- **路径二**：用户在 DispatcherChat 中对话 → Dispatcher LLM Agent 决定委派子任务 → 触发路径一

---

## 2. 执行路径一：直接任务执行

### 2.1 任务创建 → 状态初始化

**入口**：用户在 NewTaskView/TodoTaskView 中编写 prompt，选择 agent 和 permission mode，点击提交。

```typescript
// src/App.tsx:211-263 — handleSubmitTask
function handleSubmitTask(project, { prompt, agent, permissionMode, images, immediate }) {
  const task: Task = {
    id: `${Date.now()}`,
    projectId: project.id,
    prompt,
    agent,              // "claude" | "codex"
    permissionMode,     // "ask" | "auto_edit" | "full_access"
    status: immediate ? "pending" : "todo",
    createdAt: Date.now(),
  };
  // 1. 更新 state → 持久化到磁盘
  setTasks(prev => [task, ...prev]);
  persistProjectTasks(task.projectId, next, showToast);

  // 2. 如果不是立即执行 (保存为 todo)，直接返回
  if (!immediate) return task.id;

  // 3. 立即执行：重置终端 buffer，调用 run_task
  tm.resetTaskTerminal(task.id);
  invokeRunTask(task, project.path, images);
  return task.id;
}
```

**关键点**：
- 任务状态从 `todo` 或 `pending` 开始（取决于是否勾选"立即运行"）
- `resetTaskTerminal` 清空前端 xterm.js buffer，为新任务准备干净的输出区域
- 调用 `invoke("run_task", ...)` 将控制权交给 Rust 后端

### 2.2 前端发起 Tauri 命令

```typescript
// src/App.tsx:169-184 — invokeRunTask
function invokeRunTask(task: Task, projectPath: string, images: string[]) {
  invoke("run_task", {
    taskId: task.id,
    projectPath,
    prompt: task.prompt,
    agent: task.agent,
    permissionMode: task.permissionMode,
    images,
    cols: tm.terminalSizeRef.current.cols,  // 终端列数
    rows: tm.terminalSizeRef.current.rows,  // 终端行数
  }).catch((err) => {
    // 如果 invoke 失败，直接在终端写入错误
    tm.writeErrorToTerminal(task.id, `\r\n错误：${msg}\r\n`);
    updateTaskStatus(task.id, "failed", undefined, msg);
  });
}
```

**关键点**：
- `invoke()` 是 Tauri 的 IPC 调用，异步将参数序列化传给 Rust 端
- 传入了终端尺寸 (`cols`/`rows`)，用于创建匹配的 PTY
- 如果 invoke 本身失败（如找不到 agent 程序），前端会在终端显示错误并标记 `failed`

### 2.3 Rust 后端：run_task 命令

整个 `run_task` 是核心，位于 `src-tauri/src/task_runtime/pty.rs:409-571`。

#### 步骤 1：创建 PTY

```rust
// src-tauri/src/task_runtime/pty.rs:427-434
let pair = native_pty_system()
    .openpty(PtySize {
        rows: rows.unwrap_or(50),
        cols: cols.unwrap_or(220),
        pixel_width: 0,
        pixel_height: 0,
    })
    .map_err(|e| e.to_string())?;
```

使用 `portable_pty` 库创建伪终端 (PTY) 对。PTY 包含一个 master 端（程序读取输出、写入输入）和一个 slave 端（提供给子进程作为其 stdin/stdout/stderr）。

#### 步骤 2：处理附件图片

```rust
// src-tauri/src/task_runtime/pty.rs:437-456
let image_paths = save_task_images(&project_path, &task_id, &images.unwrap_or_default())?;

// 读取项目配置，拼接 prompt_prefix
let config = read_project_config(project_path.clone())?;
let base_prompt = if config.agent.prompt_prefix.is_empty() {
    prompt.clone()
} else {
    format!("{}\n{}", config.agent.prompt_prefix, prompt)
};

// 将图片路径追加到 prompt 末尾
let final_prompt = if image_paths.is_empty() {
    base_prompt
} else {
    format!("{}\n\n[Attached images]\n{}", base_prompt, image_paths.join("\n"))
};
```

- 图片 data URL 被解码写入 `.jkcodingagent/attachments/<taskId>/` 目录
- 项目的 `prompt_prefix` 配置被拼接到任务 prompt 前
- 图片文件路径被追加到 prompt 末尾，Agent CLI 可通过文件工具读取

#### 步骤 3：构建并启动 Agent 进程

```rust
// src-tauri/src/task_runtime/pty.rs:458-494
let agent_bin = get_agent_bin_checked(&agent)?;  // 查找 claude/codex 可执行文件路径

let mut cmd = if is_codex {
    let mut c = CommandBuilder::new(&agent_bin);
    c.arg("--");
    c.arg(&final_prompt);
    c
} else {
    let mut c = build_claude_cmd(&agent_bin, &permission_mode);
    // Claude >= 2.1.87 使用 --session-id 预指定会话 ID
    if let Some(ref sid) = pre_session_id {
        c.arg("--session-id");
        c.arg(sid);
    }
    c.arg("--");
    c.arg(&final_prompt);
    c
};
cmd.cwd(&project_path);
setup_env(&mut cmd);  // 注入 login shell 环境变量 + TERM=xterm-256color

let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
```

**Claude 权限模式映射**（`build_claude_cmd` at line 387-404）：

| 前端 permissionMode | CLI 标志 |
|---|---|
| `ask` | `--permission-mode default` |
| `auto_edit` | `--permission-mode acceptEdits` |
| `full_access` | `--dangerously-skip-permissions` |

#### 步骤 4：注册到 TaskManager 并发送 "running" 状态

```rust
// src-tauri/src/task_runtime/pty.rs:496-505
task_manager.insert_pty_handles(&task_id, pair.master, writer, child);
// → shared/state.rs:50-66 将 master/writer/child 存入 HashMap

let _ = app.emit(
    "task-status",
    serde_json::json!({ "task_id": task_id, "status": "running" }),
);
```

TaskManager 是 Tauri 托管状态，使用 `parking_lot::Mutex` 保护的多个 HashMap：

```rust
// src-tauri/src/shared/state.rs:19-37
pub struct TaskManager {
    pty_masters: Mutex<HashMap<String, SharedPtyMaster>>,
    pty_writers: Mutex<HashMap<String, SharedPtyWriter>>,
    child_handles: Mutex<HashMap<String, SharedChildHandle>>,
    codex_sessions: Mutex<HashMap<String, CodexSessionInfo>>,
    claude_sessions: Mutex<HashMap<String, ClaudeSessionInfo>>,
    // ...
}
```

#### 步骤 5：启动 PTY 输出读取线程

```rust
// src-tauri/src/task_runtime/pty.rs:508-567
let (session_tx, session_rx) = std::sync::mpsc::channel::<String>();

// 启动会话监视器（监听 JSONL 文件获取 session_id）
spawn_status_session_watcher(
    app.clone(), task_id.clone(), project_path.clone(), is_codex, session_rx, pre_session_id,
);

// 启动 PTY reader 线程
spawn_pty_reader(
    app.clone(), task_id.clone(),
    "agent-output",  // 事件名 → 前端 listen("agent-output")
    "task_id",       // payload 中的 ID key
    PtyEmitMode::Batched { ... },
    reader,          // PTY master 端的 reader
    Some(session_tx),
    None, None, None,
);
```

`spawn_pty_reader` 的架构（line 234-342）：

```
┌─ spawn_pty_reader ──────────────────────────────────────────┐
│                                                               │
│  tokio::task::spawn_blocking 主线程                          │
│  ┌─ reader.read(&mut buf) 循环 ───────────────────────────┐ │
│  │  - 从 PTY master 读取原始字节                            │ │
│  │  - 处理 UTF-8 边界（不完整的跨缓冲区字节序列）           │ │
│  │  - 通过 sync_channel 发送给 worker 线程                  │ │
│  │  - 同时通过 session_tx 转发给 session watcher            │ │
│  └───────────────────────────────────────────────────────┘ │
│                                                               │
│  worker 线程（emit batcher）                                 │
│  ┌─ rx.recv_timeout(flush_interval) ──────────────────────┐ │
│  │  - 按 flush_interval 周期性 flush                        │ │
│  │  - 达到 max_batch_bytes 立即 flush                       │ │
│  │  - 通过 app.emit("agent-output", payload) 发送到前端    │ │
│  └───────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────┘
```

**输出流的反压链**：Claude/Codex 进程 → OS PTY buffer → reader.read() → sync_channel(容量=32) → worker → Tauri emit → JS listener → requestAnimationFrame drain

#### 步骤 6：启动进程退出监视器

```rust
// src-tauri/src/task_runtime/pty.rs:568
spawn_exit_monitor(app, task_id, project_path, is_codex);
```

`spawn_exit_monitor`（line 345-384）在后台轮询子进程状态（每 100ms），进程退出后：
1. 等待 session watcher 完成会话注册（最长 500ms）
2. 调用 `finalize_task_exit` 清理资源并发送最终状态

### 2.4 输出流如何到达前端

```
Claude/Codex stdout
    │
    ▼
PTY slave ──→ PTY master
    │
    ▼
reader.read(&mut buf)          [pty.rs:299-331]
    │
    ├──→ session_tx.send()     [pty.rs:316-318] → session watcher (解析 JSONL)
    │
    └──→ sync_channel.send()   [pty.rs:319-324]
              │
              ▼
         worker 线程           [pty.rs:258-296]
              │
              │  batch + flush
              ▼
         app.emit("agent-output", { task_id, data })
              │
              ▼
    ┌─ 前端 listen("agent-output") ─────────────────────┐
    │  useTerminalManager.ts:157-168                      │
    │                                                     │
    │  - 累积 chunks 到 pendingOutputs Map                │
    │  - requestAnimationFrame 批量 drain                 │
    │  - 写入 taskBufferRef (内存 buffer，上限 10MB)      │
    │  - 写入 xterm.js terminal (显示在 TerminalView)     │
    └───────────────────────────────────────────────────┘
```

### 2.5 输入如何到达 Agent

用户在 TerminalView 中打字 → xterm.js onData 回调：

```
TerminalView (xterm.js)
    │  onData(data)
    ▼
ProjectPage → onInput(taskId, data)
    │
    ▼
useTerminalManager.handleInput  [useTerminalManager.ts:224-233]
    │  invoke("send_input", { taskId, data })
    ▼
pty.rs:send_input               [pty.rs:677-683]
    │  task_manager.write_to_pty(&task_id, data.as_bytes(), false)
    ▼
TaskManager.write_to_pty        [shared/state.rs:68-79]
    │  获取 pty_writers[task_id] → writer.write_all(data)
    ▼
PTY master → PTY slave → Agent 进程 stdin
```

### 2.6 任务恢复 (resume_task)

```typescript
// src/App.tsx:187-209 — invokeResumeTask
function invokeResumeTask(task, projectPath) {
  const sessionId = task.agent === "claude"
    ? task.claudeSessionId
    : task.codexSessionId;

  invoke("resume_task", {
    taskId: task.id, projectPath, agent: task.agent,
    sessionId, prompt: task.prompt, permissionMode: task.permissionMode,
    cols, rows,
  });
}
```

Rust 端 (`pty.rs:590-674`)：
- Claude: 使用 `claude --resume <session_id>` 恢复会话
- Codex: 使用 `codex resume <session_id>` 恢复会话
- 走和 `run_task` 相同的 PTY reader + exit monitor 流程

### 2.7 任务取消与停止

```rust
// src-tauri/src/task_runtime/pty.rs:574-587
#[tauri::command]
pub async fn cancel_task(task_manager: State<'_, TaskManager>, task_id: String) {
    request_task_termination(&task_manager, &task_id, TaskTerminationIntent::Cancelled)
}

pub async fn stop_task(task_manager: State<'_, TaskManager>, task_id: String) {
    request_task_termination(&task_manager, &task_id, TaskTerminationIntent::Stopped)
}
```

`cancel_task` 和 `stop_task` 的区别仅在于最终状态标签：
- `Cancel` → 最终状态 = `"cancelled"`
- `Stop` → 最终状态 = `"stopped"`

两者都调用 `task_manager.kill_child(task_id)` 杀死子进程。清理逻辑在 `finalize_task_exit` 中统一处理。

---

## 3. 执行路径二：Dispatcher 调度执行

### 3.1 概述

Dispatcher 是一个内置的 LLM Agent（运行在 `agent/runtime.rs` 中），它：
1. 接收用户在 `DispatcherChat` 中的消息
2. 调用配置的 LLM API（OpenAI 兼容）进行对话和推理
3. 可以通过工具调用 `dispatch_claude` / `dispatch_codex` 将编码任务委派给终端 Agent
4. 接收子进程的执行结果，继续协调或返回最终结论

### 3.2 前端入口：DispatcherChat

```typescript
// src/components/DispatcherChat.tsx — 核心 dispatch 逻辑
// 用户发送消息 → invoke("dispatcher_send_message", {
//   workspaceId, projectPath, content,
//   onEvent: new Channel<AgentEvent>()  // 用于接收流式事件
// })
```

关键：使用 Tauri 的 `Channel` 类型实现服务端到客户端的流式事件推送，支持：
- `assistantDelta`：LLM 流式输出的 token 增量
- `toolStarted` / `toolFinished`：工具调用的开始与结束
- `dispatchProposed`：等待用户批准的子任务提案
- `finished`：完成整个对话轮次

### 3.3 Rust 后端：DispatcherState 与 run 循环

#### DispatcherState 初始化

```rust
// src-tauri/src/agent/commands.rs:43-63
impl DispatcherState {
    pub fn new(project_mcp_registry: ProjectMcpRegistry) -> Result<Self> {
        let config = DispatcherAgentConfig::load()?;
        let db = DispatcherDb::new(config.db_path.clone())?;
        let mut agent = DispatcherAgent::new(config, project_mcp_registry);
        // 恢复保存的 LLM 设置
        if let Ok(Some(settings)) = db.get_settings() {
            agent.apply_settings(&settings);
        }
        Ok(Self { agent: tokio::sync::Mutex::new(agent), db, ... })
    }
}
```

在 Tauri 中以托管状态注入：

```rust
// src-tauri/src/app/mod.rs:23
.manage(dispatcher_state)
```

#### dispatcher_send_message 命令

```rust
// src-tauri/src/agent/commands.rs:360-394
#[tauri::command]
pub async fn dispatcher_send_message(
    state: State<'_, DispatcherState>,
    app: AppHandle,
    workspace_id: String,
    project_path: String,
    content: String,
    on_event: Channel<AgentEvent>,  // 流式事件通道
) -> Result<AgentTurn, String> {
    // 1. 异步生成会话标题
    spawn_session_title_update(&state, &app, &workspace_id, &content);

    // 2. 应用最新设置
    if let Ok(Some(settings)) = state.db.get_settings() {
        agent.apply_settings(&settings);
    }

    // 3. 运行 LLM 对话循环
    let run_handle = state.begin_run(&workspace_id);
    let result = agent.run(
        &state.db, &workspace_id, &project_path,
        &content, on_event, run_handle.cancel_rx,
    ).await;
    state.finish_run(&workspace_id, run_handle.generation);
    result
}
```

#### DispatcherAgent.run() 核心循环

```rust
// src-tauri/src/agent/runtime.rs — DispatcherAgent::run()
// 核心逻辑（简化）：
// 1. 从 DB 加载历史消息
// 2. 将用户消息追加到消息列表
// 3. 构建 system prompt（SOUL.md + USER.md + TOOLS.md + MCP context + 子进程状态）
// 4. 循环调用 LLM:
//    a. 发送消息 + 工具定义
//    b. 解析 LLM 响应（文本 or 工具调用）
//    c. 如果是工具调用 → 执行工具 → 返回结果 → 继续循环
//    d. 如果是文本 → 返回给用户
// 5. 最多迭代 max_tool_iterations (200) 次
```

#### 工具系统

```rust
// src-tauri/src/agent/tools/mod.rs:15-21
impl ToolRegistry {
    pub fn default_tools(project_mcp_registry: ProjectMcpRegistry) -> Self {
        let mut tools = builtin::builtin_tools();       // read_file, write_file, glob, grep, exec, etc.
        tools.extend(delegation::delegation_tools());    // dispatch_claude, dispatch_codex, continue_*, exit_*
        Self::new(tools).with_dynamic_provider(mcp::mcp_tool_bridge(project_mcp_registry))
    }
}
```

#### 子进程委派流程

当 LLM 调用 `dispatch_claude` 或 `dispatch_codex` 时：

```
1. delegation.rs 解析工具参数 → 生成 DispatchAgent, task_prompt, description
2. runtime.rs 通过 ProtocolBatchState 验证：
   - 该 agent 是否已有活跃子进程？→ 阻止重复 dispatch
   - 同轮是否已有 pending dispatch？→ 阻止重复
3. 发送 AgentEvent::DispatchProposed 事件到前端
   ↓
4. 前端 DispatchChat 显示 DispatchApprovalDialog：
   - 显示任务摘要和完整的 task_prompt
   - 用户可编辑 task_prompt
   - 批准 → onApprove(dispatchId, taskPrompt)
   - 拒绝 → onReject(dispatchId)
   ↓
5. 前端调用 handleSubmitTask({
     prompt: taskPrompt,
     agent, permissionMode,
     immediate: true,  // 立即执行
     dispatcherDispatchId: dispatchId,
     dispatcherSessionId: workspaceId,
     dispatcherDescription: description,
   })
   ↓
6. 进入路径一的 run_task 流程
   - 注意 dispatcher_dispatch_id 被传入，用于关联子进程
```

#### LLM 流式调用

```rust
// src-tauri/src/agent/llm.rs:269-396 — OpenAiCompatProvider::chat_stream()
pub async fn chat_stream(
    &self, messages: &[ChatMessage], tools: &[ToolDefinition],
    enable_multimodal: bool,
    mut on_delta: impl FnMut(&str),
) -> Result<LlmResponse> {
    // 1. POST /v1/chat/completions (SSE streaming)
    // 2. 逐行解析 SSE chunks
    // 3. 提取 content delta → on_delta(token)
    // 4. 累积 tool call fragments (id, name, arguments)
    // 5. 返回完整的 LlmResponse { content, tool_calls, usage }
}
```

### 3.4 子进程结果反馈

子进程运行完毕后，Dispatcher 通过以下方式接收结果：

```typescript
// DispatcherChat 中监听 "dispatcher-subprocess-idle" 事件
// → 当子进程每轮完成时，回调 dispatcher_continue_after_dispatch
invoke("dispatcher_continue_after_dispatch", {
  workspaceId, projectPath,
  dispatchResult: "...",  // 子进程输出摘要
  dispatchState: "round_completed" | "process_done" | "process_failed" | "process_cancelled",
  onEvent: new Channel<AgentEvent>(),
});
```

Rust 端 (`commands.rs:543-569`) 调用 `agent.continue_after_dispatch()`，将子进程执行结果注入到 LLM 对话上下文中，让调度 Agent 根据结果决定下一步。

---

## 4. 核心数据结构

### 4.1 Task（TypeScript）

```typescript
// src/types.ts:22-41
interface Task {
  id: string;                   // 时间戳 ID
  projectId: string;
  name?: string;                // AI 生成的标题
  prompt: string;               // 用户输入的 prompt
  agent: "claude" | "codex";
  permissionMode: "ask" | "auto_edit" | "full_access";
  status: TaskStatus;           // 见下方状态机
  createdAt: number;
  claudeSessionId?: string;     // 会话发现后回填
  claudeSessionPath?: string;   // JSONL 文件路径
  codexSessionId?: string;
  codexSessionPath?: string;
  dispatcherSessionId?: string; // Dispatcher 关联字段
  dispatcherDispatchId?: string;
  dispatcherDescription?: string;
}
```

### 4.2 任务状态机

```
                      ┌─────────┐
                      │  todo   │ ← 保存但不执行
                      └────┬────┘
                           │ 用户点击"运行"
                           ▼
                      ┌─────────┐
                ┌─────│ pending │
                │     └────┬────┘
                │          │ run_task() 成功
                │          ▼
                │     ┌─────────┐      用户输入 stdin
                │     │ running │◄─────────────┐
                │     └────┬────┘              │
                │          │ Agent 请求输入     │
                │          ▼                   │
                │   ┌──────────────┐ ──────────┘
                │   │input_required│
                │   └──────────────┘
                │
                │          ┌──────────┐
                ├─────────►│ stopped  │ ← 用户点击 stop
                │          └──────────┘
                │
                │          ┌──────────┐
                ├─────────►│ cancelled│ ← 用户删除运行中任务
                │          └──────────┘
                │
                │          ┌──────────┐
                └─────────►│  done    │ ← 正常退出 (exit 0)
                           └──────────┘
                           ┌──────────┐
                           │  failed  │ ← 异常退出 (exit ≠ 0)
                           └──────────┘
```

### 4.3 TaskManager（Rust）

```rust
// src-tauri/src/shared/state.rs:19-37
pub struct TaskManager {
    pty_masters: Mutex<HashMap<String, SharedPtyMaster>>,    // PTY master 端
    pty_writers: Mutex<HashMap<String, SharedPtyWriter>>,    // PTY 写入器
    child_handles: Mutex<HashMap<String, SharedChildHandle>>, // 子进程句柄
    codex_sessions: Mutex<HashMap<String, CodexSessionInfo>>, // Codex 会话
    claude_sessions: Mutex<HashMap<String, ClaudeSessionInfo>>,// Claude 会话
    claimed_session_paths: Mutex<HashSet<String>>,             // 已认领的会话路径
    dispatcher_subprocess_ids: Mutex<HashMap<String, String>>, // task_id → dispatch_id
    task_project_paths: Mutex<HashMap<String, String>>,        // task_id → project_path
}
```

### 4.4 Dispatcher 消息类型

```typescript
// src/types.ts:142-153
interface DispatcherMessage {
  id: string;
  workspaceId: string;
  role: "user" | "assistant" | "tool";
  content: string;
  toolCallId?: string;
  toolName?: string;
  // ...
}
```

---

## 5. 事件系统

Tauri 事件是前后端通信的核心机制。所有事件定义在 `src/App.tsx` 的 `useEffect` 监听器中。

### 5.1 事件一览

| 事件名 | 方向 | Payload | 触发位置 | 监听位置 |
|---|---|---|---|---|
| `task-status` | Rust → JS | `{ task_id, status, failure_reason? }` | `pty.rs:502,641,83-98` | `App.tsx:105-117` |
| `agent-output` | Rust → JS | `{ task_id, data }` | `pty.rs:195-202` | `useTerminalManager.ts:157` |
| `task-session` | Rust → JS | `{ task_id, session_id, session_path }` | `session.rs:27-30` | `App.tsx:119-124` |
| `shell-output` | Rust → JS | `{ shell_id, data }` | `pty.rs:195-202` | ShellTerminalPanel |
| `dispatcher-session-updated` | Rust → JS | `DispatcherSessionRecord` | `commands.rs:263` | DispatcherChat |

### 5.2 task-status 事件生命周期

```
status: "pending"  ──→ 前端 resetTaskTerminal + invoke(run_task)
status: "running"  ──→ 前端开始显示终端输出
status: "input_required" ──→ 前端高亮任务等待用户输入
status: "done" | "failed" | "cancelled" | "stopped"
     ──→ 前端 updateTaskStatus + 清理 inactive buffers
```

---

## 6. 关键常量与配置

### 6.1 PTY 配置

```rust
// src-tauri/src/task_runtime/pty.rs:14-22
const SESSION_WAIT_POLL: Duration = Duration::from_millis(50);
const SESSION_WAIT_MAX: Duration = Duration::from_millis(500);
const PTY_READ_BUFFER_SIZE: usize = 32 * 1024;       // 32KB
const PTY_EMIT_FLUSH_INTERVAL: Duration = Duration::from_millis(16); // ~60fps
const PTY_EMIT_MAX_BATCH_BYTES: usize = 64 * 1024;   // 64KB
const PTY_IDLE_OUTPUT_MAX_BYTES: usize = 256 * 1024;  // 256KB
const PTY_EMIT_CHANNEL_CAPACITY: usize = 32;          // 有界 channel
```

### 6.2 前端 Buffer 配置

```typescript
// src/hooks/useTerminalManager.ts:7-10
const MAX_BUFFER_SIZE = 10 * 1024 * 1024;   // 10MB 内存上限
const MAX_BUFFER_CHUNKS = 256;              // 碎片合并阈值
const DRAIN_FRAME_BUDGET = 128 * 1024;      // 每帧最大 drain 128KB
const MAX_PENDING_TERMINAL_BYTES = 512 * 1024; // 终端未 ready 时临时缓冲上限
```

### 6.3 Dispatcher 配置

```rust
// src-tauri/src/agent/config.rs:122-188
// 默认 LLM：qwen3.6-plus（可通过 MODEL_NAME 环境变量覆盖）
// 默认 summary 模型：deepseek-v4-flash
// max_tool_iterations: 200
// exec_timeout_secs: 60
// temperature: 0.1
// max_tokens: 8192
```

### 6.4 项目配置

```rust
// src-tauri/src/agent/config.rs:139-187
DispatcherAgentConfig {
    root_dir: ~/.jkcodingagent/,
    db_path: ~/.jkcodingagent/jkbot.sqlite3,
    // Prompt 模板文件：
    //   ~/.jkcodingagent/SOUL.md   — 系统角色定义
    //   ~/.jkcodingagent/USER.md   — 用户偏好
    //   ~/.jkcodingagent/TOOLS.md  — 工具使用说明
    //   ~/.jkcodingagent/memory/MEMORY.md — 持久化记忆
}
```

---

## 7. 文件索引

### 前端核心文件

| 文件 | 行数 | 核心职责 |
|---|---|---|
| `src/App.tsx` | 474 | 根组件，状态持有，Tauri 事件监听，`handleSubmitTask`/`invokeRunTask`/`invokeResumeTask` |
| `src/types.ts` | 292 | 所有 TypeScript 类型定义：Task, DispatcherMessage, AgentEvent 等 |
| `src/hooks/useTerminalManager.ts` | 321 | PTY 输出 buffer 管理，`agent-output` 事件监听，xterm.js 写入协调 |
| `src/components/ProjectPage.tsx` | ~800 | 项目主页面，协调子面板、任务列表、DispatcherChat 通信 |
| `src/components/DispatcherChat.tsx` | ~800 | Dispatcher 对话 UI，dispatch 审批对话框，流式消息渲染 |
| `src/components/dispatcherChatView.ts` | — | 流式消息视图状态管理，assistant turn segment 构建 |
| `src/components/TerminalView.tsx` | — | xterm.js 封装，PTY 输出的终端渲染 |
| `src/components/SubProcessTabs.tsx` | — | Dispatcher 子进程 tab 切换 UI |
| `src/components/task-panel/BranchBar.tsx` | 531 | Git 分支切换，分支创建 |

### Rust 后端核心文件

| 文件 | 行数 | 核心职责 |
|---|---|---|
| `src-tauri/src/main.rs` | 6 | 程序入口 |
| `src-tauri/src/lib.rs` | 11 | 模块声明 |
| `src-tauri/src/app/mod.rs` | 128 | Tauri 应用初始化，所有命令注册（128 个 Tauri command） |
| `src-tauri/src/task_runtime/mod.rs` | 3 | task_runtime 模块入口 |
| `src-tauri/src/task_runtime/pty.rs` | 817 | **核心**：`run_task`、`resume_task`、`cancel_task`、`stop_task`、`send_input`、`resize_pty`、`open_shell`、`kill_shell`、`spawn_pty_reader`、`spawn_exit_monitor`、`finalize_task_exit` |
| `src-tauri/src/task_runtime/session.rs` | ~800 | 会话发现：Claude JSONL 监视、Codex rollout JSONL 监视、状态解析（`input_required`/`done`/`failed`）、`spawn_status_session_watcher`、`spawn_resume_session_watcher` |
| `src-tauri/src/shared/mod.rs` | 5 | shared 模块入口 |
| `src-tauri/src/shared/state.rs` | 124 | **TaskManager**：PTY 句柄管理、子进程 I/O、终止意图 |
| `src-tauri/src/agent/mod.rs` | 13 | agent 模块入口 |
| `src-tauri/src/agent/commands.rs` | 711 | **Dispatcher 命令**：`dispatcher_send_message`、`dispatcher_continue_after_dispatch`、`dispatcher_stop_run`、子进程注册/状态同步、会话标题生成与持久化 |
| `src-tauri/src/agent/runtime.rs` | 2141 | **DispatcherAgent**：LLM 对话循环、工具调用协议、子进程生命周期状态机、`run()`/`continue_after_dispatch()` |
| `src-tauri/src/agent/llm.rs` | 726 | OpenAI 兼容 LLM 客户端：`OpenAiCompatProvider`、SSE 流式解析、多模态图片处理、模型列表获取 |
| `src-tauri/src/agent/config.rs` | 213 | `DispatcherAgentConfig`：从环境变量加载配置，Prompt 模板（SOUL/USER/TOOLS）初始化与同步 |
| `src-tauri/src/agent/summary.rs` | ~500 | 工具结果摘要、dispatch 结果摘要、会话标题生成 |
| `src-tauri/src/agent/tools/mod.rs` | 22 | 工具注册表聚合（builtin + delegation + MCP） |
| `src-tauri/src/agent/tools/registry.rs` | — | Tool 注册表核心，支持静态 + 动态 provider |
| `src-tauri/src/agent/tools/builtin.rs` | — | 内置工具：`read_file`、`write_file`、`edit_file`、`list_dir`、`glob`、`grep`、`exec` |
| `src-tauri/src/agent/tools/delegation.rs` | — | 委派工具：`dispatch_claude`、`dispatch_codex`、`continue_*_session`、`exit_*_session` |
| `src-tauri/src/agent/tools/mcp.rs` | — | MCP 工具桥接，将项目 MCP 服务器暴露为 LLM 可调用工具 |
| `src-tauri/src/agent/db.rs` | — | SQLite 数据库操作：消息、会话、设置、工具产物的 CRUD |
| `src-tauri/src/platform/mod.rs` | 8 | 平台模块入口 |
| `src-tauri/src/platform/app_settings.rs` | — | Agent 路径检测、版本检测（`claude --version`）、环境变量获取 |
| `src-tauri/src/project/config.rs` | — | 项目 `.jkcodingagent/config.toml` 管理 |
| `src-tauri/src/project/storage.rs` | — | 项目/任务 JSON 文件持久化 (`~/.jkcodingagent/projects.json`) |
| `src-tauri/src/project/mcp.rs` | — | MCP 状态管理、服务启停、配置解析 |
| `src-tauri/src/workspace/fs.rs` | — | 文件系统操作：目录读取、文件读写、图片预览、文件移动/删除 |
| `src-tauri/src/scm/git.rs` | — | 完整 Git 集成操作 |

---

## 附录：完整调用时序图

```
用户                       React 前端                      Tauri Rust 后端                Agent 进程 (Claude/Codex)
 │                            │                                │                              │
 │  输入 prompt + 点击运行     │                                │                              │
 ├───────────────────────────►│                                │                              │
 │                            │  handleSubmitTask()            │                              │
 │                            │  task.status = "pending"       │                              │
 │                            │  resetTaskTerminal()           │                              │
 │                            │                                │                              │
 │                            │  invoke("run_task", {...})     │                              │
 │                            ├───────────────────────────────►│                              │
 │                            │                                │  create PTY pair             │
 │                            │                                │  save_task_images()          │
 │                            │                                │  build command               │
 │                            │                                │  spawn child process         │
 │                            │                                ├─────────────────────────────►│
 │                            │                                │                              │
 │                            │  emit("task-status",           │                              │
 │                            │    { status: "running" })      │                              │
 │                            │◄───────────────────────────────┤                              │
 │                            │                                │                              │
 │                            │  updateTaskStatus("running")   │                              │
 │                            │                                │                              │
 │                            │     ┌─ spawn_pty_reader ─┐     │                              │
 │                            │     │  - reader loop      │     │                              │
 │                            │     │  - session_tx ──────┼────►│                              │
 │                            │     │  - sync_channel ────┼──┐  │                              │
 │                            │     └─────────────────────┘  │  │                              │
 │                            │                              │  │     Claude stdout/stderr      │
 │                            │                              │◄─┼──────────────────────────────┤
 │                            │  emit("agent-output",        │  │                              │
 │                            │    { task_id, data })        │  │                              │
 │◄───────────────────────────┤◄─────────────────────────────┘  │                              │
 │                            │                                │                              │
 │  (看到终端实时输出)         │  drainPendingOutputs()          │                              │
 │                            │  → xterm.write(data)           │                              │
 │                            │  → pushToBuffer(data)          │                              │
 │                            │                                │                              │
 │                            │              ┌─ session watcher ─┐                            │
 │                            │              │ 寻找 JSONL 文件   │                            │
 │                            │              │ 解析会话 ID       │                            │
 │                            │  emit("task-session",           │                            │
 │                            │    { session_id, session_path })│                            │
 │                            │◄────────────────────────────────┤                            │
 │                            │  updateTaskSession(id, path)    │                            │
 │                            │                                │                              │
 │                            │              ┌─ exit monitor ─┐  │                            │
 │                            │              │  try_wait → exit│  │                            │
 │                            │              │  wait_for_session│ │                            │
 │                            │              │  finalize_task   │  │                            │
 │                            │  emit("task-status",            │                              │
 │                            │    { status: "done"/"failed" }) │                              │
 │◄───────────────────────────┤◄───────────────────────────────┤                              │
 │                            │                                │                              │
 │  看到最终状态 ✓/✗           │  updateTaskStatus("done")      │                              │
 │                            │  removeInactiveTaskBuffers()   │                              │
```

---

> 生成日期：2026-05-08 · 适用版本：当前 `main` 分支
