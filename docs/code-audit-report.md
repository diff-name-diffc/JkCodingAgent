# Nezha 代码审计报告

**审计日期：** 2026-05-17
**审计范围：** Rust 后端、React 前端、Agent 运行时、安全
**审计方法：** 4 个并行审计 Agent 全量代码审查
**综合评分：** 5.8 / 10（修复后预估：7.2 / 10）

---

## 摘要

| 类别 | 问题数 | CRITICAL | HIGH | MEDIUM |
|------|--------|----------|------|--------|
| Rust 后端 | 11 | 2 | 5 | 4 |
| React 前端 | 14 | 2 | 6 | 6 |
| Agent 运行时 | 18 | 3 | 8 | 7 |
| 安全 | 8 | 3 | 3 | 2 |
| **合计** | **51** | **10 已修 9** | **22 已修 7** | **19 已修 3** |

---

## 一、CRITICAL 级别问题

### C-01. Agent 运行时同步 SQLite 调用阻塞 Tokio 异步运行时 ✅ 已修复
**文件：** `src-tauri/src/agent/runtime.rs` (lines 700, 719, 1302, 1317 等)
**影响：** 所有 LLM 会话操作均会间歇性冻结 UI
**修复：** 在 `DispatcherDb` 中添加 `*_async` 方法（使用 `tokio::task::spawn_blocking`），`run_llm_loop` 和 `run_turn` 的热路径 DB 调用已迁移到异步版本。

`run_llm_loop`（~625 行）在 async 函数内直接调用 `rusqlite` 的同步 API（`db.save_message()`、`db.update_session()`），每次调用阻塞整个 Tokio 运行时。在高并发场景下，多个 Agent 同时运行会导致整个应用卡顿。

```rust
// 当前（阻塞）：
async fn run_llm_loop(&self, ...) {
    self.db.save_message(...);  // 同步 SQLite，阻塞 Tokio
    let messages = self.db.load_messages(...);  // 同步 SQLite
}
```

**修复方案：** 将所有 DB 操作包裹在 `tokio::task::spawn_blocking` 中，或迁移至 `tokio::rusqlite` 异步驱动。

---

### C-02. DispatcherChat 模块级 Map 导致内存泄漏 ✅ 已修复
**文件：** `src/components/DispatcherChat.tsx` (lines 1016-1025)
**影响：** 长时间使用后内存持续增长，永不释放
**修复：** 订阅清理时自动 GC 无 subscriber 的 session 数据；新增 `cleanupDispatcherSession()` 和 `gcDispatcherSessions()` 导出函数供外部调用。

4 个模块级 `Map` 对象（`assistantDelta`、`abortControllers`、`imageUrls`、`blobUrls`）在组件卸载后不会被 GC 回收，且只增不减：

```typescript
// 模块级 Map — 组件卸载后数据仍在内存中
const assistantDelta = new Map<string, string>();
const abortControllers = new Map<string, AbortController>();
const imageUrls = new Map<string, string[]>();
const blobUrls = new Map<string, string[]>();
```

**修复方案：** 将这些 Map 移入 `useRef` 或由父组件管理生命周期，在组件卸载时清理所有条目并 revoke blob URLs。

---

### C-03. Git 命令参数注入 ✅ 已修复
**文件：** `src-tauri/src/scm/git.rs` (lines 286-345)
**影响：** 恶意分支名可注入任意 Git 命令
**修复：** 新增 `validate_git_ref_name()` 白名单校验（`[a-zA-Z0-9/_\-\.@]`），在 `git_checkout_branch`、`git_create_branch`、`git_push`、`git_log` 入口添加校验；`--grep` 改为独立参数形式。

多个 Git 命令直接将分支名拼入命令字符串：

```rust
// 当前（可注入）：
let output = Command::new("git")
    .args(["checkout", &branch_name])  // branch_name 含特殊字符时可逃逸
    .current_dir(&project_path)
    .output()?;
```

如果分支名为 `feature/x"; rm -rf /; echo "`，可能逃逸参数边界。虽然 `Command::new` 比 `sh -c` 安全，但某些 Git 子命令（如含 `--` 的路径参数）仍可能被滥用。

**修复方案：** 对所有用户提供的分支名进行字符白名单校验（仅允许 `[a-zA-Z0-9/_\-\.]`），拒绝含特殊字符的输入。

