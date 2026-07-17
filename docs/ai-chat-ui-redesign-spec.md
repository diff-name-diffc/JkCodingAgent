# AI Chat UI 重设计规范

> 目标：把应用主题统一为「现代 AI 聊天」风格 —— 干净、克制、内容优先（Claude / ChatGPT 级别的简洁感）。
> 亮色单主题（无暗色模式），单一品牌色（深青 teal），暖白纸面，柔和圆角，清晰层级。
> 本文件是本轮重设计的唯一设计依据，所有样式/组件修改遵循它。

## 0. 硬性规则

- **业务逻辑零改动**：数据流、事件、Tauri 命令、流式管道、快捷键、审批流全部保持原样。只改视觉与布局结构。
- **类名稳定**：组件正在使用的 `.ai-*` 类名保持不变（只改其样式定义），除非组件与 CSS 在同一次修改中同步更新。
- 样式仍集中在 `src/styles/tailwind.css` 的 `@layer components`（`.ai-*` 类）+ `src/App.css`（令牌与全局基线）。组件内用 Tailwind 工具类 + `cn()`。
- 颜色一律引用 `App.css` 的 CSS 变量，**禁止新的硬编码色值**（含 rgba）。需要 alpha 时用 `rgb(var(--accent-rgb) / 0.5)` 形式。
- 仅亮色主题，不留暗色分支。圆角用 `--radius-*`，阴影用 `--shadow-*`，动效用 `--motion-*`。
- 单文件组件不超 400 行（新拆分时遵守）。

## 1. 设计令牌调整（`src/App.css`）

在现有「暖纸 + 深青」令牌基础上：

```css
/* 新增 RGB 伴随令牌（用于 alpha 合成） */
--accent-rgb: 41 124 112;        /* = --accent #297c70 */
--accent-strong-rgb: 31 102 93;
--danger-rgb: 220 38 38;
--warning-rgb: 249 115 22;
--success-rgb: 22 163 74;
--info-rgb: 37 99 235;

/* 修复：引用了但从未定义 */
--text-error: var(--danger);

/* 新增聊天气泡令牌 */
--chat-user-bubble-bg: #dcebe7;        /* 柔和青调用户气泡 */
--chat-user-bubble-text: var(--text-primary);
--chat-composer-shadow: 0 8px 30px rgb(23 32 29 / 0.08);
```

字体治理（`src/main.tsx` + `App.css`）：
- 删除 `@fontsource/inter` 引入（无任何 font-family 使用 Inter）。
- 删除 `App.css` 顶部的 Google Fonts `@import`（JetBrains Mono 已由 `@fontsource/jetbrains-mono` 本地提供，桌面应用不应运行时联网拉字体）。

## 2. 全局硬编码科幻色清除（`tailwind.css` 非聊天/非设置区）

现存 ~500 处硬编码 rgba 色（旧科幻主题残留）与新令牌冲突，全部替换：

| 旧值 | 替换为 |
|---|---|
| `rgba(33,244,223,a)` / `rgba(94,242,234,a)` / `#21f4df`（青） | `rgb(var(--accent-rgb) / a)` 或对应 `--accent-*` 令牌 |
| `rgba(255,95,122,a)`（粉） | `rgb(var(--danger-rgb) / a)` |
| `rgba(255,189,90,a)`（琥珀） | `rgb(var(--warning-rgb) / a)` |
| 深色科幻背景色（如 `#0a…`、`#1…` 深青黑） | 对应 `--bg-*` / `--bg-card` / `--bg-elevated` |
| 发光阴影 | `--shadow-accent-glow` 或删除 |

同步现代化核心区类（`.ai-dialog-*`、`.ai-primary-button`、`.ai-secondary-button`、`.ai-button*`、`.ai-context-menu-*`、`.ai-field-*`、`.ai-list-row`、`.ai-status-pill*`、`.ai-empty-state*`）：主按钮 = `var(--accent)` 底白字、`--radius-md`；对话框 = `--bg-elevated` 底、`--radius-xl`、`--shadow-lg`。

## 3. 聊天界面重设计

### 布局（`app-layout.tsx`）
- 阅读列宽 `max-w-[920px]` → **`max-w-[768px]`**（聊天阅读最佳宽度），居中。
- 主区背景 `--chat-surface`（纸面），侧栏 `--bg-sidebar`，分界 `1px var(--border-dim)`。
- 头部：精简，毛玻璃 sticky；底部输入区无边框线，直接融为背景（输入卡片自带阴影浮起）。

