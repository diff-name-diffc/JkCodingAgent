# 改造任务明细与跟踪清单

更新日期：2026-09-06（M3 批次三 UI-24b 切片更新 2026-09-07）。关联：[审查](01-audit.md) · [设计规格](02-design.md)。

**当前：UI-01–10（M0+M1）、UI-11–20（M2 全部）实施完成一轮；M3 批次一（UI-21「设置布局与自动保存反馈」+ UI-22「MCP/SSH/RAG 状态及作用域提示」，设置与连接状态链）、M3 批次二（UI-25「各场景空态/加载/错误规格落地」+ UI-26「产品术语与图标视觉统一」，空态与术语链）、M3 批次三（UI-23「键盘焦点与输入法整合」全量 + UI-24a「性能低风险切片」+ UI-24b「窗口化动态测量主改造与同链路热点」，键盘与性能链）实施完成**——UI-01 部分 BLOCKED（交互走查不可达，见记录），UI-05 DONE，其余 REVIEW（自动化门禁全绿，人工运行态/截图验收遗留至 UI-23/28/29/31）；**UI-24 保持 DOING**（24b 代码改造完成，验收「附实际 profile」需运行态 DevTools 采样，不虚报）。M0+M1 源码基线 `04df59a`，实施提交 `e4d2b82..15e50be`；M2 批次一 `4ee7c71..633ec4a`、批次二 `88babe4..9a20640`、批次三 `f4cab0a`/`f4f63a9`；M3 批次一源码基线 `0c406b1`，实施提交 `23f6613`/`f968332`/`ba30612`（UI-21a/b/c）与 `fe1b414`/`e24462d`/`444669a`（UI-22a/b/c）；M3 批次二源码基线 `c73a75f`，实施提交 `6d27277`（UI-25）/`13b93fb`（UI-26）；M3 批次三源码基线 `3118a41`，实施提交 `a835fcb`/`7e665b1`/`3d9ec51`/`85db7b3`（UI-23a/b/c/d）与 `21e70da`/`b3b2b8f`/`a9099fd`/`0a05499`（UI-24a-1..4）；UI-24b 切片源码基线 `d2d1f9c`，实施提交 `4b9a637`（24b-1/2 行级 UI 状态存储+Shiki 高亮 LRU 缓存）/`76bec9f`（24b-3 窗口化迁移 react-virtual 动态测量）/`2cfd798`（24b-4 merge 归一化身份缓存）/`ab5c2c5`（键分隔符修正），详见第 15 节。下一步 M3 余量（UI-27 旧规则清理、UI-24 的 profile 验收与 ④⑥⑦ 遗留项）与 M4。

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
| UI-21 | P1 / M | 设置布局与自动保存反馈 | 06,07 | REVIEW | claude（会话领取） | `23f6613/f968332/ba30612`；保存状态持续可见(头部指示器+去 toast 闪烁)+去重标题+导航三组小标题+分类 tabs 横滚+Radix Dialog 外壳/焦点；运行态焦点走查遗留 UI-23/28/29 |
| UI-22 | P1 / M | MCP/SSH/RAG 状态及作用域提示 | 11,21 | REVIEW | claude（会话领取） | `fe1b414/e24462d/444669a`；McpServersPage 拆分(648→305)+连接状态双编码(connection/mcp-server 域)+SSH 徽标去彩点+RAG 未运行不被吞；运行态走查遗留 UI-28/29 |
| UI-23 | P1 / L | 键盘、焦点与输入法整合 | 07,09,11,21 | REVIEW | claude（会话领取） | `a835fcb/7e665b1/3d9ec51/85db7b3`；IME/终端/Monaco 防抢键+enabled 门控、Escape 栈裁决+焦点陷阱/还原、5 分隔条键盘化+步进纯函数、Mod+1..4/J/Shift+A 键位+ContextNav ARIA+列表 ↑↓ 导航；运行态走查遗留 UI-28/29 |
| UI-24 | P1 / L | 长列表、流式输出和布局性能 | 08,12,19 | DOING | claude（会话领取） | 24a 切片 `21e70da/b3b2b8f/a9099fd/0a05499`：memo 击穿修复+role=log 多实例修复+scroll rAF 合帧/ResizeObserver+编辑器占比拖拽隔离；24b 切片 `4b9a637/76bec9f/2cfd798/ab5c2c5`：窗口化迁移 react-virtual 动态测量+行卸载展开态/高亮缓存恢复+merge 归一化身份缓存；**验收「附实际 profile」仍未满足**（需运行态 DevTools 采样，仓库无性能基建），④⑥⑦⑧ 遗留登记 |
| UI-25 | P1 / S | 各场景空态/加载/错误规格落地 | 10,12,15,20 | REVIEW | claude（会话领取） | `6d27277`；普通/项目/架构三入口领域空态(纯函数+5 test)+模型未配置深链取代装饰性禁用按钮+加载占位双编码；运行态走查遗留 UI-28/29 |
| UI-26 | P2 / S | 产品术语与图标视觉统一 | 06,21,22 | REVIEW | claude（会话领取） | `13b93fb`；CloakBrowser→浏览器/Aha→JKCodingAgent 品牌归位+中英混排清理+5 缺名图标按钮补 aria-label；图标全应用统一与后端 Rust 串遗留 UI-29/后续 |
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
| 2026-09-07 | F1：UI-21a 设置保存状态持续可见（头部 SaveStatusIndicator + 纯函数 deriveSaveStatus+4 test + 去每次 toast.success 闪烁 + ProvidersPage/PurposesPage 去重标题 + GeneralPage theme 内联报错） | `23f6613` |
| 2026-09-07 | F2：UI-21b 导航三组小标题（通用/模型/能力连接，窄宽横排）+ 模型分类 tabs 由换行改单行横向滚动 + scrollIntoView 选中可见 + subtab active 视觉统一 | `f968332` |
| 2026-09-07 | F3：UI-21c 设置外壳迁移 Radix Dialog 原语（portal/焦点陷阱/Esc/焦点还原 + ConfirmDialog/Toaster 作 Content 子节点使焦点栈识别嵌套 + sr-only Title + .ai-settings-shell 自居中 z-301） | `ba30612` |
| 2026-09-07 | F4：UI-22a McpServersPage 拆分达标（648→305；纯逻辑抽 mcp-config.ts+20 test，服务器卡抽 McpServerCard.tsx；全局/项目作用域文案基线已满足登记） | `fe1b414` |
| 2026-09-07 | F5：UI-22b MCP 连接状态双编码（status-meta 新增 connection/mcp-server 域+8 test；getMcpIndicatorState→getMcpConnectionStatus 保留 degraded/invalid_config 区分；头部去内联色点改 McpStatusButton+StatusPill；弹窗服务器去彩点改 StatusPill+中性 summary） | `e24462d` |
| 2026-09-07 | F6：UI-22c SSH StatusDot 纯色点改 TestStatusBadge（复用 StatusBadge 点+常驻文字）；RAG runtimeProbing 使探活窗口耗尽显示「未运行」不被吞 + deriveRagRuntimeState 纯函数+4 test；rag_status 无失败原因字段登记后续 | `444669a` |
| 2026-09-07 | G1：UI-25 各场景空态/加载/错误规格（chat-empty-content 纯函数+5 test：普通/项目领域空态分派、架构沿用 UI-15；ModelSelector 无可用模型改「配置模型」深链取代装饰性禁用按钮——主聊天经 ChatShell.onOpenSettings、架构助手就地懒挂 AppSettingsDialog(providers)；ProjectLazyPaneFallback 改 spinner+文案双编码 role=status） | `6d27277` |
| 2026-09-07 | G2：UI-26 产品术语与图标统一（CloakBrowser→浏览器〔面板标题+导入确认正文〕、Aha→JKCodingAgent/AI 助手〔头像+空态 logo alt〕；Run/Running…/Copy/Copied/Input/Output/Tokens/Host/Port/Username/stdout/stderr/Image preview unavailable 中英混排清理；5 缺名 icon-only 按钮补 aria-label；图标按钮容器 32×32/24×24 基线已满足，lucide size/stroke 全应用三方言统一登记 UI-29、后端 Rust CloakBrowser 错误串登记后续） | `13b93fb` |

