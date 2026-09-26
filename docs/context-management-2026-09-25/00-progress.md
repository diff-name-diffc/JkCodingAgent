# 上下文管理重构优化 — 统一进度文档

> 起始日期：2026-09-25
> 目标：在满足上下文管理需求和 Agent 运行准确率不降低的前提下控制好上下文。
> 基准分析：聊天 agent（rig-core 0.42 portable contracts）现状梳理见本文档「背景与现状问题」节。

## 背景与现状问题

现状管线：`load_llm_history`（SQL 按最近 5 个 user 对话开窗，无大小预算）→ `chat_history_to_rig` → `run_rig_loop` 每轮全量直发，增量无裁剪回灌。

核心问题（按严重度）：

1. **主对话上下文无预算、无整形、无超限恢复** — 唯一裁剪是 `MAX_LLM_DIALOGUES = 5`（`src-tauri/src/agent/db/util.rs`），窗口内无限制；长 run 必撞窗口且 400 后整轮报废。
2. **压缩只作用于工具结果，不作用于对话历史** — `tool_result.rs` 的双标签摘要管线成熟但未复用到历史层；assistant reasoning 全量回灌。
3. **主对话与子智能体两套割裂策略** — 子智能体有滑窗无摘要（`sub_agent/context.rs`），主对话有摘要无滑窗；token 估算口径不一致。
4. **容量回路缺失** — `estimate_context_tokens` 仅编排器 debug 日志；`context_window_tokens` 快照落库后无 UI 消费、无控制回路。
5. 长期税：tool_call 配对读时补救（`common/message.rs` `repair_tool_call_pairing`）、`context_cleared` 死列、图片 base64 无缓存且每轮全量 clone。

对照 rig 官方最佳实践（rig-core `memory` 模块 + `rig-memory` crate）：`ConversationMemory` 持久化后端 → `MemoryPolicy` 整形（滑窗/TokenWindow）→ `Compactor` 逐出压缩为摘要前置（rolling summary）→ `DemotionHook` 长尾归档；窗口处理必须保持 tool_call/tool_result 配对（rig-memory 内置孤儿 tool-result 丢弃）。

## 阶段划分与进度

| 阶段 | 内容 | 状态 |
|------|------|------|
| 阶段 1 | 统一上下文整形层：预算（ContextBudget）+ 配对安全滑窗（shape_history 纯函数，无 LLM 压缩）+ 三处装配点接入（plain_chat / project / architecture）+ 循环内整形 | ✅ 已完成（2026-09-25） |
| 阶段 2 | 历史级压缩：rolling summary 复用摘要模型槽位（失败回退规则抽取）；reasoning 跨轮不回灌；摘要产物持久化（schema v6 `dispatcher_session_summaries`）与级联清理；context-length 400 超限收缩重试 | ✅ 已完成（2026-09-26） |
| 阶段 3 | 子智能体对齐到同一整形层；容量回路闭环（装配前估算超阈值触发压缩 + 前端占用展示） | ✅ 已完成（2026-09-26） |
| 阶段 4 | 长期税清理：读侧配对修复退化为防御校验、`context_cleared` 死列落地或移除、图片缓存与增量 attach | ✅ 已完成（2026-09-26） |

### 阶段 1 详细任务

- [x] `rig_ext/context.rs` 新模块：预算解析 `context_budget_chars`（库条目 `contextWindow`，缺省 1M → 字符预算 = window × 3.5 字符/token × 0.6 安全系数，预留 preamble/输出/本轮增长空间）
- [x] `shape_history` 纯函数：保头（首轮 user 意图）保尾（最近完整轮）、裁剪边界配对安全（起点只落在不含 ToolResult 的 User / 不含 ToolCall 的 Assistant 之前，否则持续后移）、裁剪处插「【上下文裁剪】」占位、`repair_pairing` 防御性兜底（复用 `UNANSWERED_TOOL_RESULT_PLACEHOLDER` 文案）、极端超预算头部截断 4000 字符不 panic
- [x] 单元测试：7 个用例覆盖预算边界/保头保尾占位/边界后移/孤儿剔除/占位补齐/极端输入
- [x] 装配方案：下沉到 `run_rig_loop`（`loop.rs`）内部统一执行——三处装配点（plain_chat / project / architecture）均已将 `spec.context_window` 回填进 `hooks.context_window`，循环入口计算一次预算、每轮迭代发请求前（`attach_turn_tool_images` 之前）对内存 messages 就地整形；装配点零改动、三路径全覆盖
- [x] 循环内整形：同上（与装配整形同一代码点）；落库路径（`persist_*`）以本轮新内容为参数、不读内存 messages，确认不受影响
- [x] `cargo check` 无错误无警告；`cargo test` 全量 560 passed / 0 failed / 1 ignored

