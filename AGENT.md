# Nezha 代码索引与业务流程

## 代码索引

- `src/App.tsx`：应用根状态；项目打开/切换、隐藏子任务创建、任务状态与会话事件持久化。
- `src/components/ProjectPage.tsx`：项目主界面；渲染项目栏、会话栏、调度器聊天、子任务终端、文件/Git 右侧面板。
- `src/components/SessionPanel.tsx`：调度器会话列表；加载、创建、删除会话，并防止首次进入重复创建 `New Session`。
- `src/components/DispatcherChat.tsx`：调度器主会话；发送消息、按 `sessionId` 读取消息历史、调度 Claude 子任务、免确认开关。
- `src/components/SubProcessTabs.tsx`：调度器拉起的 Claude 子任务终端；只展示调度产生的子进程，不提供手动新建入口。
- `src-tauri/src/lib.rs`：Tauri 命令注册；调度器命令、任务 PTY 命令、设置命令入口。
- `src-tauri/src/agent.rs`：Dispatcher Agent 主循环；LLM 调用、工具调用拦截、Claude 子任务调度、结果回注。
- `src-tauri/src/dispatcher_db.rs`：调度器 SQLite 存储；会话、消息、设置与免确认开关。
- `src-tauri/src/dispatcher_tools.rs`：调度器可用工具定义；`dispatch_claude`、继续/退出 Claude 会话等工具指令。
- `src-tauri/src/pty.rs`：Claude 子任务 PTY 生命周期；启动、输出转发、idle 检测、退出状态上报。

## 业务流程

1. 用户在欢迎页选择一个项目目录，`App.tsx` 初始化项目配置并进入 `ProjectPage`。
2. `SessionPanel` 加载该项目的调度器会话；没有会话时自动创建一个 `New Session`，并通过并发锁避免重复创建。
3. 主内容区默认进入 `DispatcherChat`，用户只和 Dispatcher Agent 对话，不再直接选择 Claude 或 Codex。
4. `DispatcherChat` 使用 `sessionId` 保存/读取聊天消息，同时把真实 `projectPath` 传给后端作为工具执行目录。
5. Dispatcher Agent 判断需要编码执行时调用 `dispatch_claude`，前端根据“免确认”开关决定是否弹出子任务审查。
6. 用户批准或免确认开启后，`ProjectPage` 通过 `onSubmitTask` 创建隐藏 Claude 子任务；子智能体只能由该调度流程拉起。
7. `pty.rs` 启动 Claude PTY，`SubProcessTabs` 展示对应终端；终端 idle 或任务结束后把输出清洗后回注给 Dispatcher Agent。
8. Dispatcher Agent 基于子任务结果继续对话并向用户反馈，任务记录与会话消息分别持久化到本地存储。

## 当前约束

- 新会话入口只面向 Dispatcher Agent；不要恢复旧的 Claude/Codex 手动选择新任务入口。
- 子任务默认使用 Claude；Codex 不作为可手动选择的子智能体入口。
- 权限控制通过 Dispatcher 的“免确认”开关管理，用于决定分发子任务时是否跳过审查确认。
- 会话消息 key 使用 `sessionId`；工具执行路径使用 `projectPath`，不要再混用同一个字段。
- 删除旧直连任务组件文件前需确认是否还有历史数据查看、迁移或回滚需求。
