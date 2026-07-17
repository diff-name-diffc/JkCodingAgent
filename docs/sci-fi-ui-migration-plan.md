# 科幻风 UI 全量改造执行计划

> 状态更新时间：2026-07-10  
> 目标：把当前只覆盖 Chat V2 的科幻视觉系统扩展到 Home、Project 工作区、文件/Git/终端/设置/分析等全部主要界面，并在迁移过程中保留既有业务逻辑。

## 1. 结论摘要

科幻 UI 全量改造已完成（Phase 0–8）。从最初只覆盖 Chat V2，到 Home、Project 工作区、文件/Git/终端/设置/分析/Browser/MCP/SubAgent 等全部主要界面均已进入新视觉体系：`src/styles/*` 对象样式模块已全部删除，全仓无 `const styles` / `const s` 样式对象块，`@radix-ui/themes` 已移除，剩余行内 `style={{}}` 均为动态/结构值（动态 width/height、计算 grid-template、依赖状态的 opacity/visibility 等）。亮/暗主题 accent 均已统一到科幻青（`#21f4df` / `#0d9488`）家族，浅色主题不再使用品牌绿。

本计划建议用 **8 个 coding 窗口**推进，每个窗口只处理一组高内聚界面，避免大模型在同一轮同时改布局、业务逻辑、旧样式删除而丢失交互。

## 2. 当前代码探索结果

### 2.1 已完成 / 部分完成

- [x] 普通聊天入口默认使用 `ChatPageV2`
- [x] 项目页 chat pane 使用 `ChatPageV2` embedded 模式
- [x] Chat 消息流、输入框、空状态、会话侧栏已接入 `.ai-*` 科幻风 class
- [x] 会话分类展示已恢复到 V2 侧栏，包括分类折叠、计数、未分类、搜索时分类标签
- [x] ArtifactPanel 已能展示工具产物和 SubAgent 执行轨迹
- [x] 命令面板 `Command+K` 已接入新 UI
- [x] Python Run 链路已接入 V2，结果展示 `PythonRunDrawer` 已迁移到 `ai-python-run-*` 科幻视觉

### 2.2 迁移区域完成情况（初始待迁移清单 → 当前状态）

> 以下表格记录迁移开始前列出的待迁移区域，现已在 Phase 0–8 全部完成（详见第 5 节）。此表为历史对照，保留以备追溯。

| 区域 | 代表文件 | 当前状态 | scope class |
|---|---|---|---|
| Home 外壳 / 项目列表 / 分析入口 | `WelcomePage.tsx`, `AnalyticsDashboard.tsx` | ✅ Phase 1 完成 | `ai-home-shell ai-migrated-home` |
| Project 工作区框架 | `ProjectPage.tsx`, `ProjectRail.tsx`, `SessionPanel.tsx`, `RightToolbar.tsx` | ✅ Phase 3 完成 | `ai-project-shell ai-migrated-project` |
| 文件浏览与编辑 | `FileExplorer.tsx`, `FileViewer.tsx`, `file-viewer/*`, `LargeFileViewer.tsx` | ✅ Phase 4 完成 | `ai-file-explorer ai-migrated-file-explorer` / `ai-file-viewer ai-migrated-file-viewer` / `ai-large-file-*` |
| Git 面板 | `GitChanges.tsx`, `GitHistory.tsx`, `GitDiffViewer.tsx` | ✅ Phase 5 完成 | `ai-migrated-git-changes` / `ai-migrated-git-history` / `ai-migrated-git-diff` |
| 设置弹窗 | `AppSettingsDialog.tsx`, `app-settings/**/*` | ✅ Phase 6 完成 | `ai-settings-shell ai-migrated-settings` |
| Aha / RAG / SubAgent 配置 | `AhaAgentPanel.tsx`, `RagKbConfigPanel.tsx`, `SshToolPanel.tsx`, `SubAgent*` | ✅ Phase 6 完成 | `ai-migrated-aha-panel` / `ai-migrated-rag-panel` / `ai-migrated-ssh-panel` / `ai-migrated-subagent-panel` |
| Browser / MCP / SubProcess / Terminal | `BrowserPanel.tsx`, `McpStatusDialog.tsx`, `SubProcessTabs.tsx`, `ShellTerminalPanel.tsx`, `TerminalView.tsx` | ✅ Phase 7 完成 | `ai-migrated-browser-panel` / `ai-migrated-mcp-dialog` / `ai-subprocess-*` / `ai-migrated-shell-terminal` / `ai-migrated-terminal-view` |
| 旧分类组件 | `ChatCategorySection.tsx`, `ChatSessionCard.tsx` | ✅ Phase 8 已删除（零引用死代码） | — |
| 保留的分类 CRUD 组件 | `ChatNewCategoryDialog.tsx`, `ChatCategoryContextMenu.tsx` | ✅ Phase 2 迁移到 `ai-dialog` / `ai-context-menu` | `ai-dialog` / `ai-context-menu` |
| 全局 Radix Themes 壳 | `App.tsx`, `main.tsx`, `package.json` | ✅ Phase 8 移除 `@radix-ui/themes` | — |

### 2.3 规模信号（当前）

- 全仓已无 `const styles` / `const s = {` / `const s: Record` 样式对象块（0 处）。
- 全仓已无 `[style*=]` CSS 属性选择器 hack（0 处）。
- `src/styles/` 仅保留 `tailwind.css`，其余样式模块（`panels.ts` / `terminal.ts` / `appSettings.ts` / `index.ts` / `common.ts` / `dialogs.ts` / `layout.ts` / `task.ts` / `subAgent.ts`）均已删除。
- 仍超过 400 行的组件（后续可选拆分）：
  - `ProjectPage.tsx`：约 1264 行（工作区布局已抽到 `src/components/project/ProjectWorkspaceLayout.tsx`）
  - `LargeFileViewer.tsx`：约 1126 行
  - `RagKbConfigPanel.tsx`：约 891 行
  - `SshToolPanel.tsx`：约 842 行
  - `AppSettingsDialog.tsx`：约 722 行
  - `GitHistory.tsx`：约 483 行
  - `GitChanges.tsx`：约 437 行
