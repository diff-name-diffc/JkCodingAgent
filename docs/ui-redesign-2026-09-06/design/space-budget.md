# UI-05 空间预算可实现性记录

关联：[关键帧](keyframes.html) · [令牌定稿](tokens.md) · 实现 `src/components/project/workspace-budget.ts` + 测试 `workspace-budget.test.ts`（commit `77c57f6`）。

本文件直接引用 UI-04 参数化测试的期望值，证明 02-design §3 的空间预算**已被纯函数实现并可机器验证**（`pnpm vitest run src/components/project/workspace-budget.test.ts`，32 case 全绿）。

## 1. 区域尺寸契约（02-design §3.1 → 常量）

| 区域 | 定稿 | 实现常量 |
|---|---|---|
| 顶栏 | 40 高（兼容 38 安全区） | `DEFAULT_CHROME.titlebarHeight = 38`（现状安全区，UI-07 调 40 时改参数） |
| 全局 rail | 52 | `railWidth` 参数化：当前外壳传 56，UI-07 后传 52 |
| 上下文导航 | 默认 248，范围 216–320 | `navMin/navMax = 216/320`；偏好宽当前 288（UI-08 持久化） |
| 右工具栏 | 移除 | `toolbarWidth` 参数化：当前 48，UI-07 后 0 |
| 底部 dock / 状态 | 220 / 24 | 终端偏好高 240，`terminalMin/MaxHeight = 100/600` 夹取 |
| 主区可用高下限 | 320 | `minMainHeight = 320`（测试断言矮窗终端被压缩保住下限） |
| 双栏最低宽 | 对话 460 / 代码 520 / 分隔 8 | `minChatWidth/minEditorWidth/splitterWidth` |

## 2. 测试锚点（机器验证的期望值）

- **不变式**：1000/1280/1440/1600/1920 × 680/800/900/1000/1080 × {导航开/关 × 右面板开/关 × 终端开/关 × 双栏请求开/关} = 288 组合：
  - 所有区域宽/高 ≥ 0（无负宽度）；
  - 水平闭合 `rail + nav + main + right + toolbar == viewportWidth`；
  - 双栏时 `chat + splitter + editor == mainWidth` 且两栏分别 ≥ 460/520；
  - 终端开启时 `mainHeight + terminalHeight + titlebar == viewportHeight`。
- **降级序**（有序 `degradations`）：
  - 1000 + 右面板 + 双栏请求 → `[right-panel-closed, single-column]`，main = 608；
  - 760 + 右面板 → `[nav-collapsed, right-panel-clamped(→180)]`；
  - 620 + 右面板 → `[nav-collapsed, right-panel-closed]`，main = 516；
  - 680 高 + 终端偏好 600 → `terminal-height-clamped(→322)`，mainHeight = 320。
- **宽屏锚点**（UI-07 外壳参数 {52, 0}）：1600 → nav 248、main 1300、双栏 chat/editor 各 646（ratio 0.5），与 02-design §3.2 样例表同构。
- **偏好冻结**：窄窗调用后输入对象逐字段相等（自动适配不写回用户偏好）。

## 3. 与关键帧的对应

| 关键帧 | 预算状态 |
|---|---|
| S1（1440） | nav 248 展开 + 单栏聊天：`navWidth=248, dualPane=false` |
| S1-N（1000） | `nav-collapsed` + 单栏：帧中导航缺席即此降级 |
| S2（1440+） | `dualPane=true`：chat ≥460 / editor ≥520 / splitter 8 |
| S4 | 执行图为主区标签（UI-13 **已兑现**：`a9f7064/4c7381c`，graph 标签走 editorPane 预算宽），不占额外水平预算；浏览器预览标签同构（UI-18 `860bf88`） |
| S5 | 助手 320–400 为架构空间自有预算（ArchitectureView 现状宽度状态），本轮不改 |

## 4. 未决项

- 顶栏 38→40 与 rail 56→52 的实际切换在 UI-07 提交时同步改 `DEFAULT_CHROME` 调用参数并重跑测试；
- ~~浏览器面板 expanded 的视口比例语义（`useDockedBrowserPanel`）未纳入预算~~ **已兑现（UI-18 `860bf88`）**：项目侧浏览器迁入主区标签，宽度由预算 grid 决定，expanded 视口比例语义删除（hook 仅 HomeChatPage 独立聊天仍用）；
- **新登记（UI-18 后）**：预算 `rightPanelOpen/rightPanelWidthPref` 入参已无真实消费者（ProjectPage 恒传 `false/0`），纯函数与 32 测试保留为通用能力——UI-27 评估移除该降级通道；`prefs.rightPanelWidth` 与 localStorage `nezha.project.browserPanelWidth` 成为孤儿数据，UI-27 一并清理；
- 多窗口尺寸独立预算不在本轮范围（02-design §7 已登记暂缓）。