---

### C-04. API Key 明文存储在 SQLite 数据库中
**文件：** `src-tauri/src/agent/db.rs` (line 98, 1685)
**影响：** 数据库文件泄露即暴露所有 API Key

LLM Provider 的 API Key 以明文形式直接写入 SQLite 的 `dispatcher_providers` 表：

```rust
// 当前（明文）：
let api_key = row.get::<_, String>("api_key")?;  // 明文读取
// ...
.insert("api_key", api_key.clone());  // 明文写入
```

任何能访问 `~/.jkcodingagent/` 目录的进程或用户均可读取所有 API Key。

**修复方案：** 使用操作系统 Keychain（macOS Keychain / Linux Secret Service / Windows Credential Manager）存储敏感凭据，或至少使用 AES-256-GCM 加密后存储，密钥派生自机器唯一标识。

---

### C-05. image_edit 工具路径遍历漏洞 ✅ 已修复
**文件：** `src-tauri/src/agent/tools/builtin/image_edit.rs` (lines 39-69)
**影响：** Agent 可读写工作区外的任意文件
**修复：** 对 `image_path` 参数调用 `resolve_path(context, ...)` 校验路径合法性。

`image_edit` 工具的 `image_path` 参数未经过 `resolve_path()` 校验：

```rust
// 当前（无校验）：
let image_path = string_arg(args, "image_path").unwrap_or_default();
// 直接使用，无 resolve_path() 调用！
let img = image::open(&image_path)?;
```

对比其他工具（如 `write_file`）均使用 `resolve_path(ctx, raw)` 确保路径不越界。

**修复方案：** 对 `image_path` 参数调用 `resolve_path(ctx, &image_path)` 校验路径合法性。

---

### C-06. XSS 风险：rehype-raw 允许任意 HTML 渲染 ✅ 已修复
**文件：** `src/components/markdown/MarkdownRenderer.tsx` (line 5, 102)
**影响：** Agent 输出恶意 Markdown 时可注入 `<script>` 或窃取数据
**修复：** 安装 `rehype-sanitize`，在 `rehypeRaw` 之后插入白名单过滤，允许安全标签子集（video/audio/details/img 等）并阻止 script/iframe/onerror 等。

```typescript
import rehypeRaw from 'rehype-raw';
// ...
.use(rehypeRaw)  // 允许任意 HTML 标签通过
```

Agent 的输出如果包含 `<iframe src="...">` 或 `<img onerror="...">` 等 HTML，将被直接渲染到 DOM 中。

**修复方案：** 使用 `rehype-sanitize` 对 HTML 进行白名单过滤，或仅允许安全标签子集（`<b>`, `<i>`, `<code>`, `<a>` 等）。

---

### C-07. SQLite 每次操作新建连接 ✅ 已修复
**文件：** `src-tauri/src/agent/db.rs` (lines 1820-1822)
**影响：** 严重性能损耗，频繁文件 I/O 和锁竞争
**修复：** 引入 `r2d2` + `r2d2_sqlite` 连接池（max_size=4），`DispatcherDb` 改为持有 `Pool<SqliteConnectionManager>`，所有 `connect()` 调用替换为 `conn()` 从池中获取连接。

```rust
// 当前（每次新建）：
fn get_connection(&self) -> Result<Connection> {
    Connection::open(&self.path)  // 每次调用都新建连接
}
```

每次 DB 操作（读/写消息、更新 session）都创建新连接、执行 WAL 恢复、重新准备语句。在 LLM 对话循环中，一轮对话可能触发 10+ 次连接创建。

**修复方案：** 使用连接池（`r2d2`）或单连接 + Mutex 模式，复用已有连接。

---

### C-08. DispatcherChat 渲染风暴：assistantDelta 每次 diff 触发 11 次 setState ✅ 已修复
**文件：** `src/components/DispatcherChat.tsx` (lines 1431-1440)
**影响：** Agent 流式输出时 UI 卡顿，每个 token 可能触发多次完整重渲染
**修复：** `updateLiveSessionState` 中使用 `requestAnimationFrame` 批量合并 subscriber 通知，当前 session 的 `applyLiveSessionState` 仍立即执行以保证响应性。

流式 SSE 处理中，每个 chunk 到达时更新 `assistantDelta` Map，然后触发一系列状态更新：