- 残留行内 `style={{}}` 均为动态/结构值（虚拟滚动计算、splitter 尺寸、运行时颜色、grid 列数等），非纯视觉样式。较多的是 `AppSettingsDialog.tsx`（9 处）、`ProjectWorkspaceLayout.tsx`（8 处）、`SubAgentExecutionView.tsx`（6 处）、`AnalyticsDashboard.tsx`（6 处）。

## 3. 改造原则

1. **先建立统一设计系统，再迁移页面。** 不再继续向 `tailwind.css` 堆一次性选择器；应抽出可复用的 shell、panel、toolbar、status、list、empty、splitter、dialog class。
2. **业务逻辑不和视觉迁移混改。** 每个窗口只允许必要的结构拆分和 className 注入，不改 Tauri 命令、数据模型和状态语义。
3. **大组件先切分再换皮。** 超 400 行组件不能继续堆视觉代码，先拆 presentational 子组件。
4. **旧对象样式逐步出清。** `src/styles/*` 在迁移期可保留，但完成区域不得继续新增 `s.*` 样式。
5. **完成区域必须标注。** 每个迁移完成的页面/组件要在本文件的进度表打 `[x]`，并在组件根节点加稳定 scope class，如 `ai-project-shell`、`ai-settings-shell`，方便视觉回归定位。
6. **失败要大声。** UI 迁移中不要吞掉加载错误、命令错误；保留现有 ErrorBoundary 和明确错误态。

## 5. 执行阶段与完成标记

### Phase 0：设计系统收口

- [x] 建立 `src/components/ui/sci-fi-shell.tsx` 或等价基础组件：`AiPanel`、`AiToolbar`、`AiSectionHeader`、`AiStatusPill`、`AiEmptyState`、`AiSplitter`
- [x] 在 `src/styles/tailwind.css` 中整理统一 class：`ai-shell`、`ai-panel`、`ai-toolbar`、`ai-list-row`、`ai-dialog`、`ai-field`
- [x] 确认亮/暗主题策略：科幻 UI 默认暗色，但不能破坏现有 `themeMode`
- [x] 建立完成标注规范，见第 7 节

完成标注：`ai-migrated-home` 已作为首个落地区域接入；验证：`pnpm exec tsc --noEmit`、`pnpm lint`、`pnpm build` 通过；浏览器烟测覆盖 1280x800、1600x1000、390x844。主题策略：科幻 scope 默认暗色视觉 token，保留 `themeMode` 传递和现有主题切换入口，不改全局主题状态。

建议窗口：S  
验收：`pnpm exec tsc --noEmit`、`pnpm lint`

### Phase 1：Home 全壳统一

- [x] `WelcomePage.tsx`：项目/聊天/分析三种 view 使用同一科幻工作区外壳
- [x] `SidebarFooterActions.tsx`：底部设置/主题/通知统一图标按钮视觉
- [x] `ProjectAvatar.tsx`：项目头像改为终端芯片/轨道风格
- [x] `AnalyticsDashboard.tsx`：分析页从旧卡片改为仪表盘风格
- [x] 移除 Home 区域新增的行内视觉样式，保留必要布局动态值

完成标注：`WelcomePage` 根节点 `ai-home-shell ai-migrated-home`，验证：`pnpm exec tsc --noEmit`、`pnpm lint`、`pnpm build` 通过；浏览器烟测覆盖 1280x800、1600x1000、390x844。补充：`SidebarFooterActions` 接入 `ai-sidebar-footer-actions` scope，不改通知、设置、主题、用量原有回调。

建议窗口：M  
完成后标注：`WelcomePage` 根节点 `ai-home-shell ai-migrated-home`

### Phase 2：Chat 分类管理补完

- [x] 分类展示恢复到 V2 侧栏
- [x] 迁移 `ChatNewCategoryDialog.tsx` 到新 `ai-dialog` 风格
- [x] 迁移 `ChatCategoryContextMenu.tsx` 到新 dropdown 风格
- [x] 在 V2 `Sidebar` 接回分类新建、重命名、删除、移动会话能力
- [x] 如果旧 `ChatCategorySection.tsx` / `ChatSessionCard.tsx` 不再被引用，删除或归档为历史参考

完成标注：V2 Sidebar 分类管理复用 `ai-dialog`、`ai-context-menu`、`ai-category-*`、`ai-session-menu-trigger`；验证：`pnpm exec tsc --noEmit`、`pnpm lint`、`pnpm build` 通过。逻辑接线：`chat_create_category`、`chat_update_category`、`chat_delete_category`、`chat_set_session_category` 通过 TanStack Query mutations 接回并刷新 categories/sessions。

建议窗口：M  
逻辑回归重点：分类计数、未分类、搜索结果、会话移动后 active session 不丢

### Phase 3：Project 工作区框架

- [x] `ProjectPage.tsx`：抽出工作区布局组件，降低主文件行数
- [x] `ProjectRail.tsx`：改为科幻 project dock，补充 hover/active/attention 状态
- [x] `SessionPanel.tsx`：项目会话列表迁移到与 Chat sidebar 一致的分组/状态语言
- [x] `RightToolbar.tsx`：右侧工具栏改为垂直 dock，统一 tooltip/icon button
- [x] `SubProcessTabs.tsx`：子进程 tabs 统一为运行轨迹 dock
- [x] 工作区 split pane 改为 `AiSplitter`，禁止散落行内 splitter 样式

完成标注：`ProjectPage` 根节点 `ai-project-shell ai-migrated-project`；Project dock 使用 `ai-project-rail` / `ai-project-drawer`，会话列表使用 `ai-project-session-*`，右侧工具栏使用 `ai-project-right-toolbar`，子进程轨迹 dock 使用 `ai-subprocess-*`，splitter 使用 `ai-splitter ai-project-*`。`ProjectWorkspaceLayout`、`ProjectMainArea`、`ProjectWorkbench`、`ProjectRightPanelHost` 已抽到 `src/components/project/ProjectWorkspaceLayout.tsx`，保留原有会话、编辑器、右侧面板、Browser dock、MCP 设置接线。验证：`pnpm exec tsc --noEmit`、`pnpm lint`、`pnpm build` 通过。