| 2026-09-07 | H1：UI-23a 快捷键基础设施（keyboard-bindings 纯函数层 26+2 test：IME 组合期全跳过、非 Mac 终端内 Mod 键放行 shell、Monaco allowInEditor 裁决、输入目标豁免；enabled 门控——隐藏工作区不注册/不渲染命令面板；bindings 改 ref 存最新值消除每渲染重挂监听；focusPrompt/handleEditMessage 输入框查询作用域化到本实例子树） | `a835fcb` |
| 2026-09-07 | H2：UI-23b Escape 栈裁决接线（overlay-stack 从零调用到 5 处接线：命令面板/KaTeX 菜单/新分类对话框/大文件查看器〔选区存在时拥有 Escape〕/GraphPanel 收敛到 useOverlayEscape；+shouldHandleEscape 纯函数 11 test 含「一键双关」回归；use-focus-trap 焦点陷阱/还原；McpStatusDialog 补 dialog 语义+Esc 可关；chat-shortcuts Escape 补 Radix data-state 跨栈让路） | `7e665b1` |
| 2026-09-07 | H3：UI-23c 5 条分隔条键盘化（splitter-step 纯函数 14 test：ratio 0.02/0.1、px 8/48、clamp/invert/复位；use-splitter-keyboard 返回 role=separator 完整 ARIA；接线 ContextNav/侧栏/终端高度/首页浏览器面板 4 条缺失站点 + ProjectWorkbenchContent 内联逻辑等价收敛；focus-visible 令牌样式） | `3d9ec51` |
| 2026-09-07 | H4：UI-23d 全局键位+ARIA+列表导航（Mod+1..4 切导航页签/Mod+J 切终端 dock/Mod+Shift+A 开关 Artifact 详情〔含焦点还原〕，visible 门控；roving-index 纯函数 14 test + 注册表无歧义门禁 2 test；ContextNav tablist id/aria-controls/labelledby+roving tabindex 方向键；SessionPanel/侧栏/命令面板 ↑↓ 导航（list-focus DOM 薄层，Radix 菜单 defaultPrevented 让路）；use-chat-shortcuts 重构委托 use-global-shortcuts） | `85db7b3` |
| 2026-09-07 | H5：UI-24a-1 memo 击穿修复（request-guard 工厂 6 test：begin/isStale/invalidate，代号 0 哨兵恒过期；useSessionRequestGuard 变薄为 useMemo 恒定实例；useDispatcherActions 返回对象 memo 化——MessageItem React.memo 流式期间恢复生效） | `21e70da` |
| 2026-09-07 | H6：UI-24a-2 role=log 多实例滚动修复（document.querySelector 全局选择器 → shellContainerRef 子树作用域；多项目保活/架构面板复用下不再滚动隐藏项目的列表；全仓核对仅此一处全局查询） | `b3b2b8f` |
| 2026-09-07 | H7：UI-24a-3 message-list scroll rAF 合帧 + viewportHeight 观测（单一 scrollMetrics state 每帧至多一次读取、同值短路；ResizeObserver 补容器 resize 通道、0 高度忽略；窗口计算抽 message-list-metrics 纯函数 14 test 含 300/301 阈值边界；rowEstimate=180/overscan=8 本体不动属 24b） | `a9099fd` |
| 2026-09-07 | H8：UI-24a-4 编辑器占比拖拽隔离（拖拽期本地 dragRatio + splitDualPaneWidths 纯函数 7 test 与预算同一钳制口径，mouseup 一次性写回偏好；旧实现每 mousemove 同步写 localStorage + ProjectPage 全树重渲染；卸载兜底 cleanup 提交+摘监听） | `0a05499` |
| 2026-09-07 | I1：UI-24b-1/2 行级 UI 状态存储 + Shiki 高亮 LRU 缓存（row-ui-state：MessageList 实例级 ref-backed store + usePersistedToggle，工具卡组/卡/展开全部/思考块四处展开态行卸载后可恢复，5 test；shiki-cache：lang+theme+code 键 LRU 200 条/超长 64KB 不入缓存，highlightCodeToHtml 命中直返，JsonCode 初值同步探测不再闪纯文本，7 test） | `4b9a637`、`ab5c2c5` |
| 2026-09-07 | I2：UI-24b-3 消息列表窗口化迁移动态测量（>300 条阈值语义不变，开窗后固定 180px 估高+spacer → @tanstack/react-virtual measureElement 逐行实测，修复审计 A11 跳读/空白根因；行距 pb-6 烘焙进行高；pinned 跟随仍走 DOM 级 useAutoScroll——外层 column 高度=虚拟化总高+流式气泡，ResizeObserver 双通道覆盖；items 身份变化 measure() 清索引键控测量缓存；24a-3 手动 scrollMetrics/rAF 管线整体移除） | `76bec9f` |
| 2026-09-07 | I3：UI-24b-4 mergeDispatcherMessages 归一化身份缓存（WeakMap 两级键控：wire→产物、产物→自身；重入 merge 时 current 命中零开销，仅新 wire 真正 normalize+JSON.parse；排序/覆盖/短路语义不变，4 test） | `2cfd798` |

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

## 13. UI-21–UI-22（M3 批次一）任务记录（第 5 节模板）

