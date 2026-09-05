# 架构画布 Agent 能力优化追踪（2026-09）

> 目标：把「架构设计」tldraw 画布从"Agent 只会画"升级为"Agent 能读结构、能组织、能指、能带走"。
> 基线调研与能力清单依据：tldraw 官方文档（tldraw.dev/docs，llms-full.txt）+ 本机 `@tldraw/editor@5.3.2` 类型定义逐项核验。
> 状态图例：✅ 完成 · 🚧 进行中 · ⬜ 计划中 · ✂️ 已否决

## 一、当前基线（优化前）

| 维度 | 现状 |
|---|---|
| 操作指令 | 7 条：`create_shape`（geo/note/text/frame）· `create_arrow`（binding 连接）· `update_shape` / `update_arrow` · `move_shape` · `delete_shape` · `layout` |
| 感知 | 双通道：结构化快照（`canvas-snapshot.ts` 文本清单）+ 全画布截图；执行后回传受影响区域截图 |
| 事务 | `markHistoryStoppingPoint` + `squashToMark`（一次 Ctrl+Z 撤销整轮）/ `bailToMark`（all-or-nothing 回滚） |
| 闭环 | Rust 权威校验（错误一次列全）→ `architecture-run-request` 事件往返 → ≤950 字符报告含 ref→shapeId 映射 |

**三个结构性缺口**（本轮优化围绕它们）：
1. 快照没有关系信息——箭头连谁、谁在哪个 frame 里、谁被锁定，模型看不见图拓扑；
2. frame 能创建却不能把形状放进去（无 reparent 能力）；
3. 没有选中/视口控制——Agent 只能"画"，不能"指着说"。

## 二、批次规划总览

| 批次 | 范围 | 状态 |
|---|---|---|
| 第一批 | #1 快照关系增强 · #2 frame 容纳（into + reparent）· #3 选中与相机控制 | ✅ |
| 第二批 | DSL 指令补全：group/ungroup、Z 序、复制/翻转/旋转、锁定 | ⬜ |
| 第三批 | 输出与闭环：导出 PNG/SVG 入库、序列化/模板库、手绘演示批注、箭头增强 | ⬜ |
| 第四批 | 愿景项：多页面、变更感知、新形状类型（bookmark/embed/image）、图→代码 | ⬜ |

---

## 三、第一批明细（本次迭代）

### #1 快照关系增强（感知先行）

让模型读懂图结构：箭头两端连接、父子容器、锁定状态、用户当前选中。

| 子任务 | 落点 | 状态 |
|---|---|---|
| `SnapshotShapeInput` 扩展 `parentId` / `locked` / `arrowEnds` | `canvas-snapshot.ts` | ✅ |
| `SnapshotInput` 扩展 `selectedIds`，头部渲染「选中」行 | `canvas-snapshot.ts` | ✅ |
| 箭头行改为 `from=shape:x to=shape:y`（读 arrow binding） | `canvas-snapshot.ts` | ✅ |
| 单测：箭头连接 / 父容器 / 锁定 / 选中四类渲染 | `canvas-snapshot.test.ts` | ✅ |
| 系统提示词「你的感知」章节同步行格式说明 | `prompt.rs` | ✅ |

关键 API（✓ 5.3.2 已验证）：`editor.getBindingsFromShape(id, "arrow")`（props.terminal + toId）、`shape.parentId`、`shape.isLocked`、`editor.getSelectedShapeIds()`。

### #2 frame 容纳（into + reparent）

补齐最大功能缺口：模块化架构图的容器语义。

| 子任务 | 落点 | 状态 |
|---|---|---|
| `create_shape.into` 字段（Rust AST + 校验） | `program.rs` | ✅ |
| `reparent` 指令（Rust AST + 校验，`parent:"page"` 回页面根） | `program.rs` | ✅ |
| JSON Schema 同步（into / reparent 分支） | `program_schema.rs` | ✅ |
| 前端类型 + 防御校验 | `arch-program.ts` | ✅ |
| 执行：创建后 `reparentShapes`（保持页面坐标）；frame 内自动放置槽位 | `arch-apply.ts` | ✅ |
| 解析：into/parent 解析 + 类型检查（frame）+ 循环包含检查 + 箭头/frame 禁入 | `arch-executor.ts` / `arch-apply.ts` | ✅ |
| 系统提示词 DSL 章节 + 工具描述 | `prompt.rs` / `architecture_run.rs` | ✅ |
| 单测：Rust 校验 + 前端校验 | `program.rs` tests / `arch-program.test.ts` | ✅ |