结构清理补丁（2026-07-09）：上一轮 Phase 3 仅用 `!important` 在 `tailwind.css` 上叠了一层科幻视觉，组件里的布局结构仍散落在 `const styles` 对象块与行内 `style={{}}`。本轮完成结构迁移：（1）`SubProcessTabs.tsx` 删除 160 行 `const styles` 块，布局下推到 `ai-subprocess-*`（dock/resize/tabbar/tabbar-label/tablist/tab/tab-icon/tab-label/pulse/pending-dot/agent-badge/status/tab-close/content/placeholder/terminal-stage/terminal-layer/terminal-wrap/status-overlay 全部补齐 display/flex/padding/gap/sizing 结构），组件仅保留 4 处动态内联（dock 显隐与高度、terminal layer 显隐、status badge 动态色）。（2）`ProjectRail.tsx` 删除 15 处行内 `style={{}}` 与 `hov`/`addHov`/`expandHov` 三个 hover 状态机，布局下推到 `ai-project-rail`（含 spacer）/`ai-project-rail-item`/`ai-project-rail-control`（含 `is-attention`/`is-active` 及 `.ai-project-rail-control-icon` 旋转）/`ai-project-rail-add`/`ai-project-status-dot`/`ai-project-drawer`/`-title`/`-list`/`-row`/`-avatar`/`-name`/`-status-dot`，hover/active 全部改由 CSS `:hover` 承接，组件仅保留 3 处动态内联（rail zIndex 依赖 drawerOpen、StatusBadge 与 drawer-status-dot 的动态背景色）。（3）`PythonRunDrawer.tsx` 完成从零迁移，删除 147 行 `const styles` 块，新增完整 `ai-python-run-*` 作用域（drawer/header/title-wrap/kicker/title/notice/actions/action/body/section/section-title/code/output/error/chip-row/chip/timeline/timeline-item/muted/empty），关闭按钮改用共享 `IconButton` 组件，仅保留 4 处动态内联（drawer 动态 width、action 按钮 opacity 状态）。验证：`pnpm exec tsc --noEmit`、`pnpm lint`、`pnpm build` 全部通过。

建议窗口：L  
逻辑回归重点：项目切换、会话选择、子进程注入、编辑区/会话区 split、Browser 面板自动打开

### Phase 4：文件浏览与编辑工作台

- [x] `FileExplorer.tsx` / `file-explorer/*`：树、右键菜单、重命名弹窗迁移
- [x] `FileViewer.tsx` / `FileTabPane.tsx`：tab strip、空状态、保存状态迁移
- [x] `LargeFileViewer.tsx`：先拆分工具栏、状态栏、编辑区域，再换 UI
- [x] `ImagePreviewPane.tsx`：图像预览改为深色 inspection frame
- [x] 保留 Monaco/Shiki 逻辑，不在本阶段改高亮实现

建议窗口：L  
逻辑回归重点：打开/关闭 tab、保存、预览切换、大文件限制、图片预览、Markdown link 打开

阶段进展：`FileExplorer` 已接入 `ai-file-explorer ai-migrated-file-explorer` scope，虚拟树行、刷新按钮、项目标签、空状态、重命名弹窗和文件右键菜单完成科幻视觉迁移；保留 `useFileExplorerTree`、`delete_fs_entry`、`move_fs_entry`、`onFileSelect` / `onFileRename` / `onFileDelete` 原接口接线。验证：`pnpm exec tsc --noEmit`、`pnpm lint`、`pnpm build` 通过。

阶段进展：`FileViewer` 已接入 `ai-file-viewer ai-migrated-file-viewer` scope，tab strip、标签操作菜单、隐藏按钮、文本文件 pane shell、加载/错误态、语言/保存状态 pill 完成迁移；`ImagePreviewPane` 已改为深色 inspection frame。保留 `flushFile` / `flushPendingSave`、自动保存防抖、`read_file_content` / `write_file_content` / `read_image_preview`、Markdown 预览切换和 Monaco 编辑器接线。验证：`pnpm exec tsc --noEmit`、`pnpm lint`、`pnpm build` 通过。

阶段完成：`LargeFileViewer` 已接入 `ai-large-file-*` 虚拟编辑器视觉，状态栏、保存提示、虚拟滚动容器、行号 gutter、选择高亮、编辑行高亮完成迁移；保留 `rope_open` / `rope_read_lines` / `rope_replace_line` / `rope_edit` / `rope_save` / `rope_close` 原有调用、缓存策略、选择逻辑、撤销重做和 `⌘S` 保存语义。验证：`pnpm exec tsc --noEmit`、`pnpm lint`、`pnpm build` 通过。

### Phase 5：Git 工作流

- [x] `GitChanges.tsx`：改为变更清单 + stage 区域的高密度操作台
- [x] `GitHistory.tsx`：提交时间线、commit details、文件列表迁移
- [x] `GitDiffViewer.tsx`：diff shell、状态标签、文件头迁移
- [x] `BranchBar.tsx`：分支切换/创建改为命令条风格
- [x] 确保大文件/大量变更时列表不会出现布局抖动

建议窗口：M  
逻辑回归重点：stage/unstage、commit、push/pull、分支切换、diff 文件打开

阶段进展：`GitChanges` 已接入 `ai-git-changes ai-migrated-git-changes` scope，变更列表、暂存/取消暂存动作、错误态、提交信息输入、AI 提交信息按钮和提交按钮完成高密度操作台迁移；保留 `git_status`、`git_stage`、`git_unstage`、`git_stage_all`、`git_unstage_all`、`generate_commit_message`、`git_commit` 原接口接线。验证：`pnpm exec tsc --noEmit`、`pnpm lint`、`pnpm build` 通过。

阶段进展：`GitDiffViewer` 已接入 `ai-git-diff-shell ai-migrated-git-diff` scope，diff shell、标题栏、关闭按钮、错误态、文件头和 hunk 视觉完成迁移；保留 `parseDiff`、`git_show_diff`、`git_show_file_diff`、`git_file_diff` 原接口接线。验证：`pnpm exec tsc --noEmit`、`pnpm lint`、`pnpm build` 通过。