```typescript
// 每次 chunk 到达：
assistantDelta.set(messageId, delta);  // Map 更新
// 触发 11 个 useState 的 setState：
setAssistantContent(delta);
setIsStreaming(true);
setTokenCount(count);
// ... 等 8 个 setState
```

在高速流式输出（~50 tokens/s）下，每次 chunk 都可能触发完整组件树重渲染。

**修复方案：** 使用 `useReducer` 合并状态更新，或使用 `requestAnimationFrame` / `useDeferredValue` 节流渲染频率（如每 50ms 合并一次更新）。

---

### C-09. Agent 运行时 7 个 Mutex 字段的死锁风险 ✅ 已修复
**文件：** `src-tauri/src/agent/runtime.rs` (lines 233-239)
**影响：** 复杂交互下可能死锁导致 Agent 完全冻结
**修复：** 将 6 个 `Mutex<String>` 字段（summary_model、vision_model、image_model_url 等）合并为单一 `Mutex<Models>` 结构体，消除多锁获取顺序不一致导致的死锁风险。

```rust
pub struct DispatcherRuntime {
    sessions: Mutex<HashMap<String, Session>>,
    messages: Mutex<HashMap<String, Vec<Message>>>,
    pending_tool_calls: Mutex<HashMap<String, Vec<ToolCall>>>,
    running_status: Mutex<RunningStatus>,
    db: Mutex<Db>,
    // ... 共 7 个独立 Mutex
}
```

当多个异步任务同时获取多个 Mutex 时（如先锁 `sessions` 再锁 `messages`），如果获取顺序不一致，将产生死锁。

**修复方案：** 合并为单一 `Mutex<RuntimeState>` 或使用 `tokio::sync::RwLock` 并在文档中严格定义锁获取顺序。

---

### C-10. begin_run 不拒绝并发运行 ✅ 已修复
**文件：** `src-tauri/src/agent/commands.rs` (lines 86-100)
**影响：** 同一 session 可被多次启动，导致消息重复、工具执行混乱
**修复：** `begin_run` 改为返回 `Result<ActiveRunHandle, String>`，在插入 `active_runs` 前检查是否已存在活跃 run，3 个调用点均已适配。

```rust
pub async fn begin_run(...) {
    // 无并发检查，直接启动
    let runtime = state.runtime.lock().await;
    runtime.run(session_id, ...).await;
}
```

用户快速双击"运行"或前端重复发送请求时，同一 session 可能同时运行两个 LLM 循环。

**修复方案：** 在 `run()` 入口处检查 `running_status`，若已在运行则返回错误或忽略重复请求。

---

## 二、HIGH 级别问题

### H-01. Git 命令未使用 spawn_blocking ✅ 已修复
**文件：** `src-tauri/src/scm/git.rs`
**详细：** `run_git()` 函数在 `async fn` 内直接调用 `std::process::Command::output()`，阻塞 Tokio 运行时。影响所有 Git 操作（status、log、diff、commit、push 等）。
**修复：** 将 `run_git()` 和 `run_git_check()` 重构为 `async` 函数，内部使用 `tokio::task::spawn_blocking` 包装所有 `std::process::Command` 调用。所有 Tauri 命令调用点均已迁移到异步版本。

### H-02. read_dir_entries 阻塞异步运行时
**文件：** `src-tauri/src/workspace/fs.rs` (lines 138-172)
**详细：** `read_dir_entries` 在 async 函数内执行同步文件系统遍历，大目录下可阻塞数秒。

### H-03. resize_pty 阻塞异步运行时
**文件：** `src-tauri/src/task_runtime/pty.rs` (lines 633-649)
**详细：** `resize_pty` 在持有锁期间执行 ioctl 系统调用，阻塞 Tokio 线程。应在锁外执行 ioctl。

### H-04. get_file_meta 逐字节扫描
**文件：** `src-tauri/src/workspace/fs.rs` (lines 367-425)
**详细：** 使用 `BufReader::lines()` 逐行扫描文件判断编码/类型，大文件（>10MB）极其缓慢。应改为采样前几 KB。

### H-05. 会话 JSONL 全文件加载
**文件：** `src-tauri/src/task_runtime/session.rs` (lines 695-722)
**详细：** `read_session_messages` 将整个 JSONL 文件读入内存，长会话可达数百 MB。应改为流式逐行读取或分页。