关键 API（✓ 已验证）：`editor.reparentShapes(ids, parentId)` —— 源码确认**保持形状页面坐标与旋转**，故「页面级创建（绝对坐标语义不变）→ reparent 进 frame」方案成立；`editor.getCurrentPageId()` 用于回页面根。

### #3 选中与相机控制（select_shapes + camera）

Agent 能"指着图说话"：圈出目标形状、大图导航。

| 子任务 | 落点 | 状态 |
|---|---|---|
| `select_shapes` 指令（targets + 可选 zoom） | 六件套 | ✅ |
| `camera` 指令（fit / point 两模式） | 六件套 | ✅ |
| 报告统计新增 `reparented` / `views` 计数 | `arch-report.ts` / `arch-executor.ts` | ✅ |
| 单测：Rust + 前端校验 | `program.rs` tests / `arch-program.test.ts` | ✅ |

关键 API（✓ 已验证）：`editor.setSelectedShapes(ids)` · `editor.zoomToSelection(opts?)` · `editor.zoomToFit(opts?)` · `editor.centerOnPoint(point, opts?)`。

---

## 四、后续批次清单（含已核验 API）

### 第二批：DSL 指令补全（⬜）

| 能力 | 关键 API（✓ 5.3.2） | 备注 |
|---|---|---|
| group / ungroup | `groupShapes(ids, {groupId?, select?})` / `ungroupShapes(ids)` | group id 回传报告映射 |
| Z 序 | `bringToFront` / `sendToBack` / `bringForward` / `sendBackward`（`{considerAllShapes}`） | frame 内用 `bringToFrontInParent` |
| 复制 | `duplicateShapes(shapes, offset?)` | |
| 翻转 | `flipShapes(ids, 'horizontal' \| 'vertical')` | |
| 旋转 | ✗ 无 `rotateShapes`（5.4 系为 `rotateShapesBy`）→ `updateShapes` 写 `rotation` prop | 5.3.2 替代方案 |
| 锁定 | `toggleLock(ids)` + `isLocked` | 与快照 `locked` 标注联动 |

### 第三批：输出与闭环（⬜）

| 能力 | 关键 API | 备注 |
|---|---|---|
| 导出 PNG/SVG 到项目 | `editor.toImage(ids, opts)` / `editor.getSvgString(ids, opts)`（✓）；✗ `editor.exportAs` 不存在，v5 用前两者 | 新 `architecture_export` 工具 + Tauri 落盘命令（路径校验 + spawn_blocking） |
| 序列化/模板库 | `editor.store.serialize('document')` / `loadSnapshot` / `mergeRemoteChanges`（RecordStore） | 对齐 `canvas-snapshot.ts` 注释预留路线；`.arch.json` 入库可 diff |
| 手绘演示批注 | `editor.scribbles.addScribble({color,size})` → `addPoint` → `stop`（✓ ScribbleManager） | 非破坏性、不进持久化，"解释模式" |
| 箭头增强 | `updateBindings`（focus/gap/isPrecise/isExact）、自由端点箭头（无 binding 的 start/end + bend） | |

### 第四批：愿景项（⬜）

| 能力 | 关键 API / 方案 |
|---|---|
| 多页面 | `createPage` / `setCurrentPage` / `duplicatePage` / `moveShapesToPage`（✓） |
| 变更感知 | `editor.store.listen(handler, {source:'user'})` → "用户手动改了 N 处"提示 |
| 新形状类型 | `bookmark`（最便宜）→ `embed`（`EmbedShapeUtil.configure`）→ `image`（需 asset 管线） |
| 图 → 代码 | 快照+截图 → 项目图编排器生成脚手架（相对官方 make-real 的差异化位） |
| 手势操作 | `@tldraw/driver`（pointerDown/Move/Up 走真实工具状态机） | 仅当需要模拟拖拽手势时 |

## 五、已知技术债

| 债 | 说明 | 处置 |
|---|---|---|
| ~~`program.rs` 超 500 行红线~~ | 权威 AST 文件在本批前已 958 行，本批新增指令后增长至 1150 行 | ✅ 已清偿（2026-09-01）：按「变化原因」拆为 `program_ast.rs`（AST 类型）/ `program_validate.rs`（权威校验），`program.rs` 保留契约常量 + 统一再导出 + 测试；拆分后 368 / 377 / 427 行，全部低于红线。详见变更日志 |
| `@tldraw` docs 按 5.4.x 撰写 | `animateCamera` / `rotateShapes` / `queryShapes` 在 5.3.2 不存在 | 升级 5.4 前禁用；本文档各条目均标注 ✓/✗ 本地核验结果 |
| tldraw 许可证 | 当前为 EVALUATION 试用 key（hosts=*），2026-09-15 到期 | 与画布能力无关，但到期后生产包画布被门禁卸载；见 `.env.production` 注释 |

