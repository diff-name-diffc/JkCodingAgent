# 跨 Run 工具任务持久化序列设计（tool_task_query / 跨 run 等待 / 进度落库）

状态：设计稿，待评审。考察基线：2026-10-09（工作区含 wait_for_tools 条件等待改造，未提交）。本文是 `agent-async-loop-2026-09-26.md` 的第二阶段姊妹篇：那份设计的首版范围明确划出了三个「不做」——「不实现后台任务跨应用重启续跑，不默认让工具脱离所属 run，也不增加模型轮询任务状态的工具」。其中第二条已被 `hand_off`（结算监督移交后台）事实上解除，第三条正是本次要受控解除的对象；**第一条维持不做**（重启后已开始的任务仍按 `external_state_unknown` 结算，绝不自动重放副作用，见 `db/tool_completions/recovery.rs` 的既定原则）。

## 1. 背景与目标

**痛点**：长时工具（10 分钟级的 build / 同步 / 远程命令）经常超出两道预算——决策预算（`max_iterations`，每轮显式 wait 也计 1 次）与等待预算（`timeout_secs` 上限 3600s）。预算耗尽后 run 收口、工具转后台，现状有四个缺口：

| # | 缺口 | 现状证据 |
| --- | --- | --- |
| G1 | 模型没有主动查询入口——新 run 里无法问「之前那个 build 怎么样了」，只能靠 run 开头的自动恢复（已完成的部分）或重跑命令（副作用风险） | 工具面无任何任务查询工具；`wait_for_tools` 只作用于本 run 的调度器任务 |
| G2 | 本 run 进行中，前 run 的 hand_off worker 完成 → 本 run 不可见——`absorb` 按 `agent_run_id + scope_id` 过滤（`scheduler.rs::absorb` → `pending_tool_completions`），跨 run 事件要等到**再下一个** run 的 `recover_completions`（`loop.rs:107`）才被吸收 | 用户在工具完成后 1 秒发问，也要再等一轮才能看到结果 |
| G3 | 预算耗尽 / 取消收口时不移交在途清单——`max_iterations` bail（`loop/decision.rs` 末尾）与 `finalize_cancelled` 不告知「还有 N 个后台任务、task_id 为 X」；下一 run 的模型不知道有东西可查可等 | 错误文案只有「已达到最大工具迭代次数」 |
| G4 | 进度粒度不足——`phase` 只有 queued/reviewing/preparing/running/cancelling 五个粗值（`delivery.rs::set_tool_task_phase` 白名单），长命令无输出尾部等进度素材，「还在跑」时模型只能得到 running + 已耗时 | `dispatcher_tool_runs` 无进度字段 |

**目标**：把「工具任务的事实序列」变成跨 run 持久可读的一等公民——

1. 模型可随时查询本会话工具任务的状态、进度与结果（已完成 / 在途 / 未观察）；
2. 等待可跨越 run 边界：显式等待一个上一 run 遗留的后台任务，完成即唤醒；
3. run 收口时把在途任务清单移交给下一 run（信息闭环）；
4. 全链路闭环：工具执行完**必然**更新持久化序列（进度与终态），任何路径（正常 / 取消 / hand_off / 进程重启）都不留无终态的孤儿行。

**非目标（本期不做）**：

- 跨应用重启续跑副作用（维持 recovery 既定原则：重启 = 事实未知，不自动重跑）；
- 子智能体挂查询工具面（子 scope 内部事实由父调用报告，`recovery.rs` 既有口径；子智能体的在途管理已由共享调度器 + 在途快照覆盖）；
- 前端进度尾部展示（`ToolRunUpdated` 已携带 phase；进度可视化另立需求，本期只落库 + 供模型）；
- 通用的任务取消工具（按 task_id 取消历史任务需要跨 run 的 cancel 通道，另立设计；本期只读）。

## 2. 已有的闭环底座（不重复建设）