任务：UI-21
负责人：claude（会话领取）
开始/完成日期：2026-09-07
基线/结果 commit 或 PR：基线 `0c406b1`；结果 `23f6613`（21a）+ `f968332`（21b）+ `ba30612`（21c）
实现文件与范围：
21a：`settings/save-status.ts`（新增纯函数 deriveSaveStatus：loading>error>saving>saved 优先级；+test 4 case）、`settings/SaveStatusIndicator.tsx`（新增头部持续指示器：保存中…/已保存/保存失败+重试，派生自 store.loading/dirty/saveError，零 store 契约改动）、`AppSettingsDialog.tsx`（内容头部加 .ai-settings-header-actions 容纳指示器+关闭）、`use-aha-settings.ts`（移除每次 toast.success("已保存")，保留 toast.error；debounce 400ms/revision 守卫/三命令 Promise.all 全不动）、`providers/ProvidersPage.tsx`+`PurposesPage.tsx`（删重复 h2.ai-set-page-title 保留描述，外壳标题成唯一标题源）、`GeneralPage.tsx`（theme fieldId 内联报错，此前无消费者）、`styles/tailwind.css`（.ai-set-save-status* 类族 + .ai-settings-header-actions；删孤立 .ai-set-page-head/-title）
21b：`AppSettingsDialog.tsx`（NAV_ITEMS→NAV_GROUPS 三组通用/模型/能力连接，扁平 NAV_ITEMS 由 flatMap 派生）、`providers/ProvidersPage.tsx`（tabsListRef + activeCategory 变化 scrollIntoView 选中可见 + trigger 加 .ai-set-subtab active 统一）、`styles/tailwind.css`（.ai-settings-nav-group/-group-label + 窄宽横排隐藏组标题；.ai-set-tabs-list flex-wrap:wrap→nowrap+overflow-x:auto+触发器 flex:0 0 auto）
21c：`AppSettingsDialog.tsx`（createPortal+裸 div 外壳→@radix-ui/react-dialog 原语 Root/Portal/Overlay/Content；onOpenChange→requestClose 脏检查；删 window keydown 监听+handleOverlayClick；ConfirmDialog/Toaster 作 Content React 子节点使 Radix 焦点栈识别嵌套、内层确认框打开时外层陷阱让位；confirmingClose 时 onInteractOutside/onEscapeKeyDown 阻断外层联动；sr-only DialogPrimitive.Title「应用设置」+aria-describedby=undefined）、`styles/tailwind.css`（.ai-settings-shell 补 position:fixed 自居中 + z-301）
对应问题：A10（设置外壳焦点隔离）、A12（设置重复标题）、设计 §5.6（内容页一组标题/描述、自动保存状态固定内容头部、分类 tabs 窄宽横滚选中可见、外壳适当 Dialog 原语、错误靠近字段可重试、密钥默认掩码）
测试命令及结果：save-status 4 case；全量 286 passed（21c 时点）；build/lint/styles:report(923:929,0 无引用)/contract:check(115:112) 全绿
截图：遗留（tauri 运行态，同 UI-29 矩阵）
人工走查：编辑→头部保存中…→已保存；制造保存失败→头部保存失败+重试可用+不显示已保存+字段内联报错；快速编辑/切页/脏关闭弹「保存并关闭」不丢值；API key 默认掩码；模型服务/模型用途不再双标题；分类 tabs 窄宽横滚选中可见；Radix 外壳焦点陷阱/还原、Esc 先关嵌套 ConfirmDialog/Select、模型下拉可用——遗留 UI-23/28/29
风险/回退：三 commit 独立可回退（21a/21b 先落地，即使 21c 回退保存反馈/去重/tabs 价值仍在）；21c 最高风险（嵌套弹层/焦点），含回退方案（保留 portal 手动补 role/aria-modal/焦点陷阱）；400ms 自动保存契约、模型库引用与容量来源、密钥掩码零改动
阻塞或剩余事项：**决策**——「已保存」为持续态（非定时淡出），符合设计「持续可见」；初载未编辑即显示「已保存」（与磁盘同步语义准确）。**边界登记**——SSH 页自有 debounce 保存管线与 MCP/RAG 手动保存反馈未统一到头部指示器（本卡 modify 列表限 use-aha-settings 全局管线；SSH/MCP/RAG 的错误就地反馈已在 UI-22 分别处理，统一头部指示器如需另评估）；Radix 焦点陷阱与内联模型下拉/Select 运行态兼容按嵌套子节点方案实现，实测走查遗留 UI-23/28
验收人/日期：待人工（UI-23/29/31）

任务：UI-22
负责人：claude（会话领取）
开始/完成日期：2026-09-07
基线/结果 commit 或 PR：基线 `ba30612`；结果 `fe1b414`（22a）+ `e24462d`（22b）+ `444669a`（22c）
实现文件与范围：
22a：`settings/mcp/mcp-config.ts`（新增纯逻辑层 toEntries/toConfig/parseLines/parseKeyValueLines/nextServerName/parseConfigText/serializeConfig + 类型常量；+test 20 case）、`settings/mcp/McpServerCard.tsx`（新增服务器条目卡 249 行）、`settings/mcp/McpServersPage.tsx`（648→305，仅留页面状态与保存管线）
22b：`detail/status-meta.ts`（新增 connection 域〔checking/not_configured/healthy/degraded/invalid_config，保留 degraded 与 invalid_config 区分不压平〕+ mcp-server 域〔disabled/healthy/invalid_config/spawn_failed/connection_failed，对齐旧 stateColor 语义 spawn_failed=warn〕；+test 8 case）、`hooks/use-mcp-status.ts`（getMcpIndicatorState 返回内联 color/label → getMcpConnectionStatus 返回 connection 域状态键）、`chat-page-v2/ChatPageHeaders.tsx`（删两处内联 style 硬编码色点+Tailwind 工具类；抽 McpStatusButton 原生 button+.ai-chat-header-mcp 避开 ui/Button [&_svg]:size-4 放大 pill 图标；MCP 标签+StatusPill）、`McpStatusDialog.tsx`（服务器纯色点+彩色 state-badge → StatusPill(mcp-server)+中性 summary；删本地 serverStateLabel/stateColor；.ai-mcp-server-actions 补 display:flex）、`styles/tailwind.css`（.ai-chat-header-mcp* + .ai-mcp-server-summary；删孤立 .ai-mcp-server-dot/.ai-mcp-state-badge 保留共用 .ai-mcp-tool-pill）
22c：`settings/ssh/SshServerCardParts.tsx`（StatusDot 纯色点+仅 hover tooltip → TestStatusBadge 复用 settings/StatusBadge 点+常驻文字「可用/失败/未测试」，tooltip 留最后测试时间）、`settings/ssh/SshServerCard.tsx`（import+用法+doc）、`app-settings/rag/useRagKbConfig.ts`（新增 runtimeProbing：pollStatus(5) 探活窗口耗尽仍未运行置 false）、`app-settings/rag/rag-config.ts`（deriveRagRuntimeState 纯函数 running/starting/stopped 语义态+文案；+test 4 case）、`app-settings/rag/RagRuntimeAndImportSections.tsx`（RAG_DOT_CLASS 字面量映射 is-running/is-starting/is-stopped；未运行 title 指向服务日志）、`styles/tailwind.css`（.ai-rag-status-dot.is-starting 琥珀/.is-stopped 红；删孤立 .ai-set-status-dot 类族）
对应问题：A12（状态/中英混用部分）、设计 §5.6（全局/项目 MCP 作用域明确可见）、tokens.md §5 结论 4（状态色双编码，收敛只靠彩点：SSH 状态点/MCP 服务器点）、审计 V03（健康 MCP 降噪、异常明确入口）、V06（连接状态不混淆、异常含真实原因）
测试命令及结果：mcp-config 20 + status-meta +8 + rag-config +4 case；全量 312 passed；build/lint/styles:report(923:929,0 无引用)/contract:check(115:112) 全绿；零 Rust 改动
截图：遗留（tauri 运行态，同 UI-29 矩阵）
人工走查：头部 MCP pill 双编码、健康态次级位置/异常可点开弹窗看真实失败服务器与原因；MCP 弹窗服务器状态图标+文字、degraded 与 invalid_config 区分、server.error 就地；SSH 状态徽标常驻文字、连接/主机密钥错误就地；RAG 启动失败不再永停「启动中…」改显「未运行」、服务日志可排查；全局/项目 MCP 作用域文案清晰——遗留 UI-28/29
风险/回退：三 commit 独立可回退；22a 纯重构（行为零变化）可独立保留；22b/22c 仅状态展示层，审查门禁与配置作用域零改动
阻塞或剩余事项：**基线已满足（登记）**——全局/项目 MCP 作用域分清：McpServersPage Section 描述已明确全局注册表语义+项目 mcp.json 同名覆盖，McpStatusDialog 已按 isGlobal 分视图。**边界登记**——rag_status 后端仅返回 {running,port} 无失败原因字段：本批用前端探活窗口（runtimeProbing）区分「未运行」不吞错，真实原因由服务日志面板承担；如需 runtime 内联失败原因，另立小后端任务（RagRuntimeStatus DTO 加 error/state，非 schema 迁移）。**决策**——connection.healthy 保留 success（绿）tone：StatusPill 浅底小图标已较旧饱和色点降噪，且 MCP 入口处于头部次级动作位（执行图/更多之间），符合「健康降噪+异常明确入口」，未额外降为中性以免「正常」读作「未知」
验收人/日期：待人工（UI-28/29/31）