### 会话侧栏（`sidebar.tsx`）
- 顶部：收起按钮 + 新建分类按钮（保持现有结构，样式精致化）。
- 搜索框：`--radius-md`，`--bg-input`，无边框或淡边框，聚焦 `var(--border-focus)` 描边。
- 分类头：13px 半粗、次要文字色，chevron 旋转动效保留；计数为小号灰 pill。
- 会话行：`--radius-md`，hover `var(--bg-hover)`，**active = `var(--bg-selected)` + 主文字色**；运行中圆点 = `var(--accent)` 呼吸；关键词 tag = 极小灰 pill；行尾 `MoreHorizontal` 菜单触发器仅 hover 显现。
- 折叠态 rail：图标按钮垂直排列，保持现状仅润色。

### 消息流（`message-item` / `user-message` / `assistant-message` / `streaming-message`）
- **用户消息**：右对齐气泡，`--chat-user-bubble-bg` 底、`--radius-xl`（18px），max-w-[75%]，padding 10px 14px，无头像。
- **助手消息**：**无气泡**，整栏宽；左侧 28px 圆角方形头像（青调底 + app logo / Sparkles 图标）；正文 `--text-primary`，markdown 排版保持；操作按钮（复制/重新生成）= 幽灵小图标，**hover 消息时显现**。
- **思考块**：可折叠，次要文字色 + 左侧 2px `var(--border-medium)` 竖线或淡底，标题行小字「思考过程」。
- **被覆盖的中间推理**：折叠灰块，样式与思考块一致。
- **流式光标**：`var(--accent)` 色闪烁竖条。
- 消息间距：turn 之间 28px，气泡与工具卡之间 12px。

### 工具调用卡（`tool-call-card.tsx`）
- `var(--bg-card)` 底、1px `var(--border-dim)`、`--radius-lg`、无重阴影。
- 头部：状态 pill（待执行=muted / 执行中=accent 呼吸 / 完成=success / 失败=danger）+ 工具名（mono 小字）+ 摘要一行省略 + chevron。
- 展开区：输入参数用浅底代码块，输出摘要 markdown；「查看执行轨迹」为 accent 文字按钮。
- 子智能体执行卡（`SubAgentExecutionView` / `artifact-panel`）：同一套卡片语言，阶段 stepper 用 accent 色。

### 输入区（`prompt-input.tsx` / `model-selector.tsx`）
- 容器：`--radius-xl`（16-20px）、`--bg-elevated` 底、1px `var(--border-medium)`、`--chat-composer-shadow`；**focus-within 时边框变 `var(--border-focus)` + 淡青外发光**。
- 文本域：15px、行高 1.6、无内边框、auto-grow（保留逻辑）。
- 工具栏：模型选择器 = 小 pill（`--bg-hover` 底、圆角 full、chevron）；右侧发送按钮 = **32px 圆形、`var(--accent)` 底、白色 ArrowUp**，禁用 = `var(--bg-hover)` 底 + hint 色图标；停止 = danger 圆形。
- 占位文案保持。

### 空状态（`empty-chat-state.tsx`）
- 垂直居中：app logo（56px，圆角 16px，柔和阴影）→ 20px 半粗标题「有什么可以帮你的？」→ 2×2 建议卡片栅格（`--bg-card`、`--radius-lg`、hover 上浮 + 边框变 accent）。
- 移除 `ai-orb` 发光球装饰。

### 审批面板 / 命令面板（`dispatch-approval-panel.tsx` / `command-palette.tsx`）
- 审批：居中卡片（480px），标题 + 可编辑任务描述 + 底部「拒绝 / 批准」按钮（次/主）。
- 命令面板：顶部居中浮层（560px），`--bg-elevated`、`--radius-xl`、大阴影，搜索行 + 分组列表，选中行 accent-soft 底。

### 项目内聊天头（`chat-page-v2.tsx` 的 projectHeader）
- 单行工具条：MCP 状态 pill、自动审批开关、清空按钮；全部小号 ghost/pill 风格，右对齐。