| 组件 | 位置 | 职责 |
| --- | --- | --- |
| 终态 outbox | `db/tool_completions.rs::settle_tool_completion` | 终态 + 产物 + 完成事件**单事务**，幂等（重复结算返回首事件）；终态词表与台账共用 |
| 唯一应答 / 迟到观察 | `delivery.rs::reply_to_tool_task` / `deliver_tool_completion` | accepted 回执与 runtime 观察消息均幂等；观察确认 `observe_tool_completions` 在有效模型响应后落 `observed_request_step` |
| 跨 run 恢复 | `recovery.rs::pending_root_tool_completions` + `loop.rs:107 recover_completions` | 新 run 开始时吸收本 workspace 根 scope 的未观察完成事件，经 `coordinator.deliver` 的 recovered 分支重新交付 |
| 重启兜底 | `state/mod.rs:70 recover_interrupted_tool_tasks` | planned/running 无 completion 的行强制结算为 failed（`external_state_unknown` / `interrupted_not_started`），并补齐缺失应答 |
| run 结束收尾 | `loop.rs:124 shutdown` → `drain(SETTLE_CEILING)` → `hand_off` | 在途 worker 后台跑完，结算照落 DB（不 abort future、不阻塞调用方） |
| 进度粗粒度 | `dispatch.rs` 各阶段 `update_phase` | reviewing/queued/running/preparing 已落 `dispatcher_tool_runs.phase` |

**结论**：写侧闭环（终态必然落库）已经成立；缺的是**读侧**（模型查询）、**跨 run 吸收的时效**（G2）、**收口移交**（G3）与**进度素材**（G4）。

## 3. 方案总览

四个模块按依赖解耦，可独立分阶段落地：

```
P1  M1 tool_task_query 查询工具 ──┐          （纯增量，无 schema 变更）
    M3 收口在途移交清单 ──────────┤
P2  M2 wait_for 跨 run 外部目标 ──┤          （依赖 M1 的任务可见性约定）
P3  M4 progress_json 进度落库 ────┘          （schema v17 迁移）
```

## 4. M1 查询工具 `tool_task_query`

### 4.1 契约

- **位置**：`agent/rig_ext/tools/task_query.rs`（新组），挂入普通聊天面（`agents/plain_chat.rs::build_surface` 汇总处）；子智能体不挂（§1 非目标）。
- **参数**（宽容解析，对齐 `wait_for_tools` 的模型面向口径）：

```json
{
  "task_ids": {"type": "array", "items": {"type": "string"},
    "description": "只查这些任务（id 取自 accepted 回执、wait 结果或在途快照）；缺省查全部未收口任务"},
  "include_observed": {"type": "boolean",
    "description": "是否包含已进入过上下文的已完成任务；缺省 false（只返回在途 + 未观察的完成）"}
}
```

- **查询范围**：`dispatcher_tool_runs` 按 `workspace_id` 过滤（workspace 即会话 id，天然隔离），`parent_run_id IS NULL` 只取根任务（子 scope 内部事实不混入主对话）。返回行数上限 32，超限截断并在结果中注明。
- **返回**（结构化 JSON，逐任务一行）：

```json
{"tasks": [{
  "task_id": "...", "tool": "ssh_exec", "status": "running",
  "phase": "running", "elapsed_ms": 184000,
  "progress": {"tail": "...最近输出尾部...", "bytes_done": 1048576},
  "result": null
}, {
  "task_id": "...", "tool": "local_zsh", "status": "succeeded",
  "phase": null, "elapsed_ms": 631000, "progress": null,
  "result": {"status": "succeeded", "summary": "display_content 截断至 2000 字符",
             "artifact_refs": ["<tool_artifacts.id>"], "observed": false}
}]}
```

  - `status` 三态归并：`in_flight`（planned/running）/ 终态词表原样（succeeded/failed/...）；
  - 在途 → 返回 phase + elapsed + progress（M4 落地前 progress 恒 null，退化为 phase+elapsed）；
  - 已完成未观察 → 返回 `result`（display 截断 + 产物引用；完整正文让模型按需走产物/文件路径，查询工具不搬运大结果）；
  - 已完成已观察 → 默认排除（已进上下文，重复搬运是噪音）。

