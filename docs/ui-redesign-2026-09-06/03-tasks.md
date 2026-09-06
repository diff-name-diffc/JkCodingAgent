# 改造任务明细与跟踪清单

更新日期：2026-09-06。关联：[审查](01-audit.md) · [设计规格](02-design.md)。

**当前：UI-01–10（M0+M1）实施完成一轮；M2 批次一（UI-11/12/13/14/18/20）、批次二（UI-16/17「代码工作区→Git 审查」链）与批次三（UI-15/19「工作区资源保活」对）实施完成——M2 全部收口**——UI-01 部分 BLOCKED（交互走查不可达，见记录），UI-05 DONE，其余 REVIEW（自动化门禁全绿，人工截图验收遗留至 UI-29/31）。M0+M1 源码基线 `04df59a`，实施提交 `e4d2b82..15e50be`；M2 批次一实施提交 `4ee7c71..633ec4a`；批次二实施提交 `88babe4..9a20640`；批次三实施提交 `f4cab0a`、`f4f63a9`。下一步 M3（UI-21 起）。

## 1. 跟踪约定

状态统一使用 `TODO / DOING / BLOCKED / REVIEW / DONE`。下表状态是唯一任务状态来源；勾选清单仅记录交付门槛。负责人中的“待分配”不是已指派人员。

从 TODO 开始时写负责人、日期和分支；BLOCKED 写具体阻塞与解除条件；只有验收通过、链接到 PR/提交和检查证据后才能标记 DONE。发现基线已修复的问题，仍需复现证明并记录“基线已满足”，不得重复实现。

工作量为熟悉仓库工程师的粗估：S 约 0.5–1 人日，M 约 1–3 人日，L 约 3–5 人日；不含等待反馈和未知环境问题。它是拆分参考，不是交付承诺，UI-01/04 完成后重新估算。

## 2. 里程碑

| 里程碑 | 范围 | 完成门槛 |
|---|---|---|
| M0 基线与布局止损 | UI-01–04 | 当前构建证据齐备，正文/输入可见，执行图无遮挡，空间策略有边界测试 |
| M1 视觉与外壳 | UI-05–10 | 浅深色关键帧、令牌和基础组件一致；统一导航与面板状态落地 |
| M2 任务工作流 | UI-11–20 | 对话→执行→成果→审查→验证链路贯通，运行资源生命周期正确 |
| M3 全局一致性 | UI-21–27 | 设置/连接/空态/键盘/性能与品牌术语收敛 |
| M4 发布验收 | UI-28–32 | 功能映射无丢失，视觉/性能/用户走查通过，可逐批回退 |

关键依赖：`01 → 04 → 05/07 → 08/09 → 11–20 → 28–32`。UI-02/03 为独立快修，不等待完整视觉稿。相邻任务可由团队协调并行，但共用样式/状态文件必须指定单一修改负责人。

## 3. 任务总表

所有文件路径相对仓库根目录。具体证据格式见第 5 节。

| ID | 优先级/规模 | 任务 | 依赖 | 状态 | 负责人 | 交付/证据 |
|---|---|---|---|---|---|---|
| UI-01 | P0 / M | 当前构建视觉基线与缺口补验 | — | BLOCKED | claude（会话领取） | evidence-baseline/ 5 帧 + manifest；V03–V07 走查阻塞登记 |
| UI-02 | P0 / M | 聊天、输入和详情容器宽度修复 | 01 | REVIEW | claude（会话领取） | `25aa667`；门禁全绿，A01 同尺寸前后截图遗留 |
| UI-03 | P0 / S | 执行图覆盖层与焦点止损 | 01 | REVIEW | claude（会话领取） | `3d9e59c`；portal+层级栈+焦点还原，遮挡前后截图遗留 |
| UI-04 | P0 / M | 工作区空间预算与自适应规则 | 01 | REVIEW | claude（会话领取） | `77c57f6`；budget 纯函数 32 case 参数化测试 |
| UI-05 | P1 / M | 浅深色关键帧与视觉规格定稿 | 04 | DONE | claude（会话领取） | `9bccc43`；design/ 关键帧+tokens+space-budget，评审结论已记录 |
| UI-06 | P1 / M | 令牌、基础控件、表面样式统一 | 05 | REVIEW | claude（会话领取） | `39e93dd/6eeff1f/24da7ea` 三阶段；对比度实测遗留 UI-29 |
| UI-07 | P1 / L | 统一应用外壳与上下文导航 | 04,06 | REVIEW | claude（会话领取） | `bb3c0e9/f41134d`；AppRail+ContextNav+StatusDockBar，保活走查遗留 |
| UI-08 | P1 / L | 工作区布局状态与偏好迁移 | 04,07 | REVIEW | claude（会话领取） | `289cb3f`；workspace-store 三层+sanitize 测试；多窗口不纳入本轮 |
| UI-09 | P1 / L | 主区标签、详情槽和抽屉统一 | 02,03,08 | REVIEW | claude（会话领取） | `afb341d`；main-tabs 8 case+Sheet/Dialog+请求守卫；窄屏 Artifact 抽屉遗留 |
| UI-10 | P1 / M | 最近项目与会话导航精修 | 07,08 | REVIEW | claude（会话领取） | `15e50be`；recency 排序修复+列表语义；50 项目/大会话人工操作遗留 |
| UI-11 | P1 / M | 任务头部与输入框上下文 | 06,09 | REVIEW | claude（会话领取） | `4ee7c71`；标题/项目/分支双行头部+更多菜单+停止态；窄窗走查遗留 |
| UI-12 | P1 / M | 消息层级与工具活动摘要 | 06,11 | REVIEW | claude（会话领取） | `307dc73`；语义摘要纯函数+StatusPill 双编码+失败 pinned 露出；主题比对遗留 |
| UI-13 | P1 / L | 执行图迁入工作视图 | 09,12 | REVIEW | claude（会话领取） | `a9f7064/4c7381c`；graph 主区标签+portal 删除+视图记忆；运行态走查遗留 |
| UI-14 | P1 / M | 子智能体与节点详情统一 | 09,13 | REVIEW | claude（会话领取） | `05bc50f`；概览/活动/输出三段+共享 detail 组件+footer 门禁文案对齐 |
| UI-15 | P1 / M | 架构画布与助手体验 | 06,08 | REVIEW | zcode（会话领取） | `f4cab0a`；宽度像素锚定+领域空态+附加上下文；画布保活三项基线已满足登记 |
| UI-16 | P1 / M | 文件树与编辑器视觉整合 | 07,09 | REVIEW | zcode（会话领取） | `88babe4`；壳归位+平面化+路径行+tab 脏标记；截图遗留 UI-29 |
| UI-17 | P1 / L | Git 审查布局与提交反馈 | 09,16 | REVIEW | zcode（会话领取） | `8e09fca/9a20640`；解析抽离+重命名/二进制+提交区范围+就地错误+对应高亮；split 模式登记待后续 |
| UI-18 | P1 / L | 浏览器预览与会话 dock 迁移 | 09 | REVIEW | claude（会话领取） | `4169175/860bf88`；拆分+串帧修复+主区单例标签，右面板机制移除；dock 走查遗留 |
| UI-19 | P1 / M | 终端 dock 与空间约束 | 08,09 | REVIEW | zcode（会话领取） | `f4f63a9`；隐藏保活两态机+结束会话语义+高度常量收敛；PTY 运行态走查遗留 |
| UI-20 | P1 / M | Python、图片和工具产物详情 | 09,12 | REVIEW | claude（会话领取） | `633ec4a`；来源/耗时/状态统一+artifact kind 分派；运行态走查遗留 |
| UI-21 | P1 / M | 设置布局与自动保存反馈 | 06,07 | TODO | 待分配 | — |
| UI-22 | P1 / M | MCP/SSH/RAG 状态及作用域提示 | 11,21 | TODO | 待分配 | — |
| UI-23 | P1 / L | 键盘、焦点与输入法整合 | 07,09,11,21 | TODO | 待分配 | — |
| UI-24 | P1 / L | 长列表、流式输出和布局性能 | 08,12,19 | TODO | 待分配 | — |
| UI-25 | P1 / S | 各场景空态/加载/错误规格落地 | 10,12,15,20 | TODO | 待分配 | — |
| UI-26 | P2 / S | 产品术语与图标视觉统一 | 06,21,22 | TODO | 待分配 | — |
| UI-27 | P2 / M | 移除旧样式与失效布局说明 | 06,07,09,26 | TODO | 待分配 | — |
| UI-28 | P1 / L | 核心功能映射回归 | 10–23,25 | TODO | 待分配 | — |
| UI-29 | P1 / M | 多尺寸双主题视觉与可访问性验收 | 23,25,26,27 | TODO | 待分配 | — |
| UI-30 | P1 / M | 性能与多工作区生命周期验收 | 18,19,24 | TODO | 待分配 | — |
| UI-31 | P1 / M | 开发者任务走查与问题修订 | 28,29,30 | TODO | 待分配 | — |
| UI-32 | P1 / S | 分批发布、回退与文档收尾 | 27–31 | TODO | 待分配 | — |