## 六、变更日志

- 2026-09-01：建立追踪文档；完成基线盘点与 tldraw 官方文档能力调研（40+ 页面）；第一批开工。
- 2026-09-01：**第一批完成** ✅。落地明细：
  - **感知**：快照新增箭头连接关系（`from=`/`to=`，读 tldraw arrow binding）、父容器归属（`parent=`）、锁定标记（`locked`）、头部用户当前选中（`选中: …`）；自由箭头（两端未连接）退回位置尺寸表示。
  - **DSL 新指令 ×3**：`reparent`（移入/移出 frame，`parent:"page"` 回页面根；禁箭头/禁 frame 目标/循环包含检查）、`select_shapes`（1~30 目标，可选 `zoom`）、`camera`（`fit` 全览 / `point` 居中）；`create_shape` 新增 `into`（直接建在容器内，frame 内 4 列槽位自动放置，坐标保持页面绝对语义——先页面级创建再 `reparentShapes`，后者源码确认保持页面坐标）。
  - **六件套全同步**：`program.rs`（AST + 权威校验 + 3 组新测试）· `program_schema.rs`（3 个新 oneOf 分支 + 测试）· `prompt.rs`（感知说明 + DSL 8/9/10 + 容器/指认约束）· `arch-program.ts`（类型 + 防御校验）· `arch-apply.ts`（执行）· `arch-executor.ts`（解析 + 统计）· `architecture_run.rs` 工具描述。
  - **拆分**：报告构造从 `arch-program.ts` 拆出为 `arch-report.ts`（+`arch-report.test.ts`），避免新指令使文件超 500 行红线；统计新增 `容器 N / 视图 N` 两类动作。
  - **验证**：Rust 18 项 architecture 测试、前端 112 项单测全绿；`tsc` + 生产构建、ESLint、`styles:report`（0 无引用）、`contract:check` 全部通过。
- 2026-09-01：**`program.rs` 红线拆分完成** ✅（清偿已知技术债；独立任务，未与功能迭代混做）。落地明细：
  - **拆分**（按追踪计划）：AST 类型（程序信封 / 指令枚举 / 样式枚举 / 指令结构体）→ `program_ast.rs`；Rust 侧权威语义校验（`validate_program` + `check_*` 助手）→ `program_validate.rs`；`program.rs` 瘦身为模块入口——保留 schema 与校验共用的契约常量、统一再导出与全部 13 项测试。1150 行 → 368 / 377 / 427 行，全部低于 500 行红线。
  - **模式**：仿 `tools/program/`（私有子模块 + 入口再导出）。外部导入路径（`architecture_run.rs` 的 `program::{validate_program, ArchProgram}`、`program_schema.rs` 的常量）零改动；`mod.rs` / `prompt.rs` / `program_schema.rs` 文档引用同步。
  - **附带修复**：清零 `cargo clippy --all-targets -D warnings` 的两处存量问题（`broker/authorization.rs` 的 `authorize` 错误变体改 `Box<ToolResult>`，与同文件 `authorize_resource_scope` 既有模式一致；`grep_fallback.rs` 字节数组字面量按 clippy 建议改写）；`cargo fmt --all` 全量校准本分支工作区格式（CI 门禁强制）。
  - **验证**：Rust 466 项测试全绿；`cargo fmt --check`、`cargo clippy --all-targets -D warnings`、`cargo check` 全部通过。纯后端重构，无前端改动。