### 4.2 登记与安全

- `spec.rs::TOOL_POLICY_TABLE` 补一行：`readonly` / `parallel_readonly=true`（PARALLEL_READONLY 预设，1s 派发窗口）/ 统一超时 / 不压缩（结果本身已截断）。fail-closed：未登记名走兜底，本工具已登记故无此问题。
- **零副作用**：只读 DB（`spawn_blocking`）；不触发交付、不写观察标记——真正的「结果入上下文」仍走 deliver/recover 路径，查询工具只回答「事实是什么」。
- 与恢复路径的关系：`recover_completions` 在 run 头已自动交付未观察完成；查询工具面向的是「交付之后新完成的」「还在跑的」「用户中途询问」三类时点。

### 4.3 新增 DB 查询

`db/tool_runs/` 增加 `query_tool_tasks(workspace, task_ids: Option<&[String]>, include_observed: bool)`：runs LEFT JOIN completions（一条 run 至多一行 completion），`observed_request_step IS NULL OR status IN ('planned','running')` 过滤，ORDER BY created_at。单口供查询工具与 M3 移交清单复用。

## 5. M2 wait_for 跨 run 外部目标

### 5.1 语义

`wait_for_tools` 的 `task_ids` 里**不在本调度器 `calls` 中的 id** 不再被当作幻觉丢弃，而是解析为**外部目标**（本 workspace 的根任务）：

- 内部目标（∩ calls）：维持现状——事件驱动（JoinSet 完成即唤醒）；
- 外部目标（余下且在 DB 中存在、未终态）：**轮询驱动**——`wait_for` 的 `select` 增加一个条件轮询分支（间隔 1s，仅当存在外部目标时挂载），查 `query_tool_tasks` 同源的终态视图，发现任一外部目标出现终态 → 经 `pending_root_tool_completions` 吸收进 `ready`（`delivery_message_id` 为 NULL，循环顶部 `deliver` 走 recovered 同款路径交付）→ 计入 `settled` → 条件判定唤醒；
- 外部目标全部已终态且已观察 → 视同 `NoPendingTasks`；外部 id 在 DB 中不存在 → 仍按幻觉过滤（现行为）。

### 5.2 为什么是轮询而不是事件

SQLite 跨连接没有进程内通知可用（russh/wal 的 `data_version` 变更检测本质仍是轮询）；每秒一条索引查询（`dispatcher_tool_completions` 按 tool_run_id 点查）成本可忽略。**只有声明了外部目标才挂载轮询分支**——纯 run 内等待的既有路径保持零轮询、纯事件驱动，语义不变。

### 5.3 超时与取消

`timeout_secs` 对内/外部目标统一计时（一个 deadline）；取消语义不变。外部目标跨 run 存在时，`wait_for` 的注释口径「事件驱动，无轮询」更新为「内部目标事件驱动；跨 run 外部目标 1s 轮询兜底」。

## 6. M3 收口在途移交（信息闭环）

run 以任何方式收口时，若仍有未结算任务，把清单落成一条 runtime 消息（进历史，下一 run 装配可见），并附进错误/收口文案：

- **挂载点**：`loop/decision.rs` 的 `max_iterations` bail、`support.rs::finalize_cancelled`、`scheduler.rs::drain_until` 的「结算未确认」错误。统一经 `scheduler` 新增 helper `handoff_manifest()`（`PendingTask` 列表 + 预计超时提示）生成。
- **落库形态**：runtime 消息 `{"kind":"task_handoff","tasks":[{"task_id","tool","phase","elapsed_ms"}],"hint":"下一轮可用 tool_task_query 查询进度与结果，或 wait_for_tools 指定 task_ids 等待"}`。
- 子智能体 run 的收口不写主对话（内部事实不混入，既有口径）；其未结算任务由父调用（`call_sub_agent` 的结果）报告。