阶段进展：`GitHistory` 已接入 `ai-git-history ai-migrated-git-history` scope，提交时间线、分支下拉、远端同步按钮、commit detail、文件变更列表完成迁移；保留 `git_list_branches`、`git_log`、`git_remote_counts`、`git_commit_detail`、`git_pull`、`git_push` 原接口接线。验证：`pnpm exec tsc --noEmit`、`pnpm lint`、`pnpm build` 通过。

阶段完成：`BranchBar` 已接入 `ai-branch-bar ai-migrated-branch-bar` scope，分支切换入口、新建分支弹窗、分支搜索 Popover、当前/切换中/错误状态完成命令条风格迁移；Git 变更与历史列表已使用固定行高、ellipsis、稳定按钮尺寸和滚动容器约束，避免大量变更或长路径导致布局抖动。保留 `git_list_branches`、`git_checkout_branch`、`git_create_branch` 原接口接线与 IME 输入保护。验证：`pnpm exec tsc --noEmit`、`pnpm lint`、`pnpm build` 通过。

### Phase 6：Settings / Aha / RAG / SubAgent

- [x] `AppSettingsDialog.tsx`：先拆 tab shell、footer、loading/error，再迁移视觉
- [x] `AhaAgentPanel.tsx`：按模型、工具、分类配置拆子组件；迁移后保留分类配置逻辑
- [x] `SshToolPanel.tsx` / `SshAuditRecordList.tsx`：SSH 工具配置迁移
- [x] `RagKbConfigPanel.tsx` / `RagSidecarLogPanel.tsx` / `PasswordInput.tsx`：RAG 知识库配置迁移
- [x] `SubAgentManagePanel.tsx` / `SubAgentEditorDialog.tsx`：SubAgent 管理迁移
- [x] 所有表单字段统一使用 `components/ui` 或 `ai-field`，不继续散落原生 input 样式

建议窗口：L，必要时拆成两个 M  
逻辑回归重点：保存配置、分类 agent config、SSH 测试、RAG ingest、SubAgent 增删改

阶段进展：`AppSettingsDialog` 已接入 `ai-settings-shell ai-migrated-settings` scope，设置弹窗主 shell、左侧 tab、右侧 header、关闭按钮、通用设置 footer、Agent 配置 footer、加载/缺失/错误/已保存状态完成迁移；保留 `load_app_settings`、`detect_agent_paths`、`save_app_settings`、`read_agent_config_file`、`write_agent_config_file`、`detect_agent_versions_for_settings` 原接口接线，Aha/RAG/SubAgent 内部业务面板暂不改动。验证：`pnpm exec tsc --noEmit`、`pnpm lint`、`pnpm build` 通过。

阶段完成：`AhaAgentPanel`（1437 → 392 行）已按模型/工具/分类配置拆分为 `provider-editor.tsx`（`useAhaProviderRegistry` hook + `ModelProviderSection` / `ProviderEditor`，集中管理 showKey/expanded/testing/feedback/modelLists/fetchError）、`tools-tab.tsx`（`ToolsTab` / `ToolOptionRow`）、`chat-category-tools.tsx`（`ChatCategoryToolsTab`，分类芯片 + 系统提示词 + 工具 + 子智能体）、`sub-agent-picker.tsx`（`SubAgentPicker` / `SubAgentOptionRow`），`BehaviorSection` 保留在主文件。全部表单字段统一为 `ai-settings-input` / `ai-settings-textarea` / `ai-aha-field` / `ai-aha-field-label`，移除 `s.aha*` 内联样式与 `style={{}}` 纯视觉对象；`ai-aha-panel` 作用域内的 `[style*=...]` 属性选择器 hack 已替换为正式 `ai-aha-*` class（provider/tool/dropdown/grid/toggle/category-chip/collapsible 等）。保留 `aha_get_settings_v2`、`aha_save_settings_v2`、`aha_get_chat_category_agent_configs`、`aha_save_chat_category_agent_configs`、`sub_agent_set_global_enabled`、`sub_agent_get_global_enabled`、`dispatcher_test_model`、`dispatcher_fetch_models`、`aha_list_agent_tools`、`sub_agent_list` 原接口接线与分类配置/全局子智能体/自动批准/上下文调试逻辑。验证：`pnpm exec tsc --noEmit`、`pnpm lint`、`pnpm build` 通过。

阶段完成：`SshToolPanel` 子组件（`ServerEditor` / `AuthMethodEditor` / `ReviewAiSection` / `Field`）深层迁移到 `ai-aha-provider*` / `ai-aha-field*` / `ai-settings-input` / `ai-settings-textarea ai-ssh-prompt-textarea` / `ai-aha-category-chip` / `ai-ssh-form-grid` / `ai-ssh-keyfile-row` / `ai-aha-feedback` 等 class，移除 `s.aha*` 与 `serverHeaderLeftStyle` / `contextButtonStyle` / `authButtonStyle` / `keyFileRowStyle` 等内联样式对象；`SshAuditRecordList` 已改为 `ai-ssh-audit-*` 审计列表、状态 badge、输出块。保留 `aha_resolve_ssh_workspace`、`ssh_tool_load_config`、`ssh_tool_load_audit`、`ssh_tool_save_config`、`ssh_tool_test_server_config`、`aha_save_settings_v2`、`dispatcher_test_model` 原接口接线。验证：`pnpm exec tsc --noEmit`、`pnpm lint`、`pnpm build` 通过。

阶段完成：`RagKbConfigPanel` 已接入 `ai-rag-panel ai-migrated-rag-panel` scope，运行状态条、服务日志、导入文档、Qdrant、Embedding、分片/稀疏向量、OCR、保存 footer 完成迁移；`RagSidecarLogPanel` 已改为 `ai-rag-log-*` 控制台样式，`PasswordInput` 已改为 `ai-rag-password-*`。保留 `rag_get_kb_config`、`rag_status`、`rag_save_kb_config`、`rag_restart`、`rag_test_qdrant`、`rag_test_embedding`、`rag_ingest_files`、`rag_ingest_job_status`、`rag_logs_snapshot`、`rag_logs_clear` 与 `rag-log` 事件接线。验证：`pnpm exec tsc --noEmit`、`pnpm lint`、`pnpm build` 通过。