## 4. 实施任务卡

### M0：基线与布局止损

**UI-01｜当前构建视觉基线。** 入口：`package.json`、`src-tauri/tauri.conf.json`、本目录 `evidence/`。使用隔离测试数据启动与 commit 对应的 Tauri 构建，记录 OS、窗口大小、缩放、主题和 commit；补齐 Artifact、终端、浏览器、Python、Git diff、模型错误与设置保存失败。验收：八个既有步骤有同构建对应截图；所有未覆盖项写明阻塞，A01/A02 标注是否复现。截图不得包含密钥或真实私人内容。

**UI-02｜容器宽度快修。** 修改：`src/components/layout/app-layout.tsx`、`src/styles/tailwind.css` 的 `.ai-chat-column`、`src/components/chat/prompt-input.tsx`。移除以 viewport 大档固定正文宽度的依赖；让消息与输入受父容器约束；在旧布局过渡期间为 420px Artifact 制定覆盖/替换策略。验收：主页/项目在开启文件树、编辑器、Artifact 各组合下无正文裁切；发送/停止按钮可见；代码仅在自身内部横向滚动。附 A01 同尺寸前后图。

**UI-03｜执行图图层快修。** 修改：`src/components/chat-page-v2/ChatPageOverlays.tsx`、`src/components/graph/GraphPanel.tsx`、相关 overlay 样式。以 root portal 或现有合适的根容器解决祖先 stacking context；修正 focus/Escape。验收：文件/Git/浏览器栏打开时，图头部所有动作与关闭按钮均可见可达；关闭恢复触发点；不遮挡更高层确认框；没有触发 start/resume 等副作用。

**UI-04｜空间分配规则。** 修改：`src/components/project/ProjectWorkspaceLayout.tsx`、`ProjectWorkbenchContent.tsx`、`src/hooks/useProjectPanels.ts`；新增小型纯函数模块，路径实施时登记。定义每个区域最小宽高、容器断点、自动收起和恢复优先级。验收：参数化测试覆盖 1000/1280/1440/1600/1920、导航开关、内容双栏、dock、详情组合；可用宽不足时得到明确单栏策略，不产生负宽度；自动适配不覆盖用户偏好。

### M1：视觉与外壳

**UI-05｜关键帧定稿。** 输出：本目录新增 `design/` 设计参考。至少给出专注对话、代码协作、Git 审查、执行图、架构与设置的主状态，覆盖浅/深色及 1000px 窄窗代表帧；标注排版、令牌和区域宽度。验收：UI-04 空间预算可实现，所有视觉入口都对应现有能力或明确标为后续功能；记录视觉评审结论，再进入大范围换肤。

**UI-06｜设计系统收敛。** 修改：`src/App.css`、`src/styles/tailwind.css`、`tailwind.config.js`、`src/components/ui/`。统一背景/文字/边框/状态和 RGB alias；减掉常驻栏投影与发光，统一按钮/标签/输入密度。验收：浅深色 hover/active/focus/disabled/error 齐备；无新增业务 CSS 或竞争性主题方案；基线重复规则按组件迁移，不盲删公共类。执行 build、lint、styles:report。

**UI-07｜统一导航外壳。** 修改：`src/App.tsx`、`WelcomePage.tsx`、`ProjectPage.tsx`、`ProjectRail.tsx`、`RightToolbar.tsx`。建立应用 rail + 上下文导航，文件/变更入口移入导航；保留项目切换、打开与关闭。验收：聊天/项目/架构互转入口位置一致；项目身份始终可辨；已打开项目关闭不等于删除项目；隐藏项目不丢失运行状态。

**UI-08｜工作区状态归属。** 修改：`src/stores/ui-store.ts`、`src/hooks/useProjectPanels.ts`、`src/components/project/`、`layout/app-layout.tsx`。区分全局偏好、每工作区布局、每会话临时详情；在已有 Zustand 中组织，勿复制业务数据。验收：拖动结束持久化，非法旧值校验；切换项目不串面板；窄窗恢复宽窗不覆盖原偏好；多窗口支持不纳入本轮。

**UI-09｜主区与详情容器。** 修改：`src/components/project/ProjectWorkbenchContent.tsx`、`layout/app-layout.tsx`、`artifact/artifact-panel.tsx`、`ChatPageOverlays.tsx`。建立代码/差异/预览/执行图/详情的活动标签与返回行为；窄屏抽屉用 Radix。验收：详情不形成第三个强制宽栏；关闭返回原内容/滚动位置；跨会话异步详情不串台；文件标签与 Git diff 的既有行为无丢失。

**UI-10｜项目与会话列表。** 修改：`WelcomePage.tsx`、`SessionPanel.tsx`、`layout/sidebar.tsx`、会话行组件。最近项目优先，时间/运行标识简洁；统一列表 hover/选中/更多菜单；分类语义保留。验收：长中文标题可截断并查看全称；零/1/50 个项目与大量会话均可操作；搜索防抖、分页、选中保持和删除确认正常；不首屏扫描全部仓库。

### M2：任务工作流

**UI-11｜任务头部与输入。** 修改：`ChatPageHeaders.tsx`、`chat/chat-shell.tsx`、`chat/prompt-input.tsx`、`chat/model-selector.tsx`。任务标题和项目语境优先；清空归更多菜单；保留模型、图片、发送/停止。验收：长标题下关键动作不溢出；正在编辑、发送中、停止中、模型缺失可辨；不新增无后端语义的模式切换；IME 验收见 UI-23。

**UI-12｜消息与活动层级。** 修改：`chat/assistant-message.tsx`、`message-item.tsx`、`streaming-message.tsx`、`tool-call-card.tsx`、`tool-run-trace.tsx`。统一进度行、工具聚合、最终结果和错误层级。验收：实时/历史同一轮次显示一致；失败与待处理状态无需展开即可看见；折叠不隐藏错误；聚合计数可由记录复算，不把缺失轨迹视为成功。

**UI-13｜执行图工作视图。** 修改：`graph/GraphPanel.tsx`、`GraphPanelHeader.tsx`、`GraphStateInspector.tsx`、图节点相关组件。取消主流程的全屏模态定位，使用主区标签/扩大视图；压缩节点信息，共享状态按需显示。验收：计划待执行/运行/暂停或取消/失败/成功按真实状态映射；运行和验收两种结论独立；同一 planId 切视图不触发新运行；大图可定位活动/失败节点。

**UI-14｜子智能体与节点详情。** 修改：`SubAgentExecutionView.tsx`、`graph/GraphNodeDrawer.tsx`、`artifact/artifact-panel.tsx`。复用概览/活动/输出视觉；显示来源任务、模型、耗时与错误。验收：实时事件和历史恢复一致；空/缺失/解析失败有不同提示；返回图时保留所选节点与视口；完整重跑/断点继续文案与已有门禁语义一致。

**UI-15｜架构空间。** 修改：`architecture/ArchitectureView.tsx`、`architecture/chat/ArchitectureChatPanel.tsx`、`chat/empty-chat-state.tsx`。画布优先、助手宽度约束、领域空态、截图/快照上下文可见。验收：切主题/收起助手/拖宽不重建 editor，撤销栈和视口保留；许可/画布崩溃明确失败；无画布操作时不会发送截图或模型请求；不增加架构→运行 DAG 转换。

**UI-16｜文件编辑工作区。** 修改：`FileExplorer.tsx`、`file-explorer/`、`FileViewer.tsx`、`file-viewer/FileTabPane.tsx`、`src/App.css`。文件导航归位，代码面平面化，路径工具行收敛，保留代码/预览切换。验收：长路径、图片、大文件、Markdown、未保存内容可辨；关闭/重命名/删除和标签同步保持原语义；编辑器字体、滚动和选择不受外壳改变影响。