## 7. M4 进度落库（schema v17）

### 7.1 schema 变更（按「基线 + 前向迁移」三件套）

- `dispatcher_tool_runs` 加列 `progress_json TEXT NULL`；
- 基线 DDL 同步加列；`SCHEMA_VERSION` 16 → 17；`init()` 追加 `if current_version < 17` 事务块（纯加列，无回填，幂等）。

### 7.2 写侧

- 新 helper `update_tool_task_progress(task_id, json: &str)`：`UPDATE ... SET progress_json=?1 WHERE id=?2 AND status IN ('planned','running')`（终态后拒绝写，与 phase 同防线）；
- **节流契约由 worker 侧保证**：间隔 ≥1s 且内容变化才写（各工具在已有输出流处顺手裁尾，不新增读取路径）；
- progress 内容宽松契约（宽松是刻意的，各工具自治）：`{"tail": "≤2000 字符的输出尾部", "bytes_done": 1048576, "note": "工具自定义一句话"}`；
- `settle_tool_completion` 事务内 `progress_json = NULL`（终态后进度无意义，防脏读；结果唯一权威 = completion + artifact）；
- **首期接入**：`local_zsh` / `ssh_exec` / `sync_directory`（自管超时三剑客，输出流现成）。`ssh_term_*` 不接（有自己的 read 语义）。其余工具不接也无损——查询工具对无 progress 的在途任务退化为 phase+elapsed。

### 7.3 读侧

- `tool_task_query` 在 `in_flight` 行返回 progress；
- 前端 `ToolRunUpdated` 携带进度尾部（本期范围外，字段已够，另立需求）。

## 8. 闭环验收清单（行为口径，非模块存在）

1. **终态必达**：任何派发过的工具 run，经正常/取消/超时/hand_off/重启路径，`dispatcher_tool_completions` 必有且仅有一行（settle 幂等已保证唯一）；
2. **结果必达上下文**：每个 completion 至少经三路之一进入模型可见面——本 run deliver / 后续 run recover（含 M2 的 run 中途吸收）/ 模型 tool_task_query 读到（`observed_request_step` 落标记为交付准据）；
3. **进度新鲜度**：接入 M4 的工具在 running 期间 progress_json 落后真实输出 ≤ ~1s；
4. **移交可见**：带在途任务收口的 run，其移交 runtime 消息在下一 run 历史装配中出现，且包含完整 task_id 清单；
5. **零副作用**：tool_task_query 只读；跨重启仍不自动重跑任何工具（原则不变）。

## 9. 实施排期

| 阶段 | 内容 | 依赖 | 风险 |
| --- | --- | --- | --- |
| P1 | M1 查询工具 + M3 移交清单 | 无 schema 变更 | 低：纯增量；策略表登记有 `dispatch_window_column` 等测试守护 |
| P2 | M2 wait_for 外部目标轮询 | M1 的任务查询口径 | 中：改 wait_for 主路径，需补「跨 run 等待唤醒」端到端测试（复用 `loop/tests.rs` 的 MockCompletionModel 模式） |
| P3 | M4 progress_json + 三工具接入 | schema v17 迁移 | 低：加列迁移幂等；worker 节流写需防高频 UPDATE（≥1s 契约写入各工具注释） |

## 10. 待确认点

1. **工具命名**：`tool_task_query`（本文案）备选 `query_tool_tasks` / `task_status`——纯命名偏好；
2. **轮询间隔 1s**（M2）：可接受即定稿；若偏好更保守（2s）只改一个常量；
3. **progress 契约宽松度**（M4）：是否需要收紧为强 schema（当前建议宽松 + 各工具自治，YAGNI）；
4. **P1 里 M3 的 hint 文案**是否直接点名工具名（依赖 M1 同期落地，建议 P1 一起出）。