阶段完成：`SubAgentManagePanel` 已接入 `ai-subagent-panel ai-migrated-subagent-panel` scope，管理列表、空态、恢复内置浏览器助手、卡片、删除确认和错误反馈完成迁移；`SubAgentEditorDialog` 已改为 `ai-subagent-dialog-*`，基本信息、工具选择、运行时参数、校验错误和 footer 完成迁移。保留 `sub_agent_list`、`sub_agent_create`、`sub_agent_update`、`sub_agent_delete`、`sub_agent_seed_browser`、`sub_agent_list_tools` 原接口接线。验证：`pnpm exec tsc --noEmit`、`pnpm lint`、`pnpm build` 通过。

### Phase 7：Browser / MCP / Terminal / 通知

- [x] `BrowserPanel.tsx` / `BrowserDock.tsx`：浏览器控制台、dock 卡片迁移
- [x] `McpStatusDialog.tsx`：服务器状态、启停开关、错误态迁移
- [x] `ShellTerminalPanel.tsx` / `TerminalView.tsx`：终端壳和状态栏迁移
- [x] `NotificationBell.tsx` / `Toast.tsx` / `UsagePopover.tsx`：全局浮层迁移
- [x] `ToolActivityBubble.tsx`：工具活动气泡改为新状态芯片

建议窗口：M  
逻辑回归重点：Browser restore/minimize/close、MCP enable toggle、终端输入输出、通知弹层层级

阶段完成：`BrowserPanel` 已接入 `ai-browser-panel ai-migrated-browser-panel` scope，顶部控制台、地址栏、画布 stage、最小化提示、空态、日志区完成迁移；`BrowserDock` 已改为 `ai-browser-dock-*`。保留 `browser_get_status`、`browser_start`、`browser_start_plain_chat`、`browser_stop`、`browser_go_back`、`browser_list_chrome_profile_candidates`、`browser_import_chrome_profile`、`browser_navigate`、`browser_reload`、`browser_click_at` 与 `browser-status`、`browser-frame`、`browser-log` 事件接线。验证：`pnpm exec tsc --noEmit`、`pnpm lint`、`pnpm build` 通过。

阶段完成：`McpStatusDialog` 已接入 `ai-mcp-dialog ai-migrated-mcp-dialog` scope，状态元信息、server 卡片、启停 switch、错误态、空态、工具展开详情完成迁移；保留 `onRefresh`、`onToggleServerEnabled`、`onClose` 回调接口和 `role="switch"` / `aria-checked` 语义。验证：`pnpm exec tsc --noEmit`、`pnpm lint`、`pnpm build` 通过。

阶段完成：`ShellTerminalPanel` 已接入 `ai-shell-terminal-panel ai-migrated-shell-terminal` scope，resize handle、header、关闭按钮和 terminal canvas 完成迁移；`TerminalView` 已接入 `ai-terminal-view ai-migrated-terminal-view`。保留 `open_shell`、`send_input`、`resize_pty`、`kill_shell` 与 `shell-output` 事件接线，以及任务终端的输入、resize、snapshot、ready 注册逻辑。验证：`pnpm exec tsc --noEmit`、`pnpm lint`、`pnpm build` 通过。

阶段完成：`NotificationBell` 已接入 `ai-notification-*` scope，触发按钮、未读计数、弹窗、列表项、已读操作、空态/错误态完成迁移；保留 `get_notifications`、`mark_notification_read`、`mark_all_notifications_read` 与外部链接打开逻辑。`Toast` 已改为 `ai-toast-*` 栈；`UsagePopover` 已改为 `ai-usage-*`，保留 `useUsageSnapshot(open)` 懒加载和 Claude/Codex 用量窗口展示。验证：`pnpm exec tsc --noEmit`、`pnpm lint`、`pnpm build` 通过。

阶段完成：`ToolActivityBubble` 已接入 `ai-tool-activity ai-migrated-tool-activity` scope，工具状态点、结果模式、详细引用、子智能体状态、loading/error/pending 状态完成迁移；移除组件内大块 `CSSProperties` 视觉对象。保留 `dispatcher_get_tool_artifact` 懒加载、artifact cache/error/loading 状态、`useSubAgentSessions(workspaceId)`、`SubAgentExecutionCard` 和 `MarkdownRenderer` 接线。验证：`pnpm exec tsc --noEmit`、`pnpm lint`、`pnpm build` 通过。

### Phase 8：旧 UI 出清

- [x] 删除未引用旧分类 UI 文件，或明确迁到新实现后删除
- [x] 清理 `src/styles/*` 中已无引用的样式 key
- [x] 移除 `@radix-ui/themes`，前提是 `App.tsx` 不再依赖 `Theme` 且全局 styles 不再需要
- [x] 清理 `App.css` 中旧浅绿主题 token，只保留新设计系统必要 token
- [x] 全仓扫描：不允许新增 `style={{}}` 做纯视觉，不允许新增 `s.*` 旧样式

建议窗口：S/M  
验收：`rg` 无旧入口引用，`pnpm exec tsc --noEmit`、`pnpm lint`、`pnpm build`

阶段完成：旧分类 UI 文件（`ChatCategorySection.tsx` / `ChatSessionCard.tsx` / `ChatSessionSidebar.tsx` / `DispatcherChat.tsx`）已确认全仓无引用并暂存删除；`src/styles/appSettings.ts`（全部为 `aha*` / `rag*` key）在 Phase 6 深层迁移后零引用，已删除并从 `styles/index.ts` 移除聚合；`@radix-ui/themes` 已从 `App.tsx`（移除 `<Theme>` 包装，根节点改为 `<div style={{ ...s.root, position: "relative", height: "100%" }}>`）、`main.tsx`（移除 `import "@radix-ui/themes/styles.css"`）和 `package.json`（`pnpm remove`，回收 26 个依赖）彻底移除——主题切换本就由 `document.documentElement.classList.toggle("dark", isDark)` 驱动 `html.dark { ... }` token，与 Radix 无关；`App.css` 暗色主题的浅绿 accent 家族（`--accent` / `--accent-strong` / `--accent-soft` / `--accent-hover` / `--accent-subtle` / `--border-focus` / `--bg-selected` / `--chat-shell-bg` 绿色径向 / `--chat-focus-ring`）统一 retune 为科幻青（`#21f4df` 家族），消除 `tailwind.css` 中 41 处 `var(--accent)` 在暗色下渲染成绿色的不一致，语义色 `--success` / `--warning` / `--danger` 保留；全仓 `grep "s\.aha\|s\.rag"` 无引用、`pnpm exec tsc --noEmit`、`pnpm lint`、`pnpm build` 通过。亮色主题（`:root`）仍沿用品牌绿，留待后续亮色科幻化时统一。