**UI-17｜Git 审查。** 修改：`GitChanges.tsx`、`GitHistory.tsx`、`GitDiffViewer.tsx`、`src/hooks/projectPanelsFileState.ts`。导航列表与 diff 对应，提交区说明暂存范围，错误就地反馈。验收：在可丢弃测试仓库覆盖暂存/取消暂存、重命名、二进制、空变更、提交失败、历史 diff；不在真实工作仓库做验证提交；默认不自动暂存/推送。UI-01 确认当前 diff 能力后再估算模式切换范围。

**UI-18｜浏览器预览迁移。** 修改：`BrowserPanel.tsx`、`BrowserDock.tsx`、`HomeChatPage.tsx`、`src/hooks/useBrowserSessionDock.ts`、`useDockedBrowserPanel.ts`。迁入主区预览，统一加载、错误、最小化和恢复。验收：同一会话页面可恢复；切会话不串帧；隐藏与关闭语义明确；窗口缩放后显示/坐标映射正确；迁移不绕过浏览器既有权限行为。

**UI-19｜终端 dock。** 修改：`ShellTerminalPanel.tsx`、`terminalShared.ts`、`ProjectWorkspaceLayout.tsx`。统一 dock 头部、高度上限、隐藏与恢复。验收：1000×680 时输入框和终端均有恢复路径；多项目终端不串流，resize 正确；隐藏恢复不重建 PTY；保持现有缓冲/批处理，不逐字触发全局渲染。

**UI-20｜Python 与产物。** 修改：`dispatcher-chat/PythonRunDrawer.tsx`、`artifact/artifact-panel.tsx`、`chat-page-v2/usePythonRunController.ts`、文件/图片预览组件。统一来源信息与运行输出布局。验收：运行/停止/失败/已完成和缺失文件均有可见反馈；大输出可滚动；图片仍走 chat-image 协议；编辑重发、清空和删除会话沿用资源清理语义。

### M3：全局一致性

**UI-21｜设置布局和保存。** 修改：`AppSettingsDialog.tsx`、`settings/use-aha-settings.ts`、`settings/providers/`、相关样式。去重复标题，窄分类 tabs 可用，外壳使用适当 Dialog 原语，展示保存状态。验收：400ms 自动保存契约保持；快速编辑/切页/关闭不丢值；保存失败不显示“已保存”，重试可用；密钥保持默认掩码；模型库引用与容量来源不变。

**UI-22｜连接状态和作用域。** 修改：`McpStatusDialog.tsx`、`settings/mcp/`、`settings/ssh/`、`app-settings/rag/`、头部状态组件。健康状态降噪，异常明确行动入口；全局/项目 MCP 分清。验收：加载/禁用/未配置/错误/可用不混淆；异常详情包含真实失败原因；SSH/MCP/RAG 错误不能被通用“离线”吞掉；不改审查门禁和配置作用域。

**UI-23｜键盘与焦点。** 修改：`src/hooks/use-chat-shortcuts.ts`、`SessionPanel.tsx`、统一外壳/分隔条/弹层。规范 Mod 快捷键、窗口焦点、ARIA 及 IME。验收：全键盘能选会话/切区/打开详情/关闭返回；左右或上下键调 separator；嵌套弹层 Escape 只处理最顶层；隐藏工作区不响应快捷键；Monaco/xterm/输入法不被抢键。

**UI-24｜动态列表与流式性能。** 修改：`chat/message-list.tsx`、`src/hooks/use-auto-scroll.ts`、会话列表和布局状态。将固定 180px 估高的窗口化评估为动态测量方案，优先复用已安装 `@tanstack/react-virtual`；隔离 resize、scroll、流式渲染。验收：300/301/1000 条消息含长代码和工具展开时无跳读/大片空白；滚到历史不被拉回；切布局后锚点正确；附实际 profile，不能只改算法后宣称更快。

**UI-25｜状态与空态。** 修改：`chat/empty-chat-state.tsx`、`ProjectLazyPaneFallback.tsx`、相关领域空态。普通聊天、项目、架构分别有起步提示；首次加载用占位，失败可重试，搜索为空不等于数据加载失败。验收：每个入口至少有 empty/loading/error/ready 证据；模型未配置直接到对应设置页；不创建装饰性不可用按钮。

**UI-26｜术语与图标。** 修改：`RightToolbar.tsx`、文件头部、消息头像/空态、设置和错误文案。以当前产品名 JKCodingAgent 为基准，统一“会话/任务/执行图/架构图/浏览器”语义；内部技术名只留在必要详情。验收：Files 等用户界面中英文混排清理；Logo 不重做；所有图标按钮有可理解名称，图标尺寸与线重一致。

**UI-27｜旧规则清理。** 修改：`src/App.css`、`src/styles/tailwind.css`、已迁移布局文件注释、`AGENTS.md`/README 相关事实。逐项移除无引用类、重复主题赋值、旧布局 `!important`；更新 Artifact “overlay”、单浅色主题、未使用命令面板等过时注释。验收：styles:report 双向检查，亮暗主题无遗漏；不删除实际还在使用的 `--sf-*`/Markdown 规则；不添加新业务 CSS 文件；文档不宣称不存在的分析入口。

### M4：发布验收

**UI-28｜功能回归。** 执行第 6 节映射矩阵，给出功能测试证据。验收：所有入口有明确目标去向；执行、编辑、停止、重发、清空、删除与资源回收均保持；运行门禁不因换容器失效。破坏性操作在隔离数据上验证。

**UI-29｜视觉与无障碍。** 按 UI-05 关键帧与第 6 节尺寸矩阵截图比较，测正文/辅助信息/焦点的对比度并走查键盘/读屏。验收：A01/A02 不复现；没有截断关键动作、遮挡焦点、不可达菜单或主题错色；不足项有优先级，不用“截图通过”代替完整可访问性检查。

**UI-30｜性能和生命周期。** 用同一机器/构建条件比较 M0 与新版本，覆盖多个项目、1000 条消息、50 节点 DAG、持续 PTY 输出。验收：面板隐藏/恢复不增殖事件监听和进程；可解释大于 50ms 的主线程长任务，重复输入/切栏无持续卡顿；记录峰值内存与交互时延，超基线明显回退必须定位后修复，不能用静态截图验收性能。

**UI-31｜开发者走查。** 邀请 3–5 名目标开发者完成选项目、新建任务、找到失败原因、打开文件、审查差异、恢复终端的固定任务。验收：记录完成情况、误操作和找不到的入口；核心流程无未解决 P0；较大 P1 进入修订并重走相关步骤。无人可参与时写 BLOCKED，不编造用户反馈。

**UI-32｜发布与回退。** 输出：PR 顺序、版本说明、最终截图与此表最终状态。验收：每批 commit 可独立回退，运行中任务升级边界有说明；若新增存储字段须另验前向迁移和快照；没有遗留永久双外壳、未使用开关或失效说明。用户未授权发布时只完成可审阅的发布材料，不自动发布。

## 5. 每项任务的完成记录模板

复制到下方“推进记录”，状态仍在总表更新。

```text
任务：UI-xx
负责人：
开始/完成日期：
基线/结果 commit 或 PR：
实现文件与范围：
对应问题：Axx 或设计章节
测试命令及结果：
截图：同窗口/主题/数据的 before 与 after 路径
人工走查：步骤、实际结果、未验证项
风险/回退：
阻塞或剩余事项：
验收人/日期：
```

## 6. 回归矩阵与交付门槛

### 环境矩阵

| 维度 | 必测组合 | 重点 |
|---|---|---|
| 窗口 | 1000×680、1280×800、1440×900、1600×1000、1920×1080 | 正文/输入/工具可见；最小空间策略 |
| 主题 | 浅色、深色、跟随系统 | 语法、终端、画布、图片占位与菜单同步 |
| 缩放 | 100%、125%、150% 文本/页面缩放（按平台支持记录） | 核心按钮不隐藏，不靠缩小字解决布局 |
| 系统 | macOS 必测；Windows/Linux 在支持环境补测 | 拖拽区、窗口控制、字体和 Cmd/Ctrl |
| 输入 | 鼠标、键盘、中文 IME、触控板横/纵滚动 | 不误发、不抢焦点、局部滚动 |
| 数据 | 空、少量、长标题、长代码、1000 条消息、50 节点图 | 无伪空态、错误可见、动态高度稳定 |