## 14. UI-25–UI-26（M3 批次二）任务记录（第 5 节模板）

任务：UI-25
负责人：claude（会话领取）
开始/完成日期：2026-09-07
基线/结果 commit 或 PR：基线 `c73a75f`；结果 `6d27277`
实现文件与范围：`chat/chat-empty-content.ts`（新增纯函数层：ChatEmptyStateContent 类型权威 + PLAIN/PROJECT_CHAT_EMPTY_STATE 常量 + resolveChatEmptyState；+test 5 case）、`chat/message-list.tsx`（ChatEmptyStateContent 由内联 interface 改再导出，权威迁至 chat-empty-content）、`chat/empty-chat-state.tsx`（缺省 title/copy/prompts 单一来源为普通聊天领域空态，删内联通用 DEFAULT_*）、`chat-page-v2.tsx`（resolveChatEmptyState(isPlainChat?plain:project) 派生 + 下传 ChatShell）、`chat/chat-shell.tsx`（新增 emptyState prop 透传 MessageList；onConfigureModel=onOpenSettings 下传 PromptInput）、`chat/model-selector.tsx`（无可用模型：装饰性禁用按钮 → 可点「配置模型」深链，onConfigureModel 改必传；删下拉内「暂无可用模型」死分支）、`chat/prompt-input.tsx`（新增 onConfigureModel 必传 prop 透传 ModelSelector）、`architecture/chat/ArchitectureChatPanel.tsx`（视觉模型未配置就地懒挂 AppSettingsDialog(initialTab=providers) 深链）、`project/ProjectLazyPaneFallback.tsx`（加载占位改 Loader2 spinner + 文案双编码，role=status/aria-live=polite）、`styles/tailwind.css`（.ai-model-selector--empty 虚线琥珀双编码 + hover/icon 变体，用 --warning 令牌 color-mix，亮暗双主题随令牌）
对应问题：A06（相同通用空态用于不同工作语境）、设计 §5.2/§5.6（起步提示按普通/项目/架构分语境、模型缺失深链到设置）、tokens.md §5 结论 4（加载/状态双编码，不只靠文字或彩点）
测试命令及结果：chat-empty-content 5 case（plain/project 分派 + 两入口文案互不相同 + 各 4 条非空不重复提示 + 项目文案含「项目/Git/执行图」而普通文案不含「执行图」）；全量 317 passed（312→317）；build/lint/styles:report(924:930,0 无引用)/contract:check(115:112) 全绿；零 Rust 改动
截图：遗留（tauri 运行态，同 UI-29 矩阵）
人工走查：普通/项目/架构三入口空态文案各不相同且贴合语境、点击起步提示填入输入框；模型库清空后主聊天与架构助手显示「配置模型」并可点开设置「模型服务」页；懒加载面板 spinner 占位与空态/错误三态可区分——遗留 UI-28/29
风险/回退：单 commit 可回退；resolveChatEmptyState 返回模块级常量（引用稳定，不引入额外重渲染）；ModelSelector.onConfigureModel 改必传，两处调用方（PromptInput 主聊天 / ArchitectureChatPanel 架构）均已接线，tsc 强制保证无遗漏；架构助手新增懒挂 AppSettingsDialog 仅在「无视觉模型 + 用户点击配置模型」时挂载
阻塞或剩余事项：**基线已满足（登记，未重复实现）**——① empty/loading/error 区分：Sidebar 已有骨架屏(loading) / 「搜索失败，请重试」(error) / 「没有匹配的会话」vs「暂无会话」(搜索为空 vs 无数据)；项目主区 ErrorBoundary 带「重试」、架构 CanvasBlockedPanel 带「重试」、WelcomePage 项目视图区分「没有匹配的项目」vs「还没有项目」。②普通/项目/架构三入口 empty/loading/error/ready 四态齐备（普通:领域空态+侧栏骨架+runError/ErrorBoundary+消息；项目:领域空态+ProjectLazyPaneFallback/「正在创建会话...」+ErrorBoundary 重试+消息；架构:ARCH_EMPTY_STATE+WelcomePaneFallback+CanvasBlockedPanel/sendError+消息）。**决策**——会话搜索错误未加显式「重试」按钮（React Query 自动重试 + 搜索去抖随输入重跑；显式重试需从 useChatSessionController 透传 refetch，收益小于改动面，登记可选后续）；模型未配置深链统一走「打开设置 providers 页」（HomeChatPage/ProjectOverlays 既有 onOpenSettings 即以 providers 为初始 tab），未把 onOpenSettings 参数化为 onOpenSettings(tab) 以免牵动多处调用方。**边界登记**——后端无「模型未配置」的显式错误码/用途槽位字段供前端精确深链到具体分类（对话/视觉）；当前深链到 providers 页由用户自行定位分类，如需直达具体槽位另立小任务（非 schema 迁移）
验收人/日期：待人工（UI-28/29/31）