- 2026-09-02：**审查遗留优化清单全量清偿** ✅（第七节 13 项一次性完成，验证全绿）：
  - **防御层补全 + 拆分**：`validateArchProgram` 从 `arch-program.ts` 拆出为 `arch-program-validate.ts`（类型与校验变化原因不同；再导出保持导入路径不变），补齐枚举白名单（shape/geo/layout mode/align/箭头 kind/arrowhead/样式枚举）与数值校验（gap ∈ [0,500]、columns 整数 1..8、origin 有限、labelPosition ∈ [0,1]、w/h ∈ [1,2000]）——非法值不再静默落入默认分支或把未赋值 partial 传给 tldraw。
  - **失败定位**：执行器 apply 阶段失败报告传真实指令下标与类型（此前固定「第 1 条」误导模型重试位置）。
  - **权威校验完备性**：`check_ref` 显式拒绝空串（空迭代器 vacuous truth 恒真的漏洞）；w/h 契约与 schema 统一为 [1, 2000]（消除 (0,1) 区间漂移），配边界测试。
  - **Schema 错误消息**：校验错误总数改为截断（take 16）前统计，「共 N 处错误」不再最多报 16；oneOf「最接近分支」平局时优先判别式 `_type` 匹配的分支（`discriminator_penalty`：子错误路径以 `/_type` 结尾——实际携带完整路径 `/item/_type`——计为降权项），不再固定命中声明顺序靠前的分支。
  - **杂项**：`architecture_run` 超时文案改由 `ARCH_RUN_WAIT_TIMEOUT.as_secs()` 推导；画布执行异常报告过 `truncateArchReport` 硬截断（守住 950 字符契约）；`dispatcher_stop_run` 补 `invoke<void>` 泛型；`credentials_complete` 更名 `endpoint_ready` 并注明「本地端点允许空 key、云端漏 key 由首次请求显式报错」；`INTERNAL_CHAT_CATEGORY` 注释路径修正为 `types/architecture.ts`；快照 `round1` 更名 `roundInt`（实现一直是取整）；`arch-report.test` 补真正走硬截断分支的用例（超长文本 + 末尾截图引用保留 + 引用唯一性）。
  - **验证**：前端 Vitest 117（+5）/ PI 11、tsc + 生产构建、ESLint 全绿；Rust 469（+3：空 ref / 尺寸边界 / oneOf 平局）、`cargo fmt`、`cargo clippy --all-targets`（0 警告）全绿。

## 七、提交前审查遗留优化清单（2026-09-02）

> 2026-09-02 提交前全面审查（OCR AI 审查 60 文件 + 人工通读核心实现）中定性为**低优先级**的遗留项。审查当日修复了 1 项高优先级（frame 内绝对坐标/layout 的父坐标系换算）与 5 项中优先级问题；本节 13 项已于同日全量清偿（见变更日志当日条目），表格保留作审计留痕。

| # | 遗留项 | 落点 | 状态 |
|---|---|---|---|
| 1 | 防御层缺枚举/数值校验（layout.mode、create_shape.shape、gap/columns/origin、labelPosition、w/h） | `arch-program-validate.ts`（自 `arch-program.ts` 拆出） | ✅ 已清偿（2026-09-02） |
| 2 | apply 阶段失败报告固定传下标 0，误导模型重试位置 | `arch-executor.ts` | ✅ 已清偿（2026-09-02） |
| 3 | `check_ref` 空串 vacuous truth 误判合法 | `program_validate.rs` | ✅ 已清偿（2026-09-02） |
| 4 | w/h 契约漂移：schema [1,2000] vs 校验 (0,2000] | `program_validate.rs` | ✅ 已清偿（统一为 [1, 2000]） |
| 5 | 校验错误 take(16) 后统计 total，「共 N 处」最多报 16 | `tools/registry.rs` | ✅ 已清偿（2026-09-02） |
| 6 | oneOf 最接近分支平局按声明顺序取，`_type` 拼错时指引失准 | `tools/registry.rs` | ✅ 已清偿（判别式匹配分支优先） |
| 7 | 超时文案硬编码「20 秒」 | `builtin/architecture_run.rs` | ✅ 已清偿（常量推导） |
| 8 | 执行异常手工报告未过 `truncateArchReport`，可破 950 上限 | `arch-run-listener.ts` | ✅ 已清偿（2026-09-02） |
| 9 | `dispatcher_stop_run` 缺返回类型泛型 | `useArchitectureChat.ts` | ✅ 已清偿（`invoke<void>`） |
| 10 | `credentials_complete` 忽略 api_key 与命名不符 | `state/mod.rs` | ✅ 已清偿（更名 `endpoint_ready` + 意图注释） |
| 11 | `INTERNAL_CHAT_CATEGORY` 注释路径指向错误 | `db/sessions.rs` | ✅ 已清偿（改指 `types/architecture.ts`） |
| 12 | `round1` 实为取整，命名误导 | `canvas-snapshot.ts` | ✅ 已清偿（更名 `roundInt`） |
| 13 | 报告截断用例未真正走到截断分支 | `arch-report.test.ts` | ✅ 已清偿（补超长文本直测用例） |