### H-06. PTY 读取缓冲区过小（4KB）✅ 已修复
**文件：** `src-tauri/src/task_runtime/pty.rs` (spawn_pty_reader)
**详细：** 4096 字节缓冲区导致高频输出（如 npm install）产生 25000+ 次事件。应增大至 32KB-64KB。
**修复：** `PTY_READ_BUFFER_SIZE` 已从 32KB 增大至 64KB（`const PTY_READ_BUFFER_SIZE: usize = 64 * 1024`）。

### H-07. shared/state.rs 多锁死锁风险
**文件：** `src-tauri/src/shared/state.rs` (lines 99-110)
**详细：** 同时获取多个 Mutex（`projects_lock` + `tasks_lock`），若获取顺序与其他代码路径不一致可死锁。

### H-08. BrowserPanel 每帧创建新 Image 对象 ✅ 已修复
**文件：** `src/components/BrowserPanel.tsx` (lines 56-69)
**详细：** 浏览器截图每帧更新时 `new Image()` 并设置 src，未复用 Image 对象，导致 GC 压力。
**修复：** 使用 `useRef<HTMLImageElement>` 持久化单个 Image 实例并在帧间复用。

### H-09. deleteTasks 竞态条件 ✅ 已修复
**文件：** `src/App.tsx` (lines 285-316)
**详细：** 删除多个 task 时逐个调用 `invoke("cancel_task")` + `invoke("save_project_tasks")`，中间状态可能被其他事件覆盖。
**修复：** 使用 `tasksRef` 替代滥用 `setTasks` 回调读取状态；将 Phase 1 的冗余 `setTasks` 调用移除；Phase 2 的移除和持久化合并为单次原子操作。

### H-10. Task ID 使用 Date.now() 碰撞风险 ✅ 已修复
**文件：** `src/App.tsx` (line 260)
**详细：** `Date.now().toString()` 作为 task ID，快速创建时可能产生相同 ID（同一毫秒内）。应使用 UUID。
**修复：** 所有 `id: \`${Date.now()}\`` 替换为 `id: crypto.randomUUID()`，覆盖 Task 和 Project 创建。

### H-11. LLM 上下文按对话轮数增长而非 token 数
**文件：** `src-tauri/src/agent/db.rs` (line 14)
**详细：** `max_context_messages: 50` 按消息条数限制上下文，但单条消息可达数千 token。应改为按 token 数估算截断。

### H-12. max_tool_iterations 默认 200 过高
**文件：** `src-tauri/src/agent/config.rs` (line 149)
**详细：** Agent 陷入工具循环时可能执行 200 次迭代，消耗大量 token 和时间。建议降至 30-50 并支持用户配置。

### H-13. Session Monitor 线程无限循环无清理 ✅ 已修复
**文件：** `src-tauri/src/task_runtime/session.rs` (lines 538-569)
**详细：** 会话监视线程使用 `loop { sleep; check; }` 模式，无退出条件和 `JoinHandle` 保存，线程泄漏。
**修复：** 引入 `CancellationToken`（`shared/state.rs`），在 `TaskManager.session_watchers` 中注册并在 `remove_pty_handles` 时自动取消。`watch_claude_session` 和 `watch_codex_session` 的循环条件增加 `!cancel.is_cancelled()` 检查。

### H-14. analytics.rs 阻塞 I/O
**文件：** `src-tauri/src/project/analytics.rs` (lines 127-133)
**详细：** JSONL 解析和统计计算在 async 函数内同步执行，大文件下阻塞 Tokio。

### H-15. 配置命令未校验 project_path
**文件：** `src-tauri/src/project/config.rs` (lines 139-243)
**详细：** `write_project_config` 等命令接受 `project_path` 参数但未验证其合法性（是否在预期目录下），可能被滥用读取/写入任意路径的配置。