任务：UI-26
负责人：claude（会话领取）
开始/完成日期：2026-09-07
基线/结果 commit 或 PR：基线 `6d27277`；结果 `13b93fb`
实现文件与范围：
品牌/内部名归位：`browser/BrowserPanelHeader.tsx`（面板标题 CloakBrowser→浏览器）、`browser/useBrowserPanelCommands.ts`（导入 Chrome 登录态确认正文 CloakBrowser→内置浏览器）、`chat/chat-avatar.tsx`（助手头像 alt "Aha AI"→"AI 助手"，对照用户头像 alt="你"）、`chat/empty-chat-state.tsx`（空态 logo alt "Aha"→"JKCodingAgent"，与 AppRail aria-label/alt、index.html title 的品牌称谓对齐）
中英混排清理（用户可见文案）：`chat/markdown-renderer.tsx` + `markdown/MarkdownCodeBlock.tsx`（Run→运行、Running…→运行中…、Copy/Copied→复制/已复制）、`chat/tool-call-card.tsx`（DataSection label Input/Output→输入/输出、Output · Agent Input→输出 · 回传模型；同步收窄 label 联合类型为中文字面量）、`SubAgentExecutionView.tsx`（meta 标签 Tokens→Token 用量，与相邻 耗时/模型/速度/迭代 中文一致）、`file-viewer/ImagePreviewPane.tsx`（Image preview unavailable→图片预览不可用）、`settings/ssh/SshServerCard.tsx`（字段 Host/Port/Username→主机/端口/用户名）、`dispatcher-chat/PythonRunDrawer.tsx` + `app-settings/aha/SshAuditRecordList.tsx`（区块标题 stdout/stderr→标准输出/标准错误，与既有 emptyText「无标准输出」用词一致）
缺名 icon-only 按钮补 aria-label：`ChatNewCategoryDialog.tsx`（关闭）、`markdown/MarkdownImage.tsx`（关闭放大图片，并补 type="button"）、`task-panel/BranchBar.tsx`（新建分支对话框关闭 + 分支搜索清除 ×2）
对应问题：A12（Files/CloakBrowser/中英混排/品牌称谓混用）、设计 §4.2 与 tokens.md §3（图标 16 常规/18 导航、图标按钮 32×32 紧凑 ≥24×24）
测试命令及结果：全量 317 passed（UI-26 为展示层文案/aria-label/alt 改动，无新增纯函数，故无新增测试——符合仓库「优先纯函数单测」口径）；build/lint/styles:report(924:930,0 无引用，计数较 UI-25 不变——无新增/删除 .ai-* 类)/contract:check(115:112) 全绿；零 Rust 改动
截图：遗留（tauri 运行态，同 UI-29 矩阵）
人工走查：浏览器面板标题与导入确认显示中文「浏览器」、助手头像与空态 logo 的 alt 读屏为中文/产品名；Python 运行按钮、工具卡输入/输出、SSH 主机/端口/用户名、Python 与 SSH 审计的标准输出/标准错误均为中文；5 处图标按钮读屏有可理解名称——遗留 UI-28/29
风险/回退：单 commit 可回退；纯文案/aria-label/alt 改动，无逻辑/布局/样式类变更（styles:report 计数不变可佐证）；tool-call-card DataSection label 联合类型同步收窄为中文字面量，tsc 通过保证调用点一致
阻塞或剩余事项：**基线已满足（登记）**——图标按钮容器尺寸合规：`ui/button.tsx` icon=h-8 w-8(32×32)/icon-sm=h-6 w-6(24×24)、`IconButton` 默认 32；shell 导航 AppRail 18/1.8、StatusDockBar(终端/浏览器)与 ContextNav 紧凑图标均带中文可见文本与 aria-pressed/aria-label。**剩余登记（图标）**——全应用 lucide `size=`/`strokeWidth` 存在「三方言」（导航 18/1.8、设置 16/1.5、聊天/面板/浏览器/Git 11–14/2；strokeWidth 实测 9 种取值、size prop 与 Tailwind h-*/w-* 两机制并存，最小 10 最大 40）：统一到 tokens.md §3 的 16/18 属全应用视觉走查项，盲改有溢出/观感回归风险且无法静态验证，登记 UI-29 携本批 inventory 定位后在运行态逐域收敛。**剩余登记（后端术语）**——后端 Rust 错误/工具结果串中的 CloakBrowser（`browser.rs:134,293`、`browser/process.rs:70,74,84`、`agent/tools/builtin/browser/actions.rs:214,224-225` 等）经 toast/工具卡透出给用户，属后端改动（需 cargo 重建 + 重启 tauri），本批前端-only 不动，登记后续小任务（仅改字符串、非命令契约/schema，contract 计数不受影响）。**决策（保留）**——MCP/SSH/RAG/Qdrant/Embedding/OCR/API Key/Python 与日志级别枚举 Debug/Info/Warning/Error、kbd Esc 为业界通用缩写/专有名词/枚举值，非内部技术名（dispatcher/nezha/aha/cloak/jkbot），保留；SSH 页描述中 `~/.jkcodingagent/jkbot.sqlite3` 为真实库文件路径（必要技术详情，改之则失真），保留
验收人/日期：待人工（UI-28/29/31）

## 15. UI-23–UI-24a（M3 批次三）任务记录（第 5 节模板）