窗口×浅深色为完整 10 组基线；系统主题/缩放/平台选核心工作区做风险组合，不声称本轮已跑过这些组合。

### 功能映射回归

| 流程 | 通过条件 | 关联任务 |
|---|---|---|
| 普通聊天 → 分类 → 会话 → 图片输入 | 分类/草稿/图片引用正确，模型来源不变 | 10,11,20,28 |
| 项目 A → B → A | 返回原会话与布局，运行流不串台，隐藏页不抢快捷键 | 07,08,23,30 |
| 流式响应 → 打开工具/子智能体 → 返回 | 流不断，时序/错误一致，滚动锚点可恢复 | 12,14,24,28 |
| 执行图 → 节点 → 共享状态 → 返回 | 状态不误报成功，视口保留，所有动作可达 | 03,13,14,28 |
| 文件树 → 文件 → 修改 → 改名/关闭 | 文件与标签正确，编辑内容处理遵守既有约定 | 16,28 |
| 变更 → diff → 暂存 → 提交 | 范围明确，失败保留文本，不自动推送 | 17,28 |
| 终端输出 → 隐藏 → 恢复 | PTY/输出连续，尺寸正确 | 19,30 |
| 浏览器打开 → 最小化 → 换会话 → 恢复 | 页面与 session 对应，帧和点击位置正确 | 18,30 |
| Python 执行 → 停止/失败 → 查看输出 | 停止是真实停止，错误与产物来源清楚 | 20,28 |
| 架构画布 → 助手收起 → 主题/布局改变 | 不丢画布、视口与撤销栈 | 15,28 |
| 设置编辑 → 保存中关闭 → 保存失败 | 明确保存结果，重试可用，不吞错误 | 21,22,28 |
| 编辑重发/清空/删除会话 | 资源清理与图片复用语义不变，真实 token 用量不回退 | 20,28 |

### 合并/发布检查清单

- [ ] 所有本批任务在总表有负责人、状态与证据。
- [ ] `pnpm build` 通过。
- [ ] `pnpm lint` 通过。
- [ ] `pnpm styles:report` 通过，或清楚记录基线失败与本批无新增证据。
- [ ] `pnpm test` 通过；新增逻辑有对应有意义的测试。
- [ ] 涉及 Tauri 调用变化时，`pnpm contract:check` 通过。
- [ ] 涉及 Rust 时执行相应 cargo 检查/测试，并重启 Tauri 验证。
- [ ] 同状态前后截图覆盖浅/深色与窄窗；输入/发送/停止无遮挡。
- [ ] 焦点、IME、菜单、长文本/动态消息与运行资源回归通过。
- [ ] 未新增持锁 I/O、主线程阻塞、逐事件全局重渲染或全文件加载。
- [ ] 新代码控制文件规模；超限生产文件先按职责拆分再扩展。
- [ ] 页面没有未接通功能、伪数据、隐藏错误或无来源“成功”。
- [ ] 不再使用的旧布局/样式已清理；必要兼容的移除条件已登记。
- [ ] 核心流程没有未解决 P0；阻塞 P1 已修复或明确不发布对应范围。
- [ ] 回退边界、版本说明、验收人和日期已记录。

本次仅新增 Markdown 与截图，不运行应用编译/测试冒充实现验收。文档本身需检查本地链接、图像格式、任务编号、依赖和状态计数。

## 7. 风险与决策登记

| 风险/决策 | 处理原则 | 跟踪 |
|---|---|---|
| 安装包不是已证明的源码同构建 | 当前构建重新截图与复现再修复 | UI-01 |
| 全局布局状态串到其他会话 | 按工作区作用域组织临时状态；保留详情请求防竞态 | UI-08/09 |
| 收起面板导致进程/画布重建 | 隐藏/关闭/结束分开；明确资源所有者 | UI-15/18/19/30 |
| 样式清理误伤第三方渲染 | Monaco/Shiki/xterm/React Flow/tldraw 独立验收 | UI-06/27/29 |
| UI 状态与运行成功语义混淆 | 从真实记录映射；验收结果独立展示 | UI-12/13/14 |
| 为美观扩大为存储/运行时重写 | 新持久化/执行能力另立需求；本轮优先展示与布局 | UI-04/32 |
| 用量分析旧入口缺失 | 核实后更新文档，本轮不重建大仪表盘 | UI-27 |

## 8. 推进记录

| 日期 | 记录 | 结果 |
|---|---|---|
| 2026-09-06 | 阅读现有入口、布局、主题、消息、执行图、设置与运行器容器；原生 UI 捕获八步截图 | 现状与设计文档完成；建立 32 项 TODO，实施完成数 0 |
| 2026-09-06 | B0：隔离 HOME 数据 + 源码同构建采集基线 5 帧；登记走查阻塞 | `e4d2b82`；UI-01 部分 BLOCKED |
| 2026-09-06 | B1：UI-02 容器宽度、UI-03 执行图层（两独立 commit） | `25aa667`、`3d9e59c` |
| 2026-09-06 | B2：UI-04 空间预算纯函数 + 接线 | `77c57f6` |
| 2026-09-06 | B3：UI-05 关键帧/令牌/预算文档定稿 | `9bccc43` |
| 2026-09-06 | B4：UI-06 令牌收敛三阶段 | `39e93dd`、`6eeff1f`、`24da7ea` |
| 2026-09-06 | B5：UI-07 统一外壳接线 + 收尾删旧件 | `bb3c0e9`、`f41134d` |
| 2026-09-06 | B6：UI-08 工作区状态归属 | `289cb3f` |
| 2026-09-06 | B7：UI-09 主区标签/详情槽/抽屉 | `afb341d` |
| 2026-09-06 | B8：UI-10 列表精修 | `15e50be` |
| 2026-09-06 | C1：UI-11 任务头部与输入（双行头部/更多菜单/停止态/分支 pill） | `4ee7c71` |
| 2026-09-06 | C2：UI-12 消息层级（status-meta/StatusPill+语义摘要+失败 pinned+superseded 共享） | `307dc73` |
| 2026-09-06 | C3：UI-13 执行图迁主区标签（C3a 状态层 + C3b 内联化，成对回退） | `a9f7064`、`4c7381c` |
| 2026-09-06 | C4：UI-14 详情统一（共享 detail 组件+抽屉拆分+子智能体三段） | `05bc50f` |
| 2026-09-06 | C5：UI-18 浏览器迁主区（C5a 拆分+串帧修复 + C5b 单例标签+右面板移除） | `4169175`、`860bf88` |
| 2026-09-06 | C6：UI-20 Python/产物详情统一（来源/耗时/kind 分派/状态同源） | `633ec4a` |
| 2026-09-06 | D1：UI-16 文件编辑工作区（壳归位+编辑器平面化+路径工具行收敛+tab 脏标记） | `88babe4` |
| 2026-09-06 | D2：UI-17 Git 审查（D2a diff 解析抽离+重命名/二进制呈现+后端 origin_path；D2b 提交区暂存范围+就地错误+导航 diff 对应高亮） | `8e09fca`、`9a20640` |
| 2026-09-06 | E1：UI-15 架构空间（助手默认宽像素锚定 360 + 领域绘图空态 + 附截图/快照实时上下文提示；画布保活/阻断面板基线已满足登记） | `f4cab0a` |
| 2026-09-06 | E2：UI-19 终端 dock（mounted/visible 两态机隐藏保活 PTY + 隐藏/结束双语义头部 + 高度常量单一出处；680px 预算与多项目隔离基线已满足登记） | `f4f63a9` |

后续每次合并只更新实际完成任务；发现新增问题使用新编号 UI-33 起，保留历史任务记录。

## 9. UI-01–UI-10 任务记录（第 5 节模板）

任务：UI-01
负责人：claude（会话领取）
开始/完成日期：2026-09-06
基线/结果 commit 或 PR：基线 `04df59a`；证据提交 `e4d2b82`
实现文件与范围：`evidence-baseline/`（01–05 png + manifest.json）
对应问题：A01/A02 复现标注、八步基线补验
测试命令及结果：无（证据任务）；`screencapture` 权限探测通过、坐标点击不可用（-25208）
截图：隔离 HOME（/tmp/jka-iso）+ 源码同构建 debug 二进制，1600×1000，浅/深色 5 帧
人工走查：V01/V02 等价帧已捕获；V03–V07、设置深层态未走查
风险/回退：仅文档，可独立回退
阻塞或剩余事项：**BLOCKED 项**——会话行旧实现不可键盘聚焦（A9）且无指针自动化权限，V03–V07 交互态不可达；解除条件：UI-10/23 键盘语义修复后重跑或授予辅助功能权限。A01/A02 以源码事实链 + 安装包截图佐证复现
验收人/日期：待人工（UI-29/31）

