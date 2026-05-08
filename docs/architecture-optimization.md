# Agent 架构优化方案

> 从架构合理性角度严格审查，仅列出最推荐优化的 3 项。

---

## 1. 消除 DispatcherAgent 与 TaskManager 之间的子进程状态双重追踪

### 问题

子进程生命周期状态同时存储在 `DispatcherAgent.subprocesses`（`src-tauri/src/agent/runtime.rs:172`）和 `TaskManager` 的三个字段中（`src-tauri/src/shared/state.rs:29-36`）：

| TaskManager 字段 | DispatcherAgent 字段 | 用途 |
|---|---|---|
| `dispatcher_subprocess_ids` (L29) | `subprocesses` (L172) | task_id → dispatch_id 映射 |
| `dispatcher_exited_subprocesses` (L33) | `subprocesses` 中各元素的 `status` | 标记已退出的子进程 |
| `dispatcher_force_idle_flags` (L35) | 无对等字段 | 会话 watcher 强制 idle 信号 |

`dispatcher_register_subprocess`（`src-tauri/src/agent/commands.rs:613-628`）**同时写入两处**：

```rust
// commands.rs:622-627
task_manager.dispatcher_subprocess_ids.lock().insert(task_id.clone(), dispatch_id.clone());
let agent_runtime = state.agent.lock().await;
agent_runtime.register_subprocess(&workspace_id, &task_id, &dispatch_id, &agent, &description);
```

状态标记命令（`dispatcher_mark_subprocess_*`）则**只写入 DispatcherAgent**，不更新 TaskManager：

```rust
// commands.rs:632-668 — 这些命令只更新 agent side，不碰 task_manager
dispatcher_mark_subprocess_round_completed → agent.mark_subprocess_round_completed
dispatcher_mark_subprocess_running       → agent.mark_subprocess_running
dispatcher_mark_subprocess_stopped       → agent.mark_subprocess_stopped
dispatcher_mark_subprocess_finished      → agent.mark_subprocess_finished
```

而 `dispatcher_exit_subprocess`（L682-700）又只写入 TaskManager 的 `dispatcher_exited_subprocesses`。

**风险**：两个数据源随时可能不一致。例如 `DispatcherAgent` 认为子进程已完成但 `TaskManager` 中无对应标记，会导致 `continue_after_dispatch` 的结果注入逻辑（runtime.rs:580-716）与 PTY 退出处理（pty.rs:51-101）产生冲突。

### 建议

将 `TaskManager` 中的 `dispatcher_subprocess_ids`、`dispatcher_exited_subprocesses`、`dispatcher_force_idle_flags` 三个字段移除，统一以 `DispatcherAgent.subprocesses` 为唯一真相来源。需要跨模块读取子进程状态的代码（如 session watcher、pty exit handler）通过方法调用或事件查询 DispatcherAgent。

### 收益

消除一类一致性 bug，减少 3 个 Mutex 字段的锁竞争，子进程生命周期可审计性大幅提升。

---

## 2. runtime.rs 中 std::sync::Mutex 的 poison 风险

### 问题

`DispatcherAgent`（`src-tauri/src/agent/runtime.rs:167-172`）的 4 个字段使用 `std::sync::Mutex`：

```rust
provider: Mutex<OpenAiCompatProvider>,     // L167
summary_model: Mutex<String>,              // L168
vision_model: Mutex<String>,               // L169
subprocesses: Mutex<Vec<RegisteredSubprocess>>, // L172
```

所有 12 处访问均使用裸 `unwrap()`：

```rust
// runtime.rs:391
let mut provider = self.provider.lock().unwrap();
// runtime.rs:411
let mut summary_model = self.summary_model.lock().unwrap();
// runtime.rs:515
let mut subprocesses = self.subprocesses.lock().unwrap();
```

而同一个项目中 `TaskManager`（`src-tauri/src/shared/state.rs:1`）使用的是 `parking_lot::Mutex`，不存在 poison 机制。

**风险**：`std::sync::Mutex` 在持有锁的线程 panic 时会进入 poisoned 状态。一旦发生（例如 LLM provider 内部 panic），`unwrap()` 会立即传播 panic，导致 dispatcher agent 整个崩溃。相比之下 `parking_lot::Mutex` 不会 poison，panic 后锁正常释放，其余线程可继续工作。

### 建议

将 `DispatcherAgent` 中 4 个 `std::sync::Mutex` 替换为 `parking_lot::Mutex`，与项目其余部分保持一致。`parking_lot::Mutex` 的 API 完全兼容（`lock()` 返回 `MutexGuard`，无需 `unwrap()`）。

涉及文件仅 `src-tauri/src/agent/runtime.rs` 一处，改动范围约 15 行（导入替换 + 删除 12 处 `unwrap()`）。

### 收益

消除因 LLM 调用 panic 导致整个 dispatcher agent 不可恢复的故障模式。

---

## 3. process_codex_session_line 的可变状态参数应提取为状态机结构体

### 问题

`process_codex_session_line`（`src-tauri/src/task_runtime/session.rs:209-353`）接受 **3 个 `&mut` 引用参数**：

```rust
fn process_codex_session_line(
    app: &AppHandle,
    task_id: &str,
    line: &str,
    project_path: &Path,
    waiting_for_user: &mut bool,              // 当前是否需要等待用户
    pending_confirmation_calls: &mut HashSet<String>, // 待确认的工具调用 ID
    awaiting_user_reply: &mut bool,            // 是否在等用户回复
)
```

这些参数跨越 140+ 行的 match 分支被多处读写：
- `*awaiting_user_reply = true`（L245, L253, L277）
- `pending_confirmation_calls.insert(...)`（L251）/ `.remove(...)`（L269）
- `sync_waiting_for_user(...)` 调用（L256, L279, L284, L295）依赖三个参数的组合状态

**风险**：
1. 调用方（L120-190 区域的 while 循环）需要在每次调用前后维护这些可变变量的正确性
2. 状态转换逻辑散落在 5+ 个 match 分支中，审计困难
3. 新增 JSONL 事件类型时，容易遗漏某个状态的更新

### 建议

提取一个 `CodexTaskState` 结构体，将 `waiting_for_user`、`pending_confirmation_calls`、`awaiting_user_reply` 封装进去，`process_codex_session_line` 改为 `&mut self` 方法：

```rust
struct CodexTaskState {
    waiting_for_user: bool,
    pending_confirmation_calls: HashSet<String>,
    awaiting_user_reply: bool,
}

impl CodexTaskState {
    fn apply_event(&mut self, app: &AppHandle, task_id: &str, event: &CodexEvent) {
        // 所有状态转换集中在此
    }
}
```

改动限于 `src-tauri/src/task_runtime/session.rs` 一个文件，不影响外部接口。

### 收益

可测试性：`CodexTaskState` 可以脱离 PTY/AppHandle 进行纯函数单元测试。可维护性：状态转换逻辑集中在一处，新增事件类型时有编译器引导所有需要更新的位置。

---

## 优先级建议

| 优先级 | 优化项 | 理由 |
|--------|--------|------|
| **P0** | #2 Mutex poison | 影响面小（1 文件），修复成本极低（15 行），但可导致运行时崩溃 |
| **P1** | #1 双重状态追踪 | 架构级隐患，每新增子进程相关功能都在累积债务 |
| **P2** | #3 会话状态机 | 纯内部重构，改善可维护性但不影响当前正确性 |