任务：UI-23
负责人：claude（会话领取）
开始/完成日期：2026-09-07
基线/结果 commit 或 PR：基线 `3118a41`；结果 `a835fcb`（23a）/`7e665b1`（23b）/`3d9ec51`（23c）/`85db7b3`（23d），四 commit 独立可回退（23a 为 keyboard-bindings lib 被 23d 结构性依赖——回退 23a 须先回退 23d）
实现文件与范围：
23a 快捷键基础设施：`lib/keyboard-bindings.ts`（新增纯判定层：isMacPlatform 平台词边界判定〔修复裸 /Mac/ 被 "Node.js" UA 误命中〕、matchesBinding Mod=mac?meta:ctrl、isImeKeyEvent、isTypingTarget/isInsideContainer 结构化 closest、shouldSkipBinding 统一裁决=IME 全跳过→非 Mac 终端内 Mod 键放行 shell→Monaco allowInEditor→输入目标豁免裸键；+test 28 case）、`hooks/use-chat-shortcuts.ts`（消费纯函数层；defaultPrevented 早退；enabled 门控；handlers 改 ref 存最新值——旧实现 effect 依赖字面量 bindings 每渲染重挂 window 监听）、`chat/chat-shell.tsx`（ChatShellProps +enabled/containerRef；focusPrompt 作用域化）、`chat-page-v2.tsx`（workspaceVisible→enabled 贯通；handleEditMessage 输入框查询作用域化）、`layout/app-layout.tsx`（+containerRef prop 挂根节点）
23b Escape 栈裁决：`lib/overlay-stack.ts`（+peekOverlay/shouldHandleEscape 纯函数；+test 11 case 含「命令面板+Artifact 一键双关」回归场景）、`hooks/use-overlay-escape.ts`（新增：active 期间 push/pop、栈顶+defaultPrevented 裁决、cleanup 保证弹栈）、`hooks/use-focus-trap.ts`（新增：initialFocus/Tab 循环/关闭还原——仅焦点仍在容器内或 body 时还原，IME 组合期不拦 Tab；保守 focusable selector）、接线 5 处自研覆盖层（`chat/command-palette.tsx`、`chat/katex-copy.tsx`〔Escape 移交栈，外部点击/滚动关闭保留〕、`ChatNewCategoryDialog.tsx`〔补 role=dialog/aria-modal、去 setTimeout 聚焦〕、`file-viewer/useLargeFileKeyboard.ts`〔选区存在时 push 拥有 Escape——修复抽屉/选区与底层关 Artifact 抢键；补 defaultPrevented 与高层让路〕、`graph/GraphPanel.tsx`〔模范实现等价收敛到 useOverlayEscape——修复其 Esc 被 chat-shortcuts 抢先 preventDefault 导致节点抽屉关不掉〕）、`McpStatusDialog.tsx`（补 role=dialog/aria-modal/aria-label + Esc 可关〔此前键盘完全关不掉〕+ 焦点陷阱〔关闭按钮 initialFocus〕+ 关闭按钮 aria-label）、`use-chat-shortcuts.ts` Escape 补 Radix 跨栈让路（设置 Dialog/Sheet/下拉/Select 按 DOM data-state 检测——Radix 不 preventDefault，防一次 Esc 同关两层）
23c 分隔条键盘化：`lib/splitter-step.ts`（新增：splitterKeyDelta 轴向/invert 映射、nextSplitterValue ratio 0.02/Shift 0.1〔与既有 splitter 语义一致，两位小数消浮点累积〕、px 8/48、clamp、resetSplitterValue；+test 14 case）、`hooks/use-splitter-keyboard.ts`（新增：返回 role=separator 完整 ARIA〔tabIndex/aria-valuenow/min/max，ratio 报百分比〕+onKeyDown/onDoubleClick，config 走 ref 回调稳定，键盘步进即时提交）、接线 4 条缺失站点（`shell/ContextNav.tsx` 216–320/默认248、`layout/app-layout.tsx` 200–480/默认264、`ShellTerminalPanel.tsx` 裸 div→separator 100–600/默认240〔useProjectPanels 暴露 commitTerminalHeight 直通〕、`HomeChatPage.tsx` 裸 div→separator invert=ArrowLeft 加宽〔useDockedBrowserPanel 补 commitWidth/getWidthBounds/getDefaultWidth〕）、`project/ProjectWorkbenchContent.tsx`（内联方向键逻辑等价替换为 splitter-step）、`project/ProjectWorkspaceLayout.tsx`（splitter 补 aria-valuenow 百分比）、`styles/tailwind.css`（4 类分隔条 :focus-visible，accent 令牌无硬编码色值）
23d 键位+ARIA+列表导航：新增键位表——**Mod+1..4** 切 ContextNav 四页签（顺序与 workspace-prefs CONTEXT_TABS 单一出处；冲突审查：应用内无 Mod+数字占用、Monaco 不绑 Ctrl+1..4、xterm Ctrl+数字非控制码、Tauri 单窗口无浏览器标签切换）、**Mod+J** 切终端 dock 显隐（与 StatusDockBar 同一 nextTerminalDockState 路径，隐藏≠结束 PTY 保活；xterm 内 Ctrl+J=LF 由 23a 终端让路策略兜底透传 shell）、**Mod+Shift+A** 开/关 Artifact 详情（无选中详情 no-op 不制造空面板伪状态；打开记录触发点、关闭还原焦点）；均经 visible/enabled 门控。`hooks/use-global-shortcuts.ts`（新增通用注册 hook，use-chat-shortcuts 重构为委托）、`lib/roving-index.ts`（+test 14 case）、`lib/list-focus.ts`（DOM 薄层：焦点在列表外 ArrowDown→首项/ArrowUp→末项、scrollIntoView nearest）、keyboard-bindings.test 追加注册表无歧义门禁 2 case（穷举平台×修饰键，任一事件至多命中一个绑定+每绑定唯一可达）、`shell/ContextNav.tsx`（tab id/aria-controls、tabpanel id/aria-labelledby、roving tabindex+方向键 automatic activation）、`SessionPanel.tsx`（搜索框 ArrowDown 入列表+列表内 ↑↓/Home/End，Radix 菜单 defaultPrevented 让路；ul 补 aria-label）、`layout/sidebar.tsx`（nav 同模式+aria-label）、`chat/command-palette.tsx`（输入框入列表+列表内边界钳制不环绕）
对应问题：审计 A10（焦点隔离）、设计 §3.3（弹层焦点还原/保活不抢键/不注册多份全局快捷键）、§6（Mod 快捷键作用域、Monaco/终端/IME 不抢键、Escape 只关最顶层、分隔条键盘调整+ARIA 数值+双击复位、会话行列表语义、中文 IME 不误触）、UI-19 记录「隐藏面板仍持监听」性能口径关联项
测试命令及结果：全量 vitest 317→404 passed（35→41 文件，新增 keyboard-bindings/overlay-stack/splitter-step/roving-index/request-guard/message-list-metrics 六个测试文件）+ PI sidecar 11 passed；build/lint(--max-warnings 0)/styles:report(924:930，0 无引用)/contract:check(115:112，无 Tauri 命令变化) 全绿；零 Rust 改动
截图：遗留（tauri 运行态，同 UI-29 矩阵）
人工走查：①非 Mac 终端内 Ctrl+K/L/N/J 透传 shell；②中文 IME 组合期按键不触发快捷键；③双项目保活下隐藏项目不响应 Mod+K/N/B/L、命令面板不双开；④命令面板开→Esc 只关面板不关 Artifact（双关回归）；⑤McpStatusDialog Esc 可关+焦点还原到触发点；⑥执行图节点抽屉打开时 Esc 只关抽屉；⑦5 条分隔条 Tab 可达、方向键/Shift 大步/双击复位一致、aria-valuenow 读数正确；⑧Mod+1..4/J/Shift+A 按预期且隐藏工作区不响应；⑨会话列表/命令面板 ↑↓ 导航与搜索框入列表——全部遗留 UI-28/29（本批以自动化门禁为准，用户确认不做人工 UI 验收）
风险/回退：四 commit 独立可回退；23b revert 后 overlay-stack 回到零调用状态（hasOpenOverlay 恒 false）与批次前一致、hook cleanup 保证弹栈无残留；23c 纯增量 ARIA/键盘，鼠标拖拽路径零改动（ProjectWorkbenchContent 为等价替换，语义有既有行为对照）；23d 键位注册表快照测试为冲突审查自动化门禁
阻塞或剩余事项：**决策**——①「切区/打开详情/关闭返回」以新增全局键位（Mod+1..4/J/Shift+A）+ ARIA 补齐（tablist 方向键/列表 ↑↓/分隔条键盘）双路径满足，不引入 F6 焦点循环（macOS 默认 fn+F6 沟通成本高）；②Mod+J 在非 Mac 终端聚焦时透传 shell（Ctrl+J=LF 控制码），dock 收起改用面板头部隐藏按钮或先移出焦点——设计即如此，文档化；③Katex 右键菜单接焦点陷阱（打开即聚焦菜单按钮），鼠标流程不受影响；④大文件查看器 Escape 所有权动态化：仅选区存在时压栈（无选区时 Esc 归还底层关 Artifact，保持既有语义）。**遗留登记**——WebView2（Windows）浏览器加速键 Ctrl+J/1..4 转发行为需人工验证（UI-28）；BranchBar 分支弹窗（局部 onKeyDown+stopPropagation，调查判定基本无冲突）未接栈，如运行态发现抢键另登记；设置 Dialog（Radix modal）打开时 Mod+K 等仍会触发命令面板（跨栈 Mod 键让路未做——Radix 无 DOM 判定锚点区分 modal 打开态的 Mod 键策略，现状与批次前一致，登记可选后续）
验收人/日期：待人工（UI-28/29/31）