任务：UI-02
负责人：claude（会话领取）
开始/完成日期：2026-09-06
基线/结果 commit 或 PR：`25aa667`
实现文件与范围：`styles/tailwind.css`（.ai-chat-column 容器化、阅读宽 760/960、stage/footer 比例边距）、`layout/app-layout.tsx`（min-w-0、Artifact embedded 覆盖层、谎言注释修正）、`chat/prompt-input.tsx`（工具行 wrap、MAX_HEIGHT 合一）、`chat/chat-shell.tsx`（artifactOverlay 传递）
对应问题：A01、A04
测试命令及结果：build/lint/styles:report/vitest 全绿（136→后续批次递增）
截图：遗留（需 tauri 运行态，见 UI-01 阻塞）
人工走查：未执行（阻塞同上）
风险/回退：单 commit 可回退；embedded 覆盖层为过渡策略，终态 UI-09
阻塞或剩余事项：A01 同尺寸前后对比截图

任务：UI-03
负责人：claude（会话领取）
开始/完成日期：2026-09-06
基线/结果 commit 或 PR：`3d9e59c`
实现文件与范围：`graph/GraphPanel.tsx`（createPortal、z=200、焦点陷阱/还原、栈顶 Escape）、`lib/overlay-stack.ts`（新增）、`hooks/use-chat-shortcuts.ts`（让路）、`chat-page-v2*`（visible 门控链）、`styles/tailwind.css`（.ai-graph-portal）
对应问题：A02
测试命令及结果：门禁全绿
截图：遗留
人工走查：未执行
风险/回退：portal+门控同 commit，避免隐藏项目浮出回归
阻塞或剩余事项：遮挡前后对比截图；Radix DismissableLayer 深度整合留 UI-23

任务：UI-04
负责人：claude（会话领取）
开始/完成日期：2026-09-06
基线/结果 commit 或 PR：`77c57f6`
实现文件与范围：`project/workspace-budget.ts`(+test 32 case)、`hooks/useWorkspaceBudget.ts`、`ProjectPage.tsx`、`project/ProjectWorkbenchContent.tsx`、`project/ProjectWorkspaceLayout.tsx`（像素 grid、分隔条 a11y）、`hooks/useProjectPanels.ts`（applyRightPanelWidth）
对应问题：A03、设计 §3
测试命令及结果：`pnpm vitest run src/components/project/workspace-budget.test.ts` 32 passed；门禁全绿
截图：N/A（纯函数+接线）
人工走查：窗口矩阵手工验证遗留 UI-29
风险/回退：预算为纯函数，接线层单 commit
阻塞或剩余事项：无

任务：UI-05
负责人：claude（会话领取）
开始/完成日期：2026-09-06
基线/结果 commit 或 PR：`9bccc43`
实现文件与范围：`design/keyframes.html`（六场景×浅深×窄窗代表帧，标注区域宽度）、`design/tokens.md`（定稿表+评审结论四条）、`design/space-budget.md`（引用 UI-04 测试锚点）
对应问题：设计 §4/§3 定稿
测试命令及结果：文档自洽检查（链接/数值引用）通过
截图：参考帧非应用截图（页面已显著声明）
人工走查：视觉评审结论记录于 tokens.md §5；人类复核留 UI-29
风险/回退：仅文档
阻塞或剩余事项：无（DONE）

任务：UI-06
负责人：claude（会话领取）
开始/完成日期：2026-09-06
基线/结果 commit 或 PR：`39e93dd`（A）`6eeff1f`（B）`24da7ea`（C）
实现文件与范围：`App.css`（权威令牌定稿+sf 族 alias+删空定义+monaco 平面化）、`styles/tailwind.css`（140 处 sf 引用归零、青描边/常驻投影/网格底纹清除、分隔条平面化）、`ui/button.tsx`（密度三档+active 态）、`IconButton.tsx`（包装 ui/Button）
对应问题：A05、设计 §4
测试命令及结果：每阶段 styles:report 0 无引用；门禁全绿
截图：浅深双主题人工比对遗留 UI-29
人工走查：未执行
风险/回退：三阶段独立 commit 可分别回退
阻塞或剩余事项：第三方渲染器（Monaco/Shiki/xterm/ReactFlow/tldraw）对比度实测

任务：UI-07
负责人：claude（会话领取）
开始/完成日期：2026-09-06
基线/结果 commit 或 PR：`bb3c0e9`（接线）`f41134d`（收尾）
实现文件与范围：`shell/AppRail.tsx`、`shell/ContextNav.tsx`、`shell/StatusDockBar.tsx`（新增）、`App.tsx`（homeView 提升）、`WelcomePage.tsx`（AppRail 替换 60px nav）、`ProjectPage.tsx`（ContextNav 四页签、右面板收敛浏览器）、`SessionPanel.tsx`（hideChrome）、删除 `RightToolbar.tsx`/`ProjectRail.tsx` 及其 CSS
对应问题：A07 入口部分、设计 §2/§3.1
测试命令及结果：门禁全绿
截图：遗留
人工走查：保活/关闭≠删除/隐藏不丢状态 人工验证遗留 UI-28/31
风险/回退：两 commit 先切调用后删旧件
阻塞或剩余事项：浏览器右面板容器迁移留 UI-18（已登记移除条件）

任务：UI-08
负责人：claude（会话领取）
开始/完成日期：2026-09-06
基线/结果 commit 或 PR：`289cb3f`
实现文件与范围：`stores/workspace-store.ts`（新增三层）、`project/workspace-prefs.ts`(+test)、`hooks/useProjectPanels.ts`（换底座+拖拽 mouseup 提交）、`ProjectPage.tsx`（偏好接线）、`chat/session-scope.ts`（新增）、`chat-shell.tsx`/`GraphPlanCard.tsx`/`useGraphPanelController.ts`（执行图归属 sessionId）、`usePythonRunController.ts`（切会话重置）、`utils.ts`（save try/catch）、`stores/ui-store.ts`（移除 graphPanelPlanId）
对应问题：设计 §7.3、串台风险登记
测试命令及结果：workspace-prefs 4 case + 全量 172 passed
截图：N/A
人工走查：窄窗恢复不覆盖宽屏偏好 手工验证遗留 UI-29
风险/回退：persist key 新命名，旧 nezha.* 保留待 UI-27 清理
阻塞或剩余事项：多窗口同步不纳入本轮（已登记）

任务：UI-09
负责人：claude（会话领取）
开始/完成日期：2026-09-06
基线/结果 commit 或 PR：`afb341d`
实现文件与范围：`project/main-tabs.ts`(+test 8 case)、`hooks/useProjectPanels.ts`（标签体系）、`project/ProjectWorkbenchContent.tsx`（activeEditorTab 渲染）、`ui/dialog.tsx`/`ui/sheet.tsx`（新增）、`ChatPageOverlays.tsx`+`PythonRunDrawer.tsx`（Sheet 覆盖层化）、`hooks/useSessionRequestGuard.ts`（新增）、`FileViewer.tsx`/`file-viewer/FileTabPane.tsx`（类型）
对应问题：A04 终态部分、设计 §5.4/§3.3
测试命令及结果：main-tabs 8 case + 全量 178 passed
截图：遗留
人工走查：关闭返回原内容/滚动位置 手工验证遗留 UI-28
风险/回退：标签 reducer 纯函数单测保护
阻塞或剩余事项：窄屏 Artifact 走 Sheet 抽屉（当前为 embedded 覆盖层过渡）

任务：UI-10
负责人：claude（会话领取）
开始/完成日期：2026-09-06
基线/结果 commit 或 PR：`15e50be`
实现文件与范围：`lib/project-sort.ts`(+test)、`App.tsx`（排序修复）、`WelcomePage.tsx`（最近项目列表）、`SessionPanel.tsx`（button 行+更多菜单+相对时间+运行文字）、`styles/tailwind.css`（列表样式+死样式清除）
对应问题：A09、A12 部分
测试命令及结果：project-sort 2 case + 全量 180 passed
截图：遗留
人工走查：零/1/50 项目与大量会话操作遗留 UI-28/31
风险/回退：单 commit
阻塞或剩余事项：无