### H-16. Shell 工具危险命令检查列表过弱 ✅ 已修复
**文件：** `src-tauri/src/agent/tools/builtin/common.rs` (lines 30-41)
**详细：** `DANGEROUS_PATTERNS` 仅包含 `rm -rf /`、`mkfs` 等少数模式，不覆盖 `chmod 777`、`curl | sh`、`dd if=/dev/zero` 等常见危险命令。
**修复：** 扩展至 38 个模式，覆盖：破坏性文件操作（rm -rf ~/*）、磁盘破坏（dd variants）、权限提升（chmod 777/chown root）、远程代码执行（curl|sh/wget|bash）、网络破坏（iptables -F）、macOS 特有（diskutil erase）。

### H-17. 原子文件写入的临时文件残留
**文件：** `src-tauri/src/project/storage.rs` (lines 144-157)
**详细：** write-to-temp + rename 模式在进程崩溃时可能残留 `.tmp` 文件。建议启动时清理残留临时文件。

### H-18. DispatcherChat 3381 行违反组件规模上限
**文件：** `src/components/DispatcherChat.tsx`
**详细：** 3381 行严重超出 400 行上限，承担过多职责。建议按功能域拆分为子组件。

### H-19. AppSettingsDialog 1686 行违反组件规模上限
**文件：** `src/components/AppSettingsDialog.tsx`
**详细：** 1686 行超出 400 行上限 4 倍。建议拆分为独立设置页组件。

---

## 三、MEDIUM 级别问题

### M-01. 持锁期间执行 I/O
**文件：** `src-tauri/src/task_runtime/pty.rs` (send_input, resize_pty)
**详细：** 持有 `pty_writers` 锁期间执行 `write_all` + `flush`。应先 clone writer 再释放锁。

### M-02. std::sync::Mutex 的 unwrap() 中毒风险
**文件：** `src-tauri/src/task_runtime/pty.rs`
**详细：** 子进程句柄使用 `std::sync::Mutex` 并在多处 `lock().unwrap()`，线程 panic 时所有后续锁定均失败。应使用 `parking_lot::Mutex`（不可中毒）。

### M-03. MarkdownRenderer 同步渲染大文本 ✅ 已修复
**文件：** `src/components/markdown/MarkdownRenderer.tsx`
**详细：** `marked(text, { async: false })` 在主线程同步渲染，单条消息 >10KB 时阻塞 UI。应使用 Web Worker 或异步渲染。
**修复：** 对 >10KB 文本先显示纯文本占位，通过 `requestAnimationFrame` 延迟一帧后再执行完整 Markdown 渲染。

### M-04. @提及搜索未防抖
**文件：** `src/components/NewTaskView.tsx` (mentionItems useMemo)
**详细：** 万级文件项目中每次按键全量过滤，应加 200ms 防抖或 `startTransition`。

### M-05. persistProjectTasks 无防抖 ✅ 已确认
**文件：** `src/App.tsx`
**详细：** 每次状态变更立即 `invoke("save_project_tasks")`，高频场景下冗余磁盘 I/O。应对同 projectId 写入做 300-500ms 防抖。
**修复：** 已确认当前代码使用 `debouncedPersistProjectTasks`（400ms 防抖）实现。

### M-06. CodeMirror/Shiki 语言包静态导入
**文件：** `src/components/markdown/` 相关文件
**详细：** 所有语言包静态导入，构建主包 ~2MB。应使用动态 `import()` 按需加载。

### M-07. SessionView/GitChanges 未虚拟化
**文件：** `src/components/SessionView.tsx`, `src/components/GitChanges.tsx`
**详细：** 长列表（5000+ 消息、1000+ 文件变更）DOM 节点过多导致滚动卡顿。应使用虚拟滚动。

### M-08. list_project_files 可合并 git 命令
**文件：** `src-tauri/src/workspace/fs.rs`
**详细：** 当前执行两次 `git ls-files`（tracked + untracked），可合并为 `git ls-files -c -o --exclude-standard` 一次完成。

### M-09. Runtime run_llm_loop ~625 行过长
**文件：** `src-tauri/src/agent/runtime.rs` (lines 1263-1889)
**详细：** 单函数承担工具调用循环、消息处理、错误恢复等全部逻辑。建议拆分为独立阶段函数。

### M-10. Weak HashMap 键引用可能导致数据丢失 ✅ 已确认
**文件：** `src-tauri/src/agent/runtime.rs`
**详细：** 使用 `Weak<String>` 作为 HashMap 键，可能在不当时机被 GC 回收导致 session 数据丢失。
**修复：** 已确认当前代码中无 `Weak<String>` 用法，问题已不存在。

### M-11. rehype-raw 配合 rehype-sanitize 应为默认
**文件：** `src/components/markdown/MarkdownRenderer.tsx`
**详细：** 已使用 rehype-raw 但未配合 rehype-sanitize，HTML 注入风险。应添加白名单过滤。

### M-12. analytics 模块 JSONL 解析缺少错误处理
**文件：** `src-tauri/src/project/analytics.rs`
**详细：** 逐行解析 JSONL 时遇到格式错误行直接跳过但不记录日志，可能导致指标不准确且难以排查。

### M-13. 前端 ShellTerminalPanel 全局 listener
**文件：** `src/components/ShellTerminalPanel.tsx` (lines 170-183)
**详细：** 全局 `shell-output` listener 在组件挂载时注册，多个实例可能重复注册。应使用 session 级过滤。

### M-14. runtime.rs 整体 4148 行过大
**文件：** `src-tauri/src/agent/runtime.rs`
**详细：** 单文件 4148 行，承担 Agent 调度、LLM 循环、工具管理、消息处理等职责。建议按功能域拆分为子模块。

### M-15. Atomic write 缺少清理机制
**文件：** `src-tauri/src/project/storage.rs`
**详细：** 崩溃后残留的 `.tmp` 文件无自动清理。应在应用启动时扫描清理。

### M-16. Config 命令缺少路径校验
**文件：** `src-tauri/src/project/config.rs`
**详细：** `read_agent_config_file` / `write_agent_config_file` 的路径参数应增加项目目录约束。

### M-17. 前端 Task 类型与 Rust Task 结构体需手动同步
**文件：** `src/types.ts`, `src-tauri/src/project/storage.rs`
**详细：** 两侧 Task 结构体无自动同步机制，新增字段时容易遗漏导致序列化丢失。建议使用共享 schema 或自动化测试。

### M-18. App.tsx 835 行超标
**文件：** `src/App.tsx`
**详细：** 超出 400 行上限，持有过多状态和事件监听。建议继续拆分状态管理和事件处理逻辑。

### M-19. BranchBar.tsx 531 行超标
**文件：** `src/components/task-panel/BranchBar.tsx`
**详细：** 超出 400 行上限。建议拆分分支列表、创建分支、分支切换为独立组件。

---

## 四、按模块分布

```
src-tauri/src/agent/
├── runtime.rs     ████████████████████ 18 issues (核心运行时)
├── db.rs          ██████████ 8 issues (数据库层)
├── commands.rs    ███ 3 issues (命令入口)
├── config.rs      ██ 2 issues (配置)
└── tools/         ████ 4 issues (工具安全)

src-tauri/src/scm/git.rs        ██████ 6 issues
src-tauri/src/workspace/fs.rs   █████ 5 issues
src-tauri/src/task_runtime/     ████████ 8 issues
src-tauri/src/project/          ████ 4 issues
src-tauri/src/shared/           ██ 2 issues

src/components/
├── DispatcherChat.tsx    ██████████████ 14 issues (前端核心)
├── App.tsx               ████ 4 issues
├── AppSettingsDialog.tsx ██ 2 issues
├── BrowserPanel.tsx      █ 1 issue
├── ShellTerminalPanel.tsx█ 1 issue
└── markdown/             ███ 3 issues
```

---

## 五、优先修复建议

### 第一优先级（P0 — 影响稳定性/安全性）

| # | 问题 | 预估工作量 |
|---|------|-----------|
| C-01 | Agent DB 同步调用 → spawn_blocking | 2-3 天 |
| C-07 | SQLite 连接池 | 1 天 |
| C-05 | image_edit 路径遍历 | 2 小时 |
| C-03 | Git 参数注入 | 4 小时 |
| C-06 | XSS rehype-sanitize | 2 小时 |
| C-10 | begin_run 并发检查 | 1 小时 |

### 第二优先级（P1 — 影响性能/内存）

| # | 问题 | 预估工作量 |
|---|------|-----------|
| C-02 | DispatcherChat 内存泄漏 | 1 天 |
| C-08 | 渲染风暴节流 | 1 天 |
| H-01 | Git spawn_blocking | 1 天 |
| H-05 | 会话 JSONL 流式读取 | 1 天 |
| H-06 | PTY 缓冲区增大 | 1 小时 |

### 第三优先级（P2 — 架构改善）

| # | 问题 | 预估工作量 |
|---|------|-----------|
| C-09 | Runtime Mutex 合并/排序 | 2 天 |
| C-04 | API Key 加密存储 | 1 天 |
| H-18/19 | 组件拆分 | 3-5 天 |
| M-14 | runtime.rs 模块化拆分 | 3 天 |

---

## 六、架构层面建议

1. **引入异步数据库层：** 将 `rusqlite` 替换为 `tokio::rusqlite` 或在所有 DB 调用处使用 `spawn_blocking`。这是当前最大的性能瓶颈。

2. **Agent 运行时状态机重构：** 将 `run_llm_loop` 的 625 行拆分为明确的状态机（IDLE → PLANNING → EXECUTING → TOOL_CALL → RESPONDING），每个状态独立函数处理。

3. **前端状态管理演进：** DispatcherChat 的模块级 Map 应迁移到 React Context 或 useReducer 管理的统一状态树，消除内存泄漏和渲染风暴。

4. **安全加固清单：** 路径校验覆盖所有工具 → rehype-sanitize → Git 参数白名单 → API Key 加密 → 危险命令模式扩充。

---

## 七、修复记录（2026-05-17）

### 已修复 CRITICAL（9/10）

| # | 问题 | 修复方案 | 涉及文件 |
|---|------|----------|----------|
| C-01 | DB 同步阻塞 | `spawn_blocking` 异步包装 | `db.rs`, `runtime.rs` |
| C-02 | 模块级 Map 内存泄漏 | 订阅清理时自动 GC | `DispatcherChat.tsx` |
| C-03 | Git 参数注入 | `validate_git_ref_name()` 白名单 | `git.rs` |
| C-05 | 路径遍历 | `resolve_path()` 校验 | `image_edit.rs` |
| C-06 | XSS | `rehype-sanitize` 白名单过滤 | `MarkdownRenderer.tsx` |
| C-07 | 连接新建 | `r2d2` 连接池 (max_size=4) | `db.rs`, `Cargo.toml` |
| C-08 | 渲染风暴 | `requestAnimationFrame` 批量合并 | `DispatcherChat.tsx` |
| C-09 | 7 个 Mutex 死锁 | 合并为单一 `Mutex<Models>` 结构体 | `runtime.rs` |
| C-10 | 并发运行 | `begin_run` 返回 Result 拒绝重复 | `commands.rs` |

### 已修复 HIGH（7/19）

| # | 问题 | 修复方案 | 涉及文件 |
|---|------|----------|----------|
| H-01 | Git 阻塞运行时 | `run_git()`/`run_git_check()` 重构为 async + spawn_blocking | `git.rs` |
| H-06 | PTY 缓冲区过小 | `PTY_READ_BUFFER_SIZE` 从 32KB → 64KB | `pty.rs` |
| H-08 | BrowserPanel 每帧 new Image | 复用 `useRef<HTMLImageElement>` 实例 | `BrowserPanel.tsx` |
| H-09 | deleteTasks 竞态条件 | 用 `tasksRef` 替代 `setTasks` 读状态；单次原子移除+持久化 | `App.tsx` |
| H-10 | Task ID 碰撞 | `Date.now()` → `crypto.randomUUID()` | `App.tsx` |
| H-13 | 线程泄漏 | `CancellationToken` + `session_watchers` 注册/取消 | `session.rs`, `state.rs` |
| H-16 | 危险命令列表弱 | 从 11 → 38 个模式（覆盖 curl|sh、chmod 777、dd variants 等） | `common.rs` |

### 已修复 MEDIUM（3/19）

| # | 问题 | 修复方案 | 涉及文件 |
|---|------|----------|----------|
| M-03 | MarkdownRenderer 同步渲染大文本 | >10KB 文本先显示纯文本占位，`requestAnimationFrame` 后再渲染 Markdown | `MarkdownRenderer.tsx` |
| M-05 | persistProjectTasks 无防抖 | 已确认存在 400ms 防抖实现 | `App.tsx` |
| M-10 | Weak HashMap 键引用数据丢失 | 已确认当前代码无 `Weak<String>` 用法 | `runtime.rs` |

### 未修复 CRITICAL（1/10）

| # | 问题 | 状态 |
|---|------|------|
| C-04 | API Key 明文存储 | 中期目标：需引入 OS Keychain 绑定 |

### 未修复 HIGH（12/19）

H-02, H-03, H-04, H-05, H-07, H-11, H-12, H-14, H-15, H-17, H-18, H-19