### 阶段 2 详细任务

- [x] `rig_ext/context.rs` 滚动压缩：`trim_history`（`shape_history` 拆出，暴露被裁中段与占位下标，新增 `keep_head` 参数——头部是【前情摘要】时不再保护、并入新摘要）+ `compact_history`（异步，被裁中段渲染 → 摘要模型生成滚动摘要，缺省/失败回退零 LLM 头尾规则抽取；摘要消息替换占位留在内存序列，下次裁剪自然并入——rolling summary，对齐 rig-memory `CompactingMemory` 语义）
- [x] 摘要渲染与提示词：`render_messages_for_summary`（角色前缀、单条 1500 字符头尾截断、总量 24K、reasoning 不入摘要）、历史压缩专用系统提示（保留任务目标/关键决策/操作结果/未决问题，合并旧摘要）、摘要正文上限 3000 字符
- [x] reasoning 跨轮不回灌：历史装配（`message.rs`）不再把 `reasoning_content` 映射为 `AssistantContent::Reasoning`；运行内（`loop/support.rs::build_assistant_message`）追加进内存视图的 assistant 消息不再携带思考。依据：rig 的 openai 线格式会把 Reasoning 序列化进请求体，DeepSeek 等服务商明确要求历史不携带 reasoning_content；思考仍落库供 UI 展示
- [x] 跨 run 持久化（schema v6）：`dispatcher_session_summaries` 表（workspace_id 主键、summary、covered_through_message_id 锚点、updated_at；DDL 单出处 `SESSION_SUMMARIES_DDL`，基线与 v5→v6 迁移共用）；`db/session_summaries.rs` CRUD（`valid_session_summary` 单连接完成「读+锚点存在性校验+失效即删」）；装配侧 `message::apply_stored_session_summary` 前插摘要消息（三装配点接入）；运行循环以 `message_ids` 平行跟踪取覆盖范围内最近已知消息 id 作锚点 upsert（best-effort）
- [x] 级联清理：会话删除/清空走 `purge_session_resources_tx`（+摘要表）；`truncate_messages_from` 精确删除锚点落在被删范围的摘要（锚点早于截断点的仍有效，有意保留）；读取路径锚点校验兜底
- [x] context-length 400 超限恢复：`is_context_overflow_error`（主流服务商文案子串匹配，从宽——误判代价仅一次带压缩重试）→ 预算减半（下限 16K 字符）重试，每 run 至多 2 次
- [x] `MAX_LLM_DIALOGUES = 5` 评估结论：**保留不动**。窗口只决定「原文回灌」范围；滚动摘要已提供窗口外历史的连续性，放宽窗口只会线性抬高每轮请求成本而不提升准确率
- [x] 测试：context.rs 新增 6 个压缩用例（规则兜底折叠/旧摘要头并入/no-op/模型成功路径/渲染口径/超限错误识别）+ schema v5→v6 迁移测试（建表/锚点校验/幂等）；`message/tests.rs` reasoning 用例反转为「不回灌」断言。全量 `cargo test` 567 passed / 0 failed

### 阶段 3 详细任务