## 10. UI-11–UI-20（M2 批次一）任务记录（第 5 节模板）

任务：UI-11
负责人：claude（会话领取）
开始/完成日期：2026-09-06
基线/结果 commit 或 PR：基线 `1e40ebf`；结果 `4ee7c71`
实现文件与范围：`ChatPageHeaders.tsx`（双行头部重构：任务标题+运行状态双编码/项目名+分支 pill；清空与设置收入 DropdownMenu 更多菜单；删硬编码「调度智能体」）、`hooks/use-project-session-title.ts`（新增：useProjectSessionsQuery 同 key 去重 + dispatcher-session-updated 事件兜底）、`hooks/use-current-git-branch.ts`（新增：mount/path 变化/获焦刷新，刻意不轮询）、`lib/git-branch.ts`(+test 4 case)、`chat-page-v2.tsx`（projectId/projectName props + isStopping 下传）、`chat-shell.tsx`/`prompt-input.tsx`（stopping prop：停止按钮 disabled+Loader2+「正在停止…」）、`ProjectWorkbenchContent.tsx`（传 projectId/projectName）、`styles/tailwind.css`（.ai-chat-header* 类族 8 个，reduced-motion 降级）
对应问题：A07、设计 §5.2
测试命令及结果：git-branch 4 case；全量 184 passed（本提交时点）；build/lint/styles:report 全绿
截图：遗留（tauri 运行态，见 UI-01 阻塞说明；同 UI-29 矩阵）
人工走查：长中文标题 1000px 窄窗头部不溢出、停止中按钮反馈、更多菜单键盘可达——遗留 UI-28/29
风险/回退：单 commit 可回退；标题查询在导航收起时由 header 独立挂载触发一次 project_list_sessions（缓存共享，可接受）
阻塞或剩余事项：分支 pill 依赖项目为 git 仓库；非 git 项目静默不显示（设计即如此）
验收人/日期：待人工（UI-29/31）

任务：UI-12
负责人：claude（会话领取）
开始/完成日期：2026-09-06
基线/结果 commit 或 PR：`307dc73`
实现文件与范围：`detail/status-meta.ts`(+test，六域状态双编码映射，未知回退 neutral 原文)、`detail/StatusPill.tsx`（图标+文字 pill，.ai-status-pill--{tone} 六类）、`dispatcher-chat/tool-activity-summary.ts`(+test 27 case：工具名→动词类别、计数可复算、文案顺序快照)、`tool-activity.ts`（ToolActivityItem 增 planned 可选标记）、`live-tool-activity.ts`（planned 翻转链路：toolPlanned 置位/started、finished 清除）、`tool-call-card.tsx`（摘要行换语义文案+失败/等待/执行中 StatusPill 计数；收起态非成功卡 pinned 渲染在折叠区外；卡片徽章换 StatusPill；planned→Clock3 琥珀节点）、`chat/superseded-block.tsx`（新增共享组件）、`assistant-message.tsx`/`streaming-message.tsx`（实时侧补 superseded 折叠分支——同轮次一致）、删除零消费者旧 AiStatusPill（sci-fi-shell）及旧 .ai-status-pill CSS 族、safelist 更新
对应问题：设计 §5.2、tokens.md §5 结论 4（只靠彩点收敛）
测试命令及结果：tool-activity-summary 27 + status-meta 6 + tool-activity planned 链路 3；全量 221 passed；门禁全绿
截图：遗留（浅/深主题聚合卡比对 UI-29）
人工走查：聚合收起态失败卡可见、pinned 区高度观感——遗留 UI-28
风险/回退：单 commit；planned 为可选字段，历史投影无该标记（皆终态），既有断言不受影响
阻塞或剩余事项：工具类别映射按工具名（顶层 ToolActivityItem 无 category 字段）；后端如未来透传 category 可切换数据源
验收人/日期：待人工（UI-29/31）

任务：UI-13
负责人：claude（会话领取）
开始/完成日期：2026-09-06
基线/结果 commit 或 PR：`a9f7064`（C3a 状态层）+ `4c7381c`（C3b 内联化）——**需成对回退**
实现文件与范围：C3a：`main-tabs.ts`（kind:"graph" 标签，id=graph:{planId}，openGraphTab 幂等/同会话替换，+7 test）、`useProjectPanels.ts`（handleOpen/CloseGraphTab；hasEditorContent 收敛单一派生值并替换 ProjectPage/PWC 两处重复表达式）、`workspace-store.ts`（graphViewBySession 每会话视图记忆，不持久化）；C3b：`hooks/useGraphTabSync.ts`（新增：意图↔标签双向同步，truncate 自动关标签，关标签按会话+计划匹配清意图）、`GraphPanel.tsx`（删 createPortal/overlay 栈登记/焦点陷阱；Escape 只关抽屉且 active 门控；视图记忆读写——拖拽结束/onMoveEnd 才落 store；stateOpen 默认收起；保活切回无记忆时 re-fitView；key=planId 整树重建替代渲染阶段清空补丁）、`GraphPanelHeader.tsx`（运行/验收两结论前缀标签分区；running 无验收→neutral「验收未开始」；扩大/还原占满主区按钮=会话 pane 收起机制）、`GraphNodeView.tsx`（压缩：标题单行/模型二级/摘要两行 line-clamp/StatusPill）、`graph-layout.ts`（节点常量 88/136→104/116）、`ProjectWorkbenchContent.tsx`（editorPane graph 分支 lazy+ErrorBoundary）、`ChatPageOverlays.tsx`（删 GraphPanel portal 分支——独立聊天无执行图已核实）、`useGraphPanelController.ts`（删 planId 暴露，保留意图开/关与 latestPlanId）、CSS 删 .ai-graph-overlay/.ai-graph-portal/节点旧类
对应问题：A02（终态）、A08、设计 §5.3/§3.3、keyframes S4
测试命令及结果：main-tabs +7；全量 228 passed；门禁全绿；contract:check 通过（无 Rust 改动）
截图：遗留
人工走查：graph 标签打开→切文件→切回视口/选中保留；运行中关标签再开不触发新 run；扩大/还原；保活项目切回 fitView——遗留 UI-28
风险/回退：C3a+C3b 成对回退（单独 revert C3b 会留下无人渲染的 graph kind）；overlay-stack 保留（当前无 push 方，hasOpenOverlay 让路契约供 UI-23）
阻塞或剩余事项：无
验收人/日期：待人工（UI-29/31）

任务：UI-14
负责人：claude（会话领取）
开始/完成日期：2026-09-06
基线/结果 commit 或 PR：`05bc50f`
实现文件与范围：`detail/DetailSection.tsx`/`detail/OutputBlock.tsx`/`detail/ActivityTimeline.tsx`（新增共享组件）、`graph/ExecutionTimelineList.tsx`（新增：虚拟化时间线+ToolCallCard+NoticeRow 自抽屉迁出）、`GraphNodeDrawer.tsx`（479→314 行；头部 StatusPill；footer「重新执行」→ completed=完整重跑 / failed,cancelled=从断点继续，与头部门禁同一表达式）、`SubAgentExecutionView.tsx`（重构为概览/活动/输出三段：概览 meta grid 含耗时/模型「未记录」如实占位/速度/迭代/tokens+来源任务可展开+failed 错误 OutputBlock；活动=PhaseIndicator+进度+ActivityTimeline；输出=OutputBlock 可选中；头部 StatusPill）、`artifact-panel.tsx`（元信息 DetailSection+meta grid、全文/预览 OutputBlock）、CSS：新增 .ai-detail-*/.ai-output-block*/.ai-activity-timeline* 类族，删除 .ai-subagent-exec-{stats,stat-chip,timeline*,result*,error*} 与 .ai-graph-chip--node-* 死类
对应问题：A08、设计 §5.3、风险登记「UI 状态与运行成功语义混淆」
测试命令及结果：全量 228 passed；门禁全绿
截图：遗留
人工走查：实时事件与历史轨迹恢复视觉一致；返回图保留所选节点与视口（依赖 C3b view-memory）——遗留 UI-28
风险/回退：单 commit；**决策**：draft Harness 编辑器与 flush 兜底保留在抽屉内未拆（key 重建会改变草稿 flush 时序语义，风险大于收益；314 行已达标）；子智能体活动用简单时间线不虚拟化（单任务工具调用十级~百级行数，图节点千级继续虚拟化——量级决策非视觉分裂）
阻塞或剩余事项：SubAgentSession/轨迹无 model 字段——按规格显示「未记录」，如需真实模型需后端 schema 变更（另立任务）
验收人/日期：待人工（UI-29/31）