任务：UI-24（24a 切片；24b 主改造与热点切片见下条记录）
负责人：claude（会话领取）
开始/完成日期：2026-09-07（24a）
基线/结果 commit 或 PR：基线 `85db7b3`；结果 `21e70da`（24a-1）/`b3b2b8f`（24a-2）/`a9099fd`（24a-3）/`0a05499`（24a-4），各 commit 独立可回退
实现文件与范围：
24a-1 memo 击穿修复：`lib/request-guard.ts`（新增 createRequestGuard 工厂：begin/isStale/invalidate 纯逻辑，代号 0 为未领取哨兵恒判过期；+test 6 case）、`hooks/useSessionRequestGuard.ts`（变薄为 useMemo 持有恒定实例——旧实现每渲染返回 {begin,isStale} 字面量，chat-shell 的 handleOpenArtifact/handleOpenSubAgent deps 连带每渲染换身份）、`dispatcher-chat/useDispatcherActions.ts`（返回对象 useMemo 化，内部 action 已是 useCallback——旧实现击穿 chat-page-v2 handleRegenerateFromMessage）；效果：MessageItem React.memo 流式期间恢复生效（此前每个可见行每 token 全量 reconcile）
24a-2 role=log 多实例修复：`chat-page-v2.tsx` scrollMessageListToBottom 的 document.querySelector('[role="log"]') 全局选择器 → shellContainerRef 子树作用域（多项目保活 DOM 多个 role=log + 架构面板复用 MessageList，此前在 B 项目发送消息可能滚动隐藏的 A 项目列表）；全仓核对全局 role=log 查询仅此一处
24a-3 scroll 节流+观测：`chat/message-list.tsx`（滚动度量收敛单一 scrollMetrics state + rAF 合帧——旧实现每 scroll 事件 setState×2 无节流；同值短路；ResizeObserver 补容器 resize 通道〔切布局/分栏不产生 scroll 事件，旧 viewportHeight 初始 720 且只在 scroll 更新会过期〕；0 高度回调忽略〔保活隐藏容器防窗口化参数被打成 0〕；useLayoutEffect 挂载实测初值；未开窗〔≤300 条〕时 scroll/resize 零 setState；useAutoScroll 回调 ref 经本地组合 ref+state 跟踪）、`chat/message-list-metrics.ts`（新增：computeWindowRange/windowSpacerHeights 纯函数，语义与旧内联实现逐位一致；+test 14 case 含 300/301 阈值边界、底部钳制、负值防御、参数化估高/overscan）
24a-4 拖拽隔离：`project/ProjectWorkbenchContent.tsx`（对齐 ContextNav 范式：拖拽期本地 dragRatio state〔重渲染限 WorkbenchContent 子树〕，mouseup 一次性 onEditorPaneRatioChange 写回偏好；卸载兜底 cleanup 提交+摘监听；aria-valuenow 拖拽期跟手）、`project/workspace-budget.ts`（+splitDualPaneWidths 纯函数：拖拽期双栏宽度本地重算与 resolveWorkspaceBudget 同一钳制口径 minEditorWidth≤editor≤inner-minChatWidth，松手预算重算无跳变；+test 7 case）
对应问题：审计 A11（>300 条窗口化固定 180px 估高，伴生 scroll 无节流/viewportHeight 过期）、UI-19 记录「性能口径归 UI-24/30」、设计 §6 与回归矩阵「流式响应→滚动锚点可恢复」
测试命令及结果：全量 vitest 404 passed（41 文件；本任务新增 request-guard 6 + message-list-metrics 14 + workspace-budget 追加 7）；build/lint/styles:report(924:930)/contract:check(115:112) 全绿；零 Rust 改动
截图：遗留（性能验收不适用截图，见「附实际 profile」遗留）
人工走查：①流式长响应中 MessageItem memo 生效（DevTools Highlight Updates 抽查）；②双项目保活下 A 项目流式输出时 B 项目列表滚动位置不变、B 发送消息不滚 A 列表；③>300 条会话滚动流畅、切布局后窗口参数不过期；④编辑器占比拖拽流畅、松手后刷新页面比例已持久化、拖拽中途切项目无悬挂——遗留 UI-28/30
风险/回退：四 commit 独立可回退；24a-1 revert 仅性能退化无功能损失（useMemo deps 由 lint exhaustive-deps 门禁把关陈旧闭包风险）；24a-3 rAF 合帧使「最新」按钮出现晚一帧（不可感知），窗口化本体（rowEstimate=180/overscan=8/>300 阈值）不动；24a-4 提交路径未变仅时序变化（每次拖拽 1 次持久化写）
阻塞或剩余事项：**24b 切片已于同日晚些时候领取实施（见下条记录）**——本条登记时的 ①窗口化主改造/③行卸载状态提升/⑤merge O(n) 热点已完成；②「附实际 profile」验收、④useLiveSessionState 每 token setState、⑥终端高度拖拽全树重渲染、⑦会话列表虚拟化、⑧保活监听者累积仍遗留。**UI-24 状态保持 DOING**——「附实际 profile，不能只改算法后宣称更快」为本任务硬性验收，不虚报 REVIEW
验收人/日期：待人工（UI-28/30/31）