补丁修复（2026-07-08）：`.ai-chat-shell` scope（`tailwind.css` 6916 行）此前只覆盖了 `--bg-card` / `--text-*` / `--border-*` 等 token，但遗漏了 `--chat-surface` / `--chat-surface-strong` / `--chat-glass-border` 和 `--markdown-*` 家族。这些变量在 `App.css :root`（浅色模式）中是白/米色（`rgba(255,254,248,0.86)`、`#fbfdff` 等），导致**全局浅色主题下**聊天面板内的 `ToolActivityBubble`（`.ai-tool-activity`）、工具调用结果代码块、Markdown 代码块仍渲染为白色卡片，与科幻暗色底形成刺眼对比。已补齐 `.ai-chat-shell` scope 内的全部缺失变量（`--chat-surface: rgba(10,22,38,0.84)`、`--chat-surface-strong: rgba(13,28,47,0.94)`、`--chat-glass-border: rgba(94,242,234,0.12)`、`--chat-sidebar-bg`、`--chat-main-rail`、完整 `--markdown-*` 暗色家族），确保聊天外壳无论全局 `themeMode` 是 light 还是 dark 都渲染为暗色科幻视觉。验证：`pnpm exec tsc --noEmit`、`pnpm lint`、`pnpm build` 通过；浏览器在浅色主题下注入 `ai-tool-activity` + `markdown-code-block` 探针，`getComputedStyle` 确认 `--chat-surface` 解析为 `rgba(10,22,38,0.84)`（不再是白色），截图确认工具活动卡片、代码块均为暗色。

旧代码出清补丁（2026-07-09）：对 `src/styles/*` 与未引用组件做了一次精确的死代码审计与清理。逐 key 比对「定义的顶层样式 key」与「8 个仍 import `s` 的文件（App.tsx / AppSettingsDialog.tsx / SubAgentExecutionView.tsx / SessionPanel.tsx / WelcomePage.tsx / ProjectPage.tsx / SidebarFooterActions.tsx / ErrorBoundary.tsx）中真实引用的 `s.<key>`」：253 个 key 中 179 个零引用。删除 100% 死代码文件 `src/styles/panels.ts`（39 key 全死）与 `src/styles/terminal.ts`（4 key 全死），并从 `styles/index.ts` 移除其聚合；`common.ts`（47→6，仅留 `errorBoundary*`）、`dialogs.ts`（32→1，仅留 `settingsBody`）、`task.ts`（32→17，移除 `newTaskRow`/`branchBar*`/`groupLabel`/`taskAction*`/`taskPlayBtn`/`taskStarBtn`/`taskRename*`/`taskActionsMeta` 等已被 `ai-*` class 取代的 key）、`layout.ts`（67→18，移除 `welcomeMain`/`sidebarBrandBadge`/`chatHomeBody`/`chatSessionPanel`/`category*`/`sessionCard*`/`searchRow`/`projectItem*`/`emptyState` 等已被 V2 聊天侧栏与 `ai-home-*` 取代的 key）；`subAgent.ts` 全部 32 key 仍被 `SubAgentExecutionView` 使用，保留不动。`src/styles/*.ts` 总计 1956 → 599 行，回收 1357 行死样式。另删除 7 个零引用的死模块：`components/SessionTokenUsageIndicators.tsx`（旧 Popover token 指标，已被 `ai-usage-*` 取代）、`components/chat/code-block.tsx`（被 `markdown/MarkdownCodeBlock` 取代的废弃重构产物）、`components/ui/skeleton.tsx`（未使用的 shadcn 原语）、`hooks/useComposedInput.ts`（仅出现在 `prompt-input.tsx` 注释中）、`hooks/useDashScopeAsr.ts`（340 行无引用的 ASR hook）、`lib/streaming.ts`（`streamingCaret`/`appendChunk`/`stripCaret` 零引用）、`utils/segments.ts`（`segmentsToMarkdown`/`markdownToSegments` 零引用），共回收 947 行。对 `tailwind.css` 的 666 个 `.ai-*` class 反向比对源码引用，零死类。验证：`pnpm exec tsc --noEmit`、`pnpm lint`、`pnpm build` 全部通过；`grep "s\.aha\|s\.rag"` 无引用。