任务：UI-18
负责人：claude（会话领取）
开始/完成日期：2026-09-06
基线/结果 commit 或 PR：`4169175`（C5a 拆分+串帧修复）+ `860bf88`（C5b 迁主区标签）——C5a 可独立保留，C5b 单独回退即回右面板
实现文件与范围：C5a：`components/browser/` 七文件（BrowserPanel 组装壳<100 行、useBrowserPanelSession——**sessionId 变化清空画布/日志/错误/状态并重拉（串帧修复）**、useBrowserPanelCommands——busy 单飞锁+全部命令 projectPath 透传权限闸门不变、Header/AddressBar/Stage/LogPane），删除 570 行超限 `components/BrowserPanel.tsx`，两宿主仅改 import；C5b：`main-tabs.ts`（kind:"browser" 工作区单例标签 id="browser"，+4 test）、`useProjectPanels.ts`（删右面板全套机制：rightPanel/toggle/拖拽宽/useDockedBrowserPanel 接线/expanded 视口比例）、`ProjectPage.tsx`（删 rightPanelNode；StatusDockBar 浏览器 toggle=开/关主区标签；dock restore=切会话+开标签、closeDocked 仍 browser_stop；预算入参 rightPanelOpen 恒 false——纯函数与 32 测试不动）、`ProjectWorkbenchContent.tsx`（browser 分支：扩大按钮与会话 pane 收起联动，语义同执行图）、`ProjectWorkspaceLayout.tsx`（删 ProjectRightPanelHost+旧 toolbar 兼容槽）、删除死模块 `projectPanelsFileState.ts`（OpenDiff 迁 main-tabs 权威定义）、CSS 删 .ai-project-right-panel/-resizer
对应问题：设计 §5.5、UI-07 登记移除条件兑现、space-budget.md 未决项 2 兑现
测试命令及结果：main-tabs +4；全量 232 passed；门禁全绿；contract:check 通过（零 Rust 改动）
截图：遗留
人工走查：切会话画面即清；dock restore 跨会话恢复正确页面；关标签后进程仍活、Square 才停止；窗口缩放坐标映射（实时 rect 逻辑未动）——遗留 UI-28/30
风险/回退：迁移 commit 内渲染点原子切换（右面板删除与标签渲染同 commit，无双渲染中间态）；**三层语义**：关标签=隐藏（帧续 emit 无人消费即丢，恢复时 refreshStatus+新帧重绘）/头部 Square=browser_stop/dock X=closeDocked(browser_stop)
阻塞或剩余事项：**决策**：HomeChatPage 独立聊天浏览器保留 flex 兄弟布局（无主区标签体系，迁移成本>收益）；prefs.rightPanelWidth 与 nezha.project.browserPanelWidth 成孤儿数据——UI-27 清理；预算 rightPanel 参数无消费者——UI-27 评估移除
验收人/日期：待人工（UI-29/31）

任务：UI-20
负责人：claude（会话领取）
开始/完成日期：2026-09-06
基线/结果 commit 或 PR：`633ec4a`
实现文件与范围：`dispatcher-chat/python-run-meta.ts`(+test 8 case：耗时终态/running/非法日期 null、来源标签、开始时间)、`PythonRunDrawer.tsx`（头部 StatusPill(python)+live 耗时——1s interval 仅抽屉打开且 running 时挂载；新增来源行 .ai-python-run-meta；Section→DetailSection、执行步骤→ActivityTimeline、错误原因→OutputBlock(tone=error)；stdout/stderr/代码统一 chat-scroll；禁用态内联 opacity→:disabled）、`artifact/artifact-renderers.tsx`（新增 kind 注册表+文本兜底）、`artifact-panel.tsx`（renderArtifactContent 接入；「产物加载失败：」前缀；元信息补来源工具 toolName；空内容显式占位）、`chat/markdown-renderer.tsx`+`markdown/MarkdownCodeBlock.tsx`（两处英文 RunStatusBadge 删除→StatusPill 同源）、App.css 删 .python-inline-badge 族、tailwind.css 增 .ai-python-run-meta/-duration/-content/:disabled 删 -title/-section*/-error/-timeline*
对应问题：设计 §5.5/§5.4（产物详情）、风险登记「运行资源生命周期」
测试命令及结果：python-run-meta 8 case；全量 240 passed；门禁全绿
截图：遗留
人工走查：运行/停止/失败/已完成四态反馈、大输出滚动、内联徽章与抽屉同色同文案——遗留 UI-28
风险/回退：单 commit；**决策**：退出码不进 record（无 schema 变更；failed+errorReason 为展示层等价物）；purge/cleanup/truncate/chat-image 资源清理语义零改动（只读边界）
阻塞或剩余事项：图片缺失反馈由 MarkdownImage 占位+chat_images_validate 先验链路承担（未改，走查确认）
验收人/日期：待人工（UI-29/31）

## 11. UI-16–UI-17（M2 批次二）任务记录（第 5 节模板）

任务：UI-16
负责人：zcode（会话领取）
开始/完成日期：2026-09-06
基线/结果 commit 或 PR：基线 `633ec4a`；结果 `88babe4`
实现文件与范围：`FileExplorer.tsx`（壳归位：删 Files 标题行/项目名行/border-left/width prop，刷新入 30px 工具行）、`file-viewer/FilePaneHeader.tsx`（新增：相对目录路径中间折叠+复制完整路径+保存 pill+预览切换；文件名/语言 pill 不再与标签条重复）、`file-viewer/ImageFilePane.tsx`（新增，共享头部）、`FileTabPane.tsx`（重写：删 PaneShell/PaneCard 卡片套卡片、onDirtyChange 上报、md-preview 头部与「基于 react-markdown 渲染」实现说明删除；464→299 行）、`FileViewer.tsx`（dirtyTabIds 集合+标签琥珀圆点+title 未保存后缀）、`ProjectPage.tsx`（去 projectName/width 透传）、`utils/filePaths.ts`（+collapseMiddlePath/getPathDirectory）、`utils/filePaths.test.ts`（新增 13 case）、`App.css`（monaco-pane 去 24px 圆角/边框与 radial loading；md-preview 拍平 30px 圆角渐变卡；user-select 列表同步）、`styles/tailwind.css`（explorer 头部类族替换为 toolbar、选中行去渐变发光、tab 活动下划线化+去 backdrop blur、pane/card 类族→pane/body、pill 平面化+warning 色调、图片棋盘格 2 层中性化、大文件去网格底纹）
对应问题：设计 §5.4、UI-06 视觉债在文件域的集中清偿
测试命令及结果：filePaths 13 case；全量 263 passed（本任务时点 253）；build/lint/styles:report 全绿
截图：遗留（tauri 运行态，同 UI-29 矩阵）
人工走查：长路径折叠+复制、图片/大文件/Markdown/未保存圆点、关闭/重命名/删除与标签同步——遗留 UI-28/29
风险/回退：单 commit 可回退；Monaco automaticLayout/自动保存管线（900ms debounce+队列 flush）与 rope 生命周期零改动
阻塞或剩余事项：**决策**——关闭脏 tab 不加未保存拦截（自动保存窗口 ≤900ms+错误态圆点常驻，拦截收益小于打断成本）；.file-viewer-code 旧 Shiki 通道残留归 UI-27
验收人/日期：待人工（UI-29/31）