- [x] 整形层泛化：`trim_history` 的 `keep_head: bool` 改为 `header_len: usize`（主对话 1 = 首轮任务意图，子智能体 2 = system + 首轮任务），头部末条带未应答 ToolCall 时自动收缩（防孤儿调用）；`compact_history` 拆出 `prepare_fold`（超预算判定 + 滚动摘要头折叠 + trim + 渲染）/ `finalize_fold`（摘要消息 1:1 替换占位）两段式
- [x] `compact_history_offline`（同步、非泛型、零 LLM 规则兜底）：子智能体 runner 从自有滑窗（`sub_agent/context.rs` 的 `trim_context_messages`，轮次语义）切到统一整形层——`sub_agent/context.rs` 整文件删除，token 估算口径（`message_chars`）与预算公式（`context_budget_chars`）主对话/子智能体归一
- [x] 子智能体 reasoning 同步剥离（`build_assistant_turn` 不再携带思考），与主对话同口径
- [x] 容量回路闭环（前端）：`chat-page-v2` 从 `useDispatcherSessionTokenUsage` 取 primary 来源最新一条，头部渲染 `ContextUsageIndicator`（`ai-chat-header-usage`，占用 ≥70% 警示色 / ≥90% 危险色，tooltip 说明历史折叠行为）——此前 `entries` 被解构丢弃（死数据）现已消费
- [x] 「装配前估算超阈值触发压缩」评估结论：**不需要单独做**。`compact_history` 在运行循环第 0 次迭代（首个请求发出前）即执行，装配后超预算的历史在进入模型前就被折叠；`project.rs` 的 `context_debug` 估算日志保留为诊断旁路
- [x] 验证：`cargo test` 565 passed / 0 failed（-2：删除的 sub_agent/context.rs 测试，语义已由 context.rs 覆盖）；`pnpm lint` 零警告、`pnpm styles:report` 0 无引用定义、`pnpm test` 569 passed、`pnpm build` 通过

### 阶段 4 详细任务

- [x] 读侧 `repair_tool_call_pairing` 退化为防御校验：行为不变（补齐/剔除语义保留），新增触发留痕（补占位/剔除孤儿计数 eprintln）——写侧已保证成对落库，读侧频繁触发即写侧回归信号
- [x] `context_cleared` 死列移除（schema v7）：事务内重建 dispatcher_messages（INSERT SELECT + ORDER BY rowid 保持物理顺序），DROP 父表前事务外关闭 `foreign_keys`（否则隐式 DELETE 触发 chat_images/python_code_runs 等子表 ON DELETE CASCADE 误删），提交后无论成败恢复；读侧 4 处过滤与索引随列移除；`scripts/backfill_chat_keywords.py` 同步删谓词；迁移测试覆盖「列删除 + 数据保留 + 子表行不丢失 + 顺序保持 + 幂等重开」
- [x] 图片 attach 一次性解析：`attach_turn_tool_images` 改为就地写入内存视图（已解析 base64 驻留，后续迭代零磁盘重读/零重编码），去重从「仅最后一条用户消息」扩为全序列扫描（图片驻留后可能挂在更早消息上）；无新引用时不做任何分配
- [x] 验证：`cargo test` 566 passed / 0 failed，`cargo check` 零警告，python 脚本语法校验通过

### 遗留观察项（不影响本次目标，记录在账）

- `RigLoopHooks::from_chat_spec` 的 `max_iterations: 200` 默认值对编排器路径偏宽松，属语义审查项而非上下文问题。
- 滚动摘要的持久化锚点只覆盖运行内追加的消息（装配前缀无落库 id）；极端场景（单 run 内未追加任何消息即压缩）摘要不落库，下一 run 重新压缩——可接受的重复成本。

### 统一 token 估算口径（阶段 1-3 已归一）

`rig_ext/context.rs::message_chars` 是主对话与子智能体的唯一口径（作用于 rig `Message`：正文/reasoning 计字符，ToolCall 计 name+arguments，ToolResult 计内容，Image 固定成本 2000 字符/张）。存量遗留：`db/mod.rs::estimate_context_tokens`（作用于 ChatMessage）仅 `project.rs` 的 context_debug 诊断日志使用，属诊断旁路，不影响整形决策。

## 变更日志