任务：UI-24（24b 切片：窗口化动态测量主改造 + 同链路热点）
负责人：claude（会话领取）
开始/完成日期：2026-09-07
基线/结果 commit 或 PR：基线 `d2d1f9c`；结果 `4b9a637`（24b-1/2）/`76bec9f`（24b-3）/`2cfd798`（24b-4）/`ab5c2c5`（24b-2 键分隔符 NUL 字节修正），各 commit 独立可回退（24b-3 结构性依赖 24b-1 的 RowUiStateProvider——回退 24b-1 须先回退 24b-3）
实现文件与范围：
24b-1 行级 UI 状态存储：`chat/row-ui-state.ts`（新增：createRowUiStateStore 不透明 key-value store + RowUiStateProvider Context + usePersistedToggle——挂载从 store 恢复初值、切换写回；store 由 MessageList 实例 ref 持有，identity 恒定不触发重渲染；无 key/无 Provider 时退化为普通 useState，语义与迁移前逐位一致；+test 5 case 含 false/undefined 可区分与实例隔离）、接线四处行内展开态——`tool-call-card.tsx` ToolCallCard 卡展开（key=`card:{toolCallId}`，id 全局唯一）、DataSection「展开全部」（`showall:{id}:input/output`）、ToolCallList 组展开（`tools:{rowId}`，rowId 由 AssistantMessage 透传 display item 稳定 id）、`reasoning-block.tsx` 思考块（`reasoning:{rowId}`）；流式气泡（StreamingMessage）不传 key 保持临时态语义
24b-2 Shiki 高亮 LRU 缓存：`utils/shiki-cache.ts`（新增：shikiCacheKey=`lang\0theme\0code`、createShikiHighlightCache——LRU Map 容量 200、get 命中提升新鲜度、超长代码 >64KB 不入缓存防单条目内存过大、失败不缓存可重试；+test 7 case）、`utils/shiki.ts` highlightCodeToHtml 先查缓存命中直返/miss 回填、`tool-call-card.tsx` JsonCode 初值与 effect 同步探测缓存——窗口化行重挂载命中即直出高亮 HTML 不再闪纯文本，主题切换先回退纯文本的旧语义保留（键含主题，miss 走异步回填）；MarkdownCodeBlock 经同一入口自动受益
24b-3 窗口化迁移动态测量：`chat/message-list.tsx`（>300 条开窗阈值语义不变，开窗后由「固定 180px 估高 computeWindowRange + spacer div」整体切换为 @tanstack/react-virtual useVirtualizer + measureElement 逐行 ResizeObserver 实测——estimateSize=180 仅首帧初值，修复审计 A11「跳读/大片空白」根因；行距 gap-6 烘焙为行 pb-6 参与实测；外层 column 为 useAutoScroll ResizeObserver 观察目标，高度=虚拟化总高 div+流式气泡，pinned 跟随与 measureElement 双跟随源无写入冲突〔DOM 级 scrollTop=scrollHeight 与 virtualizer 索引测量正交〕；items 身份变化〔会话切换/截断/finalize〕时 virtualizer.measure() 清索引键控测量缓存防旧行高错位，流式期间 messages 身份稳定不触发；24a-3 的手动 scrollMetrics state/rAF 合帧/ResizeObserver 度量管线整体移除——scroll 事件由 virtualizer 内部处理；MessageList 外包 RowUiStateProvider）、`chat/message-list-metrics.ts`（收缩为 shouldUseWindowing 阈值判定纯函数 + ROW_ESTIMATE_PX/OVERSCAN_ROWS/WINDOWING_THRESHOLD 常量单一出处；computeWindowRange/windowSpacerHeights 随 spacer 方案删除；test 重写 6 case 保留 300/301 验收边界）
24b-4 merge 归一化身份缓存：`dispatcher-chat/dispatcherChatUtils.ts`（normalizeCached：WeakMap 两级键控——原始 wire 对象→产物、产物对象→产物自身〔normalize 幂等，产物引用复用保持下游 memo 稳定〕；每次 finalize/dispatcher-session-updated 重入 merge 时 current 全量命中零开销，仅新到 wire 对象真正走 normalize+JSON.parse，消除 1000 条会话 O(n) 热点；对象丢弃后缓存随 GC 回收；+test 4 case：身份复用、同 id 覆盖为新对象、空 incoming 短路返回原数组、排序语义不变）
对应问题：审计 A11（固定估高跳读/空白主根因）、24a 记录遗留登记项 ①③⑤、设计 §6「流式响应→滚动锚点可恢复」
测试命令及结果：全量 vitest 416 passed（43 文件；本切片新增 row-ui-state 5 + shiki-cache 7 + dispatcherChatUtils 追加 4 + message-list-metrics 重写 6，删 computeWindowRange 旧 14 case 中 8 case 随方案作废）+ PI sidecar 11 passed；build/lint(--max-warnings 0)/styles:report(924:930，0 无引用)/contract:check(115:112) 全绿；零 Rust 改动
截图：遗留（性能验收不适用截图，「附实际 profile」见阻塞项）
人工走查：①>300 条会话快速滚动无跳读/大片空白（动态测量修正估高）；②滚出窗口的工具卡展开态/思考块展开态/「展开全部」态滚回后恢复；③滚回的行 JSON 高亮直出不闪纯文本；④流式中 pinned 跟随正常、上滚不被拉回、行高实测修正不打断阅读位置；⑤regenerate/编辑重发截断后行高不错位；⑥1000 条会话 finalize 时无明显主线程长任务（merge 缓存生效）——遗留 UI-28/30
风险/回退：四 commit 独立可回退（24b-3 依赖 24b-1 Provider，回退顺序登记）；24b-1 无 Provider 场景退化 useState 与迁移前一致；24b-2 缓存 miss 路径与旧实现逐位一致，仅新增命中捷径；24b-3 未开窗（≤300 条）路径渲染结构与 24a 完全一致（flex gap-6 全量渲染），开窗路径为方案替换——spacer 方案删除后无回退共存态，revert 即整体回 24a；24b-4 纯内存缓存无持久化面，WeakMap 无泄漏面
阻塞或剩余事项：**UI-24 验收「附实际 profile，不能只改算法后宣称更快」仍未满足——状态保持 DOING**。需运行态 DevTools Performance 采样（300/301/1000 条含长代码夹具数据），仓库无性能基建，本会话无运行态桌面环境，不虚报。其余遗留：④useLiveSessionState 每 token 立即 setState（改动影响流式手感，需与 profile 数据协同评估）；⑥终端高度拖拽 ProjectPage 全树重渲染（牵动 UI-19 两态机）；⑦SessionPanel/侧栏会话列表是否虚拟化（以 profile 数据决定）；⑧保活监听者累积盘点（归 UI-30）；「切布局后锚点正确」的按 index 锚点保持逻辑未单独实施——react-virtual 测量修正 + useAutoScroll recompute 闲置钩子可激活，随 profile 走查验证，不足再登记
验收人/日期：待人工（UI-28/30/31）