任务：UI-17
负责人：zcode（会话领取）
开始/完成日期：2026-09-06
基线/结果 commit 或 PR：基线 `88babe4`；结果 `8e09fca`（D2a）+ `9a20640`（D2b）
实现文件与范围：D2a：`lib/git-diff.ts`（新增：parseUnifiedDiff 结构化 oldPath/newPath/isNew/isDeleted/isBinary/renameFrom/renameTo/similarity，扁平 diff hunk 挂载修正）、`lib/git-diff.test.ts`（新增 10 case）、`GitDiffViewer.tsx`（重命名 old → new+相似度 chip、Binary 占位态、meta 行噪音不渲染）、`src-tauri/scm/git/types.rs`（GitFileChange+origin_path）、`queries.rs`（parse_porcelain_fields 纯函数+4 cargo 单测，porcelain `R old -> new` 旧路径不再丢弃）；D2b：`GitChanges.tsx`（提交区「提交暂存的 N 个文件」+0 暂存禁用守卫、提交/生成错误就地于提交框下（重试+保留文本）、暂存/加载错误列表区就近行、全部暂存/取消改显式文件列表不再 add -A 波及未跟踪区、重命名行 old → new、is-active 高亮、去 width prop；537→362 行，行组件拆至 `git/GitChangesParts.tsx`）、`GitHistory.tsx`（activeCommitHash 高亮、错误上移操作行就近+重试、去 width prop；526→348 行，CommitRow/BranchOption/CommitDetailPanel 拆至 `git/GitHistoryParts.tsx`）、`ProjectPage.tsx`（activeFileDiff/activeCommitHash 派生下传）、CSS：diff shell 平面化（bg-panel/去渐变底纹青 tint）、git-diff-rename-*/similarity/binary 新类、提交按钮去渐变发光、行 hover 去 translateX、ai-git-error 死类清除（被 inline-error 类族替代）
对应问题：设计 §5.4（审查模式暂存范围/错误就地/空变更不制造成功指标）
测试命令及结果：git-diff 10 case；全量 263 passed；cargo test 507 passed（+4）；build/lint/styles:report/contract:check(115:112) 全绿
截图：遗留（同 UI-29 矩阵）
人工走查：可丢弃测试仓库覆盖暂存/取消/重命名/二进制/空变更/提交失败/历史 diff——遗留 UI-28（不在真实工作仓库验证提交）
风险/回退：D2a+D2b 各自独立可回退（D2a 单独回退时 Viewer 退回内置解析器）；后端仅 DTO 字段+解析函数，无 schema 迁移
阻塞或剩余事项：**决策**——split（并排）diff 不在本轮（任务卡「UI-01 确认当前 diff 能力后再估算」，当前 unified-only，能力估算登记后续）；历史分页（固定 limit 50）不在本轮；提交后 GitChanges/GitHistory 跨面板缓存同步（页签切换重挂载隐性掩盖）留待统一 git 数据层时处理
验收人/日期：待人工（UI-29/31）

## 12. UI-15–UI-19（M2 批次三）任务记录（第 5 节模板）

任务：UI-15
负责人：zcode（会话领取）
开始/完成日期：2026-09-06
基线/结果 commit 或 PR：基线 `9a20640`；结果 `f4cab0a`
实现文件与范围：`hooks/useDockedBrowserPanel.ts`（DockedPanelMetrics 增 defaultWidthPx 像素锚定 + 纯函数 resolveDockedPanelDefaultWidth 抽出；+test 5 case；浏览器消费方不传新参、行为不变）、`architecture/ArchitectureView.tsx`（默认宽 0.28 视口比→defaultWidthPx 360，落设计 §5.6 的 320–400px；editorVersion 计数驱动 shape 订阅）、`architecture/use-canvas-shape-count.ts`（新增：store.listen{scope:"document"} 订阅当前页图形数，值不变不 setState）、`architecture/chat/attachment-context.ts`（新增纯函数 formatAttachmentHint；+test 8 case）、`architecture/chat/ArchitectureChatPanel.tsx`（canvasShapeCount prop + .ai-arch-attach-hint 提示行 + ARCH_EMPTY_STATE 绘图任务空态）、`chat/empty-chat-state.tsx`（title/copy 可选 props，缺省保持通用文案）、`chat/message-list.tsx`（空态透传 emptyState 与 className——修复空态丢 .ai-arch-chat-messages 类）、`styles/tailwind.css`（.ai-arch-attach-hint）
对应问题：A06/V08、设计 §5.6
测试命令及结果：docked-panel 5 + attachment 8 case；全量 276 passed；build/lint/styles:report(916:922)/contract:check(115:112) 全绿
截图：遗留（tauri 运行态，同 UI-29 矩阵）
人工走查：切主题/收起助手/拖宽后撤销栈与视口保留、空画布提示与附带提示切换——遗留 UI-28/29
风险/回退：单 commit 可回退；画布持久化 key、感知发送管线（截图/快照段顺序）、arch-run 监听零改动
阻塞或剩余事项：**基线已满足（复现证明，未重复实现）**——①切主题不重建 editor：命令式 applyTldrawColorScheme（ArchitectureView.tsx:112-114 / architecture-theme.ts）；②收起助手/拖宽不重建：ArchitectureCanvas memo + 稳定回调，aside 交换不触及画布；③许可/画布崩溃明确失败：CanvasBlockedPanel license/crash/unexpected 三类（canvas-block-info.test.ts 既有 29 case）；④空画布不附截图：collectScreenshotSegment 首行形状数守卫（useArchitectureChat.ts:151），快照同理；无自动模型请求（send 仅显式触发）。**边界登记（不实施）**——主页视图切换（架构↔聊天/项目）整体卸载 ArchitectureView，视口/撤销栈丢失（形状经 IndexedDB 保留）；超出本卡验收清单（仅列切主题/收起/拖宽），常驻 tldraw 实例保活涉及内存与 WelcomePage 结构改造，挂 UI-30 或后续编号评估；tldraw 画布工具部分英文（locale 已 zh-cn，属上游翻译覆盖）遗留
验收人/日期：待人工（UI-29/31）

任务：UI-19
负责人：zcode（会话领取）
开始/完成日期：2026-09-06
基线/结果 commit 或 PR：基线 `f4cab0a`；结果 `f4f63a9`
实现文件与范围：`project/terminal-dock.ts`（新增纯函数两态机 TerminalDockState{mounted,visible} + nextTerminalDockState；+test 6 case）、`ProjectPage.tsx`（showShellTerminal useState→terminalDock 状态机；once-mounted 渲染隐藏不卸载；budget.terminalOpen 接 visible；StatusDockBar toggle 派发动作）、`ShellTerminalPanel.tsx`（props onClose→onHide+onTerminate、新增 visible：根节点 display:none 保活，挂载主 effect 仍只随 [shellId, projectPath] 重建——xterm/scrollback/PTY/事件监听全保活；fit 零尺寸守卫；重激活 effect 门控 isActive&&visible 并加容器零尺寸检查；头部双语义按钮：ChevronDown 隐藏=中性色 / X 结束=危险色，aria-label 齐备）、`project/workspace-budget.ts`（导出 TERMINAL_HEIGHT_LIMITS{100,600}，DEFAULT_CHROME 引用）、`project/workspace-prefs.ts`（sanitize 引用同一常量）、`hooks/useProjectPanels.ts`（拖拽钳制引用同一常量，三处字面量收敛）、`project/ProjectWorkspaceLayout.tsx`（dock 槽注释语义更新）、`styles/tailwind.css`（.ai-shell-terminal-actions 组 + .ai-shell-terminal-hide 中性 hover 变体）
对应问题：设计 §3.3（隐藏≠结束/关闭按钮文案区分）、§5.5（布局更新不重建 PTY）、§3.1（dock 高度预算）
测试命令及结果：terminal-dock 6 case；全量 282 passed；build/lint/styles:report(918:924)/contract:check(115:112) 全绿
截图：遗留（tauri 运行态，同 UI-29 矩阵）
人工走查：隐藏→恢复 PTY/scrollback 保留、结束会话→再开全新 shell、1000×680 隐藏恢复、多项目终端隔离、resize（50ms debounce + 恢复显示 RO 重触发）——遗留 UI-28/30
风险/回退：单 commit 可回退；后端 open_shell kill-first/resize_pty/kill_shell 与 64KB 读缓冲、16ms 批量 emit 零改动；rAF drain/SmartWriter/InputBatcher 缓冲管线零改动
阻塞或剩余事项：**基线已满足（登记）**——多项目不串流：shellId=`shell:${projectId}` 事件过滤 + 每项目独立面板（保活后隐藏面板仍持监听、各自过滤，功能无碍）；1000×680 预算钳制：workspace-budget.test.ts 矮窗用例既有断言（终端≤322、主区≥320）；恢复路径=常驻 24px StatusDockBar。**决策**——terminalOpen 开关态不持久化（避免重启意外自动拉起 shell；高度偏好已按工作区持久化不变）；N 个保活终端=N 个全局监听者为已知小项（各自按 shell_id 过滤），性能口径归 UI-24/30
验收人/日期：待人工（UI-29/31）