- 2026-09-25：建立本文档；阶段 1 开工。
- 2026-09-25：**阶段 1 完成**。新增 `src-tauri/src/agent/rig_ext/context.rs`（494 行，含测试）；`loop.rs` 每轮迭代发请求前接入 `shape_history`（预算 `hooks.context_window` → `context_budget_chars`）；`common/message.rs` 的 `UNANSWERED_TOOL_RESULT_PLACEHOLDER` 提升可见性供整形兜底复用；`AGENTS.md` 容量参数条目同步。`MAX_LLM_DIALOGUES` SQL 窗口保留不动（双保险，阶段 2 评估放宽）。已知边界：整形只影响发给模型的视图；极端预算下尾部孤儿工具结果会被 repair 剔除，请求仍合法不 panic。
- 2026-09-26：**阶段 2 完成**。`context.rs` 拆出 `trim_history` 并新增 `compact_history` 滚动压缩（摘要模型 + 规则兜底，旧摘要头并入保持滚动连续）；reasoning 全链路不回灌（装配 + 运行内追加）；schema v6 新增 `dispatcher_session_summaries`（锚点校验 + purge/truncate 级联清理 + 迁移测试）；运行循环接线压缩 + `message_ids` 锚点跟踪 + 400 超限预算减半重试（≤2 次）；三装配点前插已存摘要；`MAX_LLM_DIALOGUES=5` 评估后保留（窗口外历史由摘要承接）。全量测试 567 passed / 0 failed。已知边界：摘要质量依赖压缩槽位模型（缺省时规则兜底仍保头尾）；装配前缀消息的锚点为 None，仅运行内追加的消息可产出持久化锚点（覆盖纯装配前缀的摘要不持久化，下一 run 重新压缩）。
- 2026-09-26：**阶段 3 完成**。整形层 `header_len` 泛化 + `compact_history_offline`（零 LLM）；子智能体 runner 切到统一整形层（`sub_agent/context.rs` 整文件删除，预算/估算口径归一，reasoning 同步剥离）；前端头部新增上下文占用指示（消费 `useDispatcherSessionTokenUsage` 的 `entries`，原死数据），容量回路闭环。`cargo test` 565 passed、`pnpm lint`/`styles:report`/`test`/`build` 全绿。
- 2026-09-26：**阶段 4 完成**。读侧 `repair_tool_call_pairing` 退化为防御校验（触发留痕）；schema v7 移除 `context_cleared` 死列（重建表 + 外键关闭防级联误删 + 迁移测试）；图片 attach 就地解析驻留（去重扩为全序列），消除每轮迭代的磁盘重读/重编码。全量 `cargo test` 566 passed / 0 failed，`cargo check` 零警告。**四阶段全部完成。**

## 最终管线总览（重构后）

```
装配（三路径统一）:
  load_llm_history（SQL 最近 5 轮窗口 + 可见性/plumbing 过滤 + 读侧配对防御校验）
  → apply_stored_session_summary（前插 dispatcher_session_summaries 滚动摘要，锚点失效即删）
  → chat_history_to_rig（reasoning 不回灌；chat-image:// 解析为 base64）

运行循环（run_rig_loop，每轮迭代）:
  compact_history（超预算 → 被裁中段折叠为【前情摘要】滚动摘要；摘要模型 15s，
    失败回退零 LLM 规则抽取；旧摘要头并入新摘要；持久化锚点 = 覆盖范围最近已知消息 id）
  → attach_turn_tool_images（就地附加，图片解析一次驻留）
  → build_completion_request → model.stream（400 上下文超限 → 预算减半重试 ≤2 次）

工具结果（不变，原有成熟管线）:
  显式 compress + 阈值双条件 → 双标签摘要（15s，失败回退规则抽取）；
  未摘要按工具分档内联上限截断，完整原文进工具产物

子智能体: 同一整形层（compact_history_offline 零 LLM 规则兜底，头部 system+任务恒保护）

容量回路: 每轮用量落库（context_window_tokens 快照）→ 前端头部占用指示
  （≥70% 警示 / ≥90% 危险，tooltip 说明折叠行为）
```