内联样式出清补丁（2026-07-09）：完成上一轮「文件级」删除后，遗留的「内联 `s` 对象块 + 行内 `style={{}}` 纯视觉」在两个组件中仍未真正迁移，本轮将其彻底出清。（1）`SessionPanel.tsx`：删除 135 行 `const s: Record<string, React.CSSProperties>` 块（原 `styles/task.ts` + `styles/layout.ts` 残留），将布局结构下推到 `ai-project-session-*` class（panel/header/icon-btn/search/actions/new-session-btn/divider/list/row/row-main/row-title/row-sub/keywords/keyword-tag/actions-inline/running/delete/empty/footer 全部补齐 display/flex/padding/gap 等结构属性），组件仅保留 3 处动态/结构内联（搜索图标 `flexShrink:0`、`+N` 计数 `opacity`、IntersectionObserver 哨兵 `height:1`）；根节点补 `ai-migrated-project` scope。（2）`SubAgentExecutionView.tsx`：删除 280 行 `const s` 块与 `commandAuditPreviewStyle` 常量，新增 `ai-subagent-exec-*` 完整作用域（card/header/name/phase/elapsed/body/phase-bar/phase-step×4/phase-connector/stats/stat-chip/timeline×8/task/progress×2/result×3/error×3/timeline-audit/timeline-args），组件仅保留 6 处动态内联（phase-step 动态 `flex`、loader 对齐 margin、`statusColor` 动态色）；根节点补 `ai-migrated-tool-activity` scope。（3）`WelcomePage.tsx`：删除导航栏/品牌头/区段标签/页脚/搜索操作行/项目卡片主体的行内 `style={{}}`，新增 `ai-home-brand/-title/-subtitle`、`ai-home-nav-list/-section-label/-footer`、`ai-home-nav-icon/-meta`、`ai-home-search-actions`、`ai-project-card-main` 等 class 承接布局，`ai-home-nav` 补齐 width/flex/padding 结构。（4）`IconButton.tsx`：从全行内样式重构为 `ai-icon-button` + `is-active`/`is-disabled` class，移除 `useState` hover 状态机（改由 CSS `:hover` 承接），`RightToolbar.tsx` 同步移除重复行内结构。（5）`tailwind.css`：清除全部 5 处 `[style*="..."]` 属性选择器 hack（`ai-settings-panel-host` 下 `settingsBody`/`var(--danger)`/`已保存`/`var(--success` 与 `ai-project-right-toolbar button[style*="var(--accent-subtle)"]`，前者因对应行内样式已删除而失效，后者改为 `.ai-icon-button.is-active` 正式 class）。验证：`pnpm exec tsc --noEmit`、`pnpm lint`、`pnpm build` 全部通过；`grep "[style\*=" src/styles/tailwind.css` 为 0；`grep "const s: Record\|const s = {" src` 为 0。

残留结构迁移补丁（2026-07-09）：复扫后发现数处「`tailwind.css` 用 `!important` 叠了科幻视觉、但组件布局结构仍散落在 `const styles` 对象块或行内 `style={{}}`」的假完成区域，本轮补齐。（1）`SubProcessTabs.tsx`：删除 160 行 `const styles` 块，结构下推到 `ai-subprocess-*`（dock/resize/tabbar/tabbar-label/tablist/tab/tab-icon/tab-label/pulse/pending-dot/agent-badge/status/tab-close/content/placeholder/terminal-stage/terminal-layer/terminal-wrap/status-overlay），组件仅保留 4 处动态内联（dock 显隐与高度、terminal layer 显隐、status badge 动态色）。（2）`ProjectRail.tsx`：删除 15 处行内 `style={{}}` 与 `hov`/`addHov`/`expandHov` 三个 hover 状态机，结构下推到 `ai-project-rail`（含 spacer）/`-rail-item`/`-rail-control`（含 `is-attention`/`is-active` 及 `-control-icon` 旋转）/`-rail-add`/`-status-dot`/`-drawer`/`-drawer-title`/`-drawer-list`/`-drawer-row`/`-drawer-avatar`/`-drawer-name`/`-drawer-status-dot`，hover/active 全部改由 CSS `:hover` 承接，组件仅保留 3 处动态内联（rail zIndex 依赖 drawerOpen、两处动态背景色）。（3）`PythonRunDrawer.tsx`（此前「结果展示仍复用旧 `PythonRunDrawer`」）完成从零迁移，删除 147 行 `const styles` 块，新增完整 `ai-python-run-*` 作用域，关闭按钮改用共享 `IconButton` 组件，根节点补 `ai-migrated-python-runner` scope，仅保留 4 处动态内联（drawer 动态 width、action 按钮 opacity 状态）。（4）`HomeChatPage.tsx`：删除根容器/浏览器面板/resizer/fallback 的 4 处行内 `style={{}}`，新增 `ai-home-chat`/`ai-home-chat-browser`/`ai-home-chat-resizer`/`ai-home-chat-fallback` 承接结构，保留 `nezha-chat-home`/`nezha-brand-surface` 的视觉修饰 class。（5）`main.tsx`：删除重复的内联 `ErrorBoundary` 类（与 `components/ErrorBoundary.tsx` 功能重复），改为复用共享 `ErrorBoundary` 组件（`label="页面"`），`.ai-error-boundary` 补 `min-height:100vh` 使其作为根级兜底也能铺满视口。验证：`pnpm exec tsc --noEmit`、`pnpm lint`、`pnpm build` 全部通过；全仓无 `const styles`/`const s = {`/`const s: Record` 样式对象块。

AppSettingsDialog ThemePanel 补丁（2026-07-09）：`AppSettingsDialog.tsx` 的 `ThemePanel` 组件此前虽已在 Phase 6 标记完成，但其主题预览卡片（`renderThemeOption`）和「跟随系统」开关仍残留 37 处行内 `style={{}}` 纯视觉布局（圆点/侧栏预览/主区预览/网格/pane 等），违反 Phase 8「不允许新增 `style={{}}` 做纯视觉」的收口规则。本轮将 ThemePanel 整体迁移到 `ai-settings-theme-*` 作用域：新增 `ai-settings-theme-body`（flex 容器）、`-system-toggle`（+ `is-active`）/`-toggle-left`/`-switch-track`（+ `is-active`）/`-switch-thumb`（transform 由 class 接管）/`-label-stack`/`-label-title`/`-status-pill`/`-manual-head`/`-section-label`/`-grid`（grid 布局）/`-option`（+ `is-selected`，border/background/box-shadow）/`-preview`（+ `is-dark`/`is-light`，106px 固定高）/`-preview-dots`/`-preview-dot`/`-preview-grid`（+ mode 变体的 grid-template-columns）/`-preview-sidebar`（+ mode 变体）/`-preview-bar`/`-preview-main`（+ mode 变体 gradient/border）/`-preview-main-head`/`-preview-accent-bar`/`-preview-square`（+ mode 变体）/`-preview-content`（grid）/`-preview-pane`/`-preview-pane-large`/`-preview-pane-col`/`-preview-pane-tall`/`-preview-pane-fill`（均含 mode 变体 background/border）/`-option-meta`/`-option-head`/`-option-title`/`-option-desc` 等 30+ class。组件行内 `style={{}}` 从 37 处降至 9 处，剩余 9 处均为合法动态值（6 处 `previewAccent` + `opacity` 运行时颜色参数、1 处 `isDark` 动态 `caretColor`、1 处 `size` prop 动态 `width/height`、1 处 dot 的 `previewAccent`）。简化了 `renderThemeOption` 签名（移除 `previewBackground`/`previewBorder`，改由 `is-dark`/`is-light` class 承接）。验证：`pnpm exec tsc --noEmit`、`pnpm lint`、`pnpm build` 全部通过。