### 聊天 CSS 落地
- **删除两代重复定义**：`tailwind.css` 中旧聊天段（约 4269–4585 行）与新一代段（约 8166–9220 行）整体移除，重写为一段干净实现（类名不变）。
- **删除 `.ai-tool-activity-*` 全族**（对应死组件 ToolActivityBubble，约 88 处）。
- 修复 `.chat-scroll` 滚动条硬编码青色 → `rgb(var(--accent-rgb) / 0.35)`。
- 删除 `@keyframes ai-orbit` 等无引用动画。

## 4. 设置界面重设计

### 对话框骨架（`AppSettingsDialog.tsx`）
- 遮罩：`rgb(23 32 29 / 0.32)` + 轻微 backdrop-blur。
- 壳：宽 960px、高 min(84vh, 720px)、`--radius-xl`、`--bg-elevated`、`--shadow-lg`，左右分栏。
- 左导航 200px：`--bg-panel` 底，标题「设置」15px 半粗；导航项 = 图标 + 文字、`--radius-md`、hover `--bg-hover`、**active = `--bg-selected` + accent 文字**；`⚙` 字符换 lucide `Settings` 图标。
- 右内容区：头部 = 面板标题 + 关闭按钮（ghost 圆形）；内容滚动区 padding 28px、max-w-[680px]。

### 面板通用语言（aha / rag / ssh / sub-agents / claude / codex 全部适用）
- 节标题：15px 半粗 + 下方 hint 小字（`--text-muted`）。
- 卡片：`--bg-card` + 1px `--border-dim` + `--radius-lg` + padding 16px。
- 输入控件：高 34px、`--radius-md`、1px `--border-medium`、聚焦 `--border-focus` + 细外环；label 12px 半粗次要色；hint 12px muted。
- 主按钮 = accent 底白字 `--radius-md`；次按钮 = 边框 `--bg-card`；危险按钮 = danger 文字/边框。
- 开关（toggle）：accent 轨道。
- tab（如 AhaAgentPanel 顶 tab）：下划线式，active = accent 文字 + 2px accent 下划线；sub-tab = pill 式。
- 移除 `ai-settings-panel-host` 的脆弱后代覆盖（用通配选择器强改子控件的做法），改为显式类。
- 日志/代码视图（RAG 日志、配置文件查看）：mono 字体、`--bg-input` 底、`--radius-md`。

### 设置 CSS 落地
- 重写 `.ai-settings-*`、`.ai-aha-*`、`.ai-rag-*`、`.ai-ssh-*`、`.ai-subagent-panel/dialog-*`、`.ai-mcp-*` 段（约 2798–4267 行 + 各处散落的 mcp 段），删除重复与硬编码色。

## 5. 死代码清理

- 删除 `src/components/ToolActivityBubble.tsx`（377 行，组件本体无 JSX 引用）；`ToolActivityItem` 类型移至 `src/components/dispatcher-chat/tool-activity.ts`，更新全部 import。
- 删除 `src/components/ui/card.tsx`（无引用）。
- `src/components/dispatcher-chat/dispatcherChatUtils.ts`：删除无引用导出（`MESSAGE_LIST_BOTTOM_THRESHOLD`、`SEARCHABLE_CONTENT_SELECTOR`、`SEARCH_MATCH_SELECTOR`、`isMessageListNearBottom`、`unwrapConversationSearchMatches`、`highlightConversationSearchMatches`、`withLiveElapsed`、`getComposerButtonLabel`、`isComposerActionDisabled`、`getPrimaryComposerOpacity`、`getSubProcessAgentLabel`），删除前逐一 grep 确认。
- 合并重复 hook：`src/hooks/use-live-session-state.ts`（40 行只读桥）并入 `src/components/dispatcher-chat/useLiveSessionState.ts`，更新 `chat-shell.tsx` 的 import。
- 删除无引用导出：`assistant-message.tsx` 的 `ArtifactRefBadges`、`message-item.tsx` 的 `messageItemClass`、`subAgentEventStore.ts` 的 `useSubAgentProgressMessages`（删除前 grep 确认）。
- 字体：见 §1。

## 6. 验收

- `pnpm build`（tsc + vite）零错误；`pnpm lint` 零警告。
- `pnpm dev` 启动后截图检查：聊天空状态、会话侧栏、消息流（含工具卡）、输入区、设置五个面板。
- 目视标准：无任何科幻青/粉残留色；主题统一为暖纸 + 深青；圆角/阴影/间距一致。