## 6. 每个阶段的固定验收清单

每完成一个阶段，必须执行：

```bash
pnpm exec tsc --noEmit
pnpm lint
pnpm build
```

如该阶段涉及交互页面，还需要用 Tauri 或浏览器截图验证：

- 1280x800：桌面常规窗口
- 1600x1000：宽屏工作区
- 390x844：窄屏或最小宽度退化检查，至少确认不重叠

视觉验收必须检查：

- 文本不溢出按钮/标签/卡片
- 面板内不出现卡片套卡片
- 侧栏折叠/展开不挤压主内容
- 输入框、菜单、弹窗层级正确
- loading、empty、error、disabled 状态都有新视觉

## 7. 完成标注规范

### 7.1 文档标注

迁移完成后，在第 5 节对应任务改为 `[x]`，并补一行：

```md
完成标注：`ai-migrated-xxx`，验证：tsc/lint/build + 截图
```

### 7.2 代码标注

每个已完成迁移的主要页面根节点必须有 scope class：

| 区域 | 建议 class |
|---|---|
| Home | `ai-home-shell ai-migrated-home` |
| Project 工作区 | `ai-project-shell ai-migrated-project` |
| 文件工作台 | `ai-files-shell ai-migrated-files` |
| Git 工作流 | `ai-git-shell ai-migrated-git` |
| 设置 | `ai-settings-shell ai-migrated-settings` |
| Browser/MCP/Terminal | `ai-ops-shell ai-migrated-ops` |

小组件不需要都加 `ai-migrated-*`，但必须使用所在区域 scope 下的 `ai-*` class 或 `components/ui` 基础组件。

### 7.3 禁止用注释假装完成

不要只写 `// migrated`。完成标注必须同时满足：

- 代码根节点可定位
- 文档 checkbox 已更新
- 验证命令通过
- 关键交互人工检查过

## 8. 逻辑回归保护清单

以下逻辑在 UI 迁移中最容易丢，必须逐项确认：

- [x] 普通聊天：新建会话、切换会话、搜索、分类展示、分类管理
- [x] 聊天消息：流式输出、工具调用、ArtifactPanel、SubAgent 轨迹、重新生成
- [x] 输入框：Enter 发送、Shift+Enter 换行、图片附件、停止/恢复
- [x] Project chat：dispatcher 子任务审批、继续、退出、结果回注
- [x] Project 工作区：SessionPanel 折叠、ProjectRail 切换、右侧工具栏、编辑区 split
- [x] 文件：打开、关闭、保存、重命名、删除、图片预览、大文件提示
- [x] Git：stage、unstage、commit、push、pull、branch 创建/切换、diff 打开
- [x] 设置：Aha 模型、分类 agent config、RAG、SSH、SubAgent 保存
- [x] Browser/MCP/Terminal：浏览器 dock 恢复、MCP 开关、shell 输入输出

> 确认方式：本轮（2026-07-09）迁移逐项核对每个改动组件的 Tauri 命令接线与回调 props 是否原样保留（`onSwitch`/`onOpen`/`onExpandSessionSidebar`/`onInput`/`onResize`/`onRegisterTerminal`/`onTerminalReady`/`onSnapshot`/`onRun`/`onStop`/`onClear`/`onClose` 等），未删除或重命名任何业务入口；hover/active 状态从 JS 状态机改为 CSS `:hover` 后语义等价；`main.tsx` 复用共享 `ErrorBoundary` 后 reset 语义一致。逻辑回归的「真机点击」建议在下一轮 `pnpm tauri dev` 时按上述清单逐项过一遍。

## 9. 建议下一步

Phase 0–8 全部完成，科幻 UI 全量迁移已收口。剩余可选优化：

1. **真机回归**：用 `pnpm tauri dev` 启动完整应用，按第 8 节清单逐项点击验证；重点关注本轮改动（ProjectRail hover/active 由 CSS 接管、SubProcessTabs 终端显隐、PythonRunDrawer 运行/停止/清空、HomeChatPage 浏览器 dock resize、根级 ErrorBoundary、亮色主题 accent retune）。
2. ~~**亮色主题科幻化**：`App.css :root`（浅色）仍沿用品牌绿 accent，与暗色科幻青不一致；如需统一可在亮色 token 上做配套 retune。~~ **已完成（2026-07-10）**：`App.css :root`（浅色）的 accent 家族从品牌绿（`--accent: #10b981` / `--accent-strong: #059669` / `--accent-hover: #047857` / `--accent-soft` / `--accent-subtle` / `--bg-selected: #d7f4e8` / `--bg-hover` / `--border-focus`）统一 retune 为科幻青色家族（`--accent: #0d9488` / `--accent-strong: #0f766e` / `--accent-hover: #115e59` / accent-soft/subtle/bg-selected/bg-hover/border-focus 同步），与暗色 `html.dark` 的 `#21f4df` 家族形成亮暗配套。`--chat-shell-bg` 径向渐变、`--chat-sidebar-bg`、`--chat-main-rail`、`--chat-surface`、`--chat-glass-border`、`--chat-focus-ring` 同步 retune；`--markdown-*` 家族（inline/code/math 的 bg/border/text/shadow）从钴蓝改为青色（`rgba(13,148,136,*)` 家族），与 accent 一致。已删除零引用的 `--brand-acid` / `--brand-acid-strong` 死变量。Git diff 的 light 语义色（add 绿 / del 红）保留行业标准不改。验证：`pnpm exec tsc --noEmit`、`pnpm lint`、`pnpm build` 通过。
3. **大组件继续拆分**：`ProjectPage.tsx`（1264）、`LargeFileViewer.tsx`（1126）、`AppSettingsDialog.tsx`（722）仍超 400 行红线（AGENTS.md 要求），新增功能前应先拆 presentational 子组件。
4. **monaco-vendor 体积治理**：4MB+，按需 `import()` 语言包（AGENTS.md 已列为技术债）。
