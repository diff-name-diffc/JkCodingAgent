# DWG Agent 工具集与渐进式披露设计总结

## 1. 背景

这次 DWG 能力建设，目标不是单纯给 Agent 多加几个 CAD 相关工具，而是建立一套真正可用于审查、导航、定位、放大、缩小、拾取和结果沉淀的工程体系。

问题的本质在于，DWG 不是适合一次性灌入模型的文本型数据。它天然具备以下特点：

- 实体数量大，整图全量读取会迅速吞掉上下文窗口。
- 不同阶段需要的数据粒度不同，概览阶段和定位阶段不需要同一批信息。
- viewer 交互是会话态的，Agent 不能只看静态 JSON，还需要知道当前视口、缩放、选择集和激活图纸。
- 审查结果是持续沉淀的，问题清单、运行记录、定位能力必须与 viewer 联动，而不是彼此孤立。

因此，这套设计的核心不是“把 DWG 数据读给模型”，而是“让 Agent 以接近人类审图的方式逐层探索 DWG”。

## 2. 为什么不能整图全量读取

如果让 Agent 默认读取整份 DWG 实体明细，会带来四类问题：

1. 上下文浪费
   - 大量 polyline 顶点、块明细、文字对象会快速挤占模型上下文。
   - 很多细节在早期探索阶段根本不需要。

2. 推理噪音
   - 模型容易在还没缩小范围前就被低价值几何细节淹没。
   - 真正重要的信息通常是图层分布、实体类型占比、局部区域特征，而不是全图顶点数组。

3. 交互失真
   - 静态读取不能替代 viewer 会话状态。
   - “当前用户看到哪里”“当前选中了什么”“现在缩放级别是多少”都属于运行时状态，必须通过 session bridge 感知。

4. 工程不可扩展
   - 一旦默认全量注入，后续再接规则审查、区域分析、定位回放、review run 联动时，会越来越难控制响应大小和性能边界。

所以设计上必须坚持“概览优先、范围收敛、小批量展开”的渐进式披露原则。

## 3. 总体设计目标

这次 DWG Agent 工具集的设计，围绕四个目标展开：

- 让 Agent 先看到结构，再看到局部，最后才看到细节。
- 让 Agent 既能理解 DWG 数据结构，又能操作 DWG viewer。
- 让 UI 点击问题项与 Agent 调用工具的行为保持一致。
- 让索引、viewer、review 三个子域解耦，但能通过稳定桥接协同工作。

最终形成的是四层能力面：

1. 索引层
   - 把 DWG 从“整包 blob”转成可筛选、可分页、可区域查询的结构化索引。

2. 探索层
   - 让 Agent 依次走 `overview -> layers/region -> envelopes -> detail`。

3. 交互层
   - 让 Agent 感知 viewer session，并执行定位、缩放、平移、选择、拾取、截图。

4. 结果层
   - 让审查结果、问题清单、运行记录和 viewer 联动沉淀下来。

## 4. 工具体系概览

当前工具集可以分为 14 个工具，按职责拆成四组。

| 分层 | 工具 | 主要职责 | 默认返回粒度 |
| --- | --- | --- | --- |
| 索引入口 | `cad_ensure_dwg_index` | 确保 DWG 已建索引，必要时触发解析 | 文档级 |
| 顶层概览 | `cad_get_dwg_overview` | 返回范围、图层、类型统计和下一步建议 | 文档级 |
| 顶层概览 | `cad_get_dwg_summary` | 兼容旧工具名，返回摘要 | 文档级 |
| 范围收敛 | `cad_list_dwg_layers` | 按图层分页查看统计，帮助先缩小范围 | 图层级 |
| 范围收敛 | `cad_inspect_dwg_region` | 对指定区域做局部摘要，而不是扫全图 | 区域级 |
| 实体探索 | `cad_query_dwg_entities` | 分页查询实体包络，支持 layer/type/text/bbox 过滤 | Envelope 级 |
| 实体探索 | `cad_get_dwg_entity_detail` | 对少量实体拉取 payload 细节 | Payload 级 |
| Viewer 会话 | `cad_get_dwg_viewer_session` | 获取或绑定可用 viewer session | 会话级 |
| Viewer 状态 | `cad_get_dwg_viewport` | 获取当前 viewport、zoom、selection 和可视样本 | 视口级 |
| Viewer 控制 | `cad_control_dwg_viewer` | 统一执行 fit、zoom、pan、focus、select、mode 切换 | 交互级 |
| Viewer 控制 | `cad_pick_dwg_viewer` | 基于屏幕点或世界点做拾取 | 命中实体级 |
| Viewer 留痕 | `cad_capture_dwg_viewer` | 导出当前视图 PNG 作为 artifact | 图像级 |
| 辅助能力 | `cad_compute_geometry` | 纯几何计算，不耦合 viewer 与索引 | 计算结果 |
| 结果沉淀 | `cad_save_review_result` | 保存审查结果、问题清单、运行记录 | Run/Issue 级 |

### 4.1 索引与概览工具

这一层解决的是“先知道这张图是什么”的问题。

- `cad_ensure_dwg_index`
  - 保证 DWG 已进入可查询状态。
  - 如果索引缺失、陈旧或需要强制刷新，会触发构建。
  - 它不是返回所有实体，而是返回 `docId`、状态、摘要预览，给后续工具提供稳定入口。

- `cad_get_dwg_overview`
  - 返回文件范围、总实体数、未知实体数、图层统计、实体类型统计、块统计、文字样本。
  - 同时返回 `nextSuggestedActions`，显式引导 Agent 继续缩小范围。

- `cad_get_dwg_summary`
  - 兼容旧接口，避免历史调用链失效。
  - 设计上不再承担“实体明细入口”的职责，只保留摘要能力。

### 4.2 范围收敛工具

这一层解决的是“先从哪里看”的问题。

- `cad_list_dwg_layers`
  - 先按图层看整体结构，比直接扫实体更贴近人工审图习惯。
  - 适合定位高密度图层、问题集中图层、文字密集图层。

- `cad_inspect_dwg_region`
  - 针对一个 bbox 或 point+radius 做局部统计。
  - 返回区域内的图层分布、实体类型分布、文字样本和少量实体样本。
  - 这样 Agent 可以围绕当前视口、某个问题区域、某段平移后的窗口做局部探索，而不是每次都回到整图。

### 4.3 实体探索工具

这一层解决的是“局部里具体有什么”的问题。

- `cad_query_dwg_entities`
  - 只返回 `CadEntityEnvelope`，不默认返回 payload。
  - 支持 `layers`、`entityTypes`、`textQuery`、`bbox`、`blockName` 等过滤。
  - 默认分页，避免一次性取出大批对象。

- `cad_get_dwg_entity_detail`
  - 只允许对小批量对象展开。
  - 这里才返回 payload，例如 polyline 顶点、insert 变换、arc 参数、dimension 明细等。
  - 通过显式上限限制，强制 Agent 在调用前已经完成范围收敛。

### 4.4 Viewer 与交互工具

这一层解决的是“看见以后怎么操作”的问题。

- `cad_get_dwg_viewer_session`
  - 为指定 DWG 绑定可用 viewer，会话是显式对象，不再假设前端一定只有一个当前 viewer。

- `cad_get_dwg_viewport`
  - 返回当前 `viewportBox`、`center`、`zoomScale`、`mode`、`selectionIds`。
  - 默认只附带少量可视实体样本，避免把当前窗口所有对象直接塞给模型。

- `cad_control_dwg_viewer`
  - 把定位、缩放、平移、选择、模式切换统一收敛为一条控制总线。
  - 当前支持的动作包括：
    - `fit_drawing`
    - `fit_bbox`
    - `fit_entities`
    - `focus_issue`
    - `fly_to_point`
    - `zoom_by_factor`
    - `pan_by_view_ratio`
    - `select_entities`
    - `clear_selection`
    - `set_mode`

- `cad_pick_dwg_viewer`
  - 让 Agent 能围绕用户当前鼠标点、屏幕点或世界坐标做拾取。
  - 这使得“从画面反查实体”成为可能。

- `cad_capture_dwg_viewer`
  - 主要用于人工核对、留痕和 artifact，不作为结构化探索主路径。

### 4.5 辅助与结果工具

- `cad_compute_geometry`
  - 保持纯函数性质，只做几何计算。
  - 这样它既能被 DWG 使用，也不会把 viewer 或数据库耦合进来。

- `cad_save_review_result`
  - 保存审查 run 和 issue。
  - 扩展支持 `viewportHint`，使得问题可以更稳定地自动聚焦。

## 5. 渐进式披露的探索链路

这套工具最重要的不是工具数量，而是工具之间的调用顺序约束。

推荐链路如下：

```text
cad_ensure_dwg_index
  -> cad_get_dwg_overview
  -> cad_list_dwg_layers / cad_inspect_dwg_region
  -> cad_query_dwg_entities
  -> cad_get_dwg_entity_detail
  -> cad_get_dwg_viewer_session
  -> cad_get_dwg_viewport / cad_control_dwg_viewer / cad_pick_dwg_viewer
  -> cad_save_review_result
```

其中有几个硬约束是设计刻意为之：

- 不允许默认整图全量实体展开。
- 不允许 `cad_query_dwg_entities` 直接返回大 payload。
- 不允许 `cad_get_dwg_entity_detail` 在目标尚未收敛时展开大批量实体。
- 不允许 viewer 工具跳过 session 直接假设前端状态。

这使得 Agent 的行为更像人工审图：

1. 先看图纸整体范围和组成。
2. 再缩小到某些图层或局部区域。
3. 再查看该局部里的候选实体。
4. 只对真正相关的对象读取详细结构。
5. 需要时再驱动 viewer 去飞行、缩放、选择和拾取。

## 6. 数据结构设计

### 6.1 从单体缓存到双层模型

旧模型的问题在于，虽然对外看似支持分页查询，但底层仍把所有实体塞进 `dwg_parse_cache.entity_index_json` 的单个 JSON blob 中。查询时本质上仍是整包反序列化后再内存过滤。

这不符合渐进式披露原则。

因此当前模型升级为双层结构：

- `CadEntityEnvelope`
  - 面向导航和筛选。
  - 保留 `id`、`handle`、`entityType`、`layer`、`bbox`、`center`、`anchor`、`blockName`、`textExcerpt`、`normalizedText`、`layout`、`ownerBlock`、`rotationDeg`、`scaleX`、`scaleY`。

- Payload 层
  - 面向少量对象深查。
  - 保留实体类型特有细节。

这意味着：

- 查询工具先围绕 envelope 工作。
- 只有 detail 工具才触发 payload 读取。

### 6.2 文档、索引、载荷三段式存储

SQLite 侧的核心表如下：

- `dwg_documents`
  - 一条 DWG 一个文档版本。
  - 保存 `project_path + file_path + file_size + file_mtime + parser_version` 对应的摘要。

- `dwg_entity_envelopes`
  - 一条实体一行。
  - 负责分页、过滤、排序和区域查询。

- `dwg_entity_payloads`
  - 存按需展开的类型细节 JSON。

- `dwg_entity_rtree`
  - 用于 bbox 区域查询。

- `dwg_parse_cache`
  - 作为旧缓存兼容层保留，但新体系会优先物化到文档索引表中。

这种设计带来的好处是：

- 文档摘要和实体明细解耦。
- envelope 查询不需要先反序列化整图。
- 局部区域分析可以直接走数据库层过滤。
- viewer 只需要少量视口样本时，不必读大 payload。

## 7. 前后端架构分层

### 7.1 前端分层

前端当前已经把 DWG 工作台拆成三个子域：

- `useDwgIndex`
  - 负责读取文件 bytes、调用 DWG parser worker、读写解析缓存。
  - 对外输出 `summary`、`docId`、`parseStatus`、`bytes`。

- `useDwgViewerSession`
  - 负责打开 viewer、注册 session、同步 viewer 状态、串行执行后端下发的命令。
  - 它还负责监听 `viewChanged`、`selectionSet` 等高频事件，并做防抖同步。

- `useCadReviewRuns`
  - 负责加载 review run 列表和明细。
  - 监听 `cad-review/run-created`，使正在打开的图纸能自动刷新问题清单。

这意味着原本堆在 `DwgWorkbenchPane` 里的索引、viewer、review 逻辑，已经可以独立演化。

### 7.2 后端分层

后端则对应三类职责：

- DWG 索引域
  - 负责摘要、图层、区域、envelope、detail 查询。

- Viewer bridge 域
  - 负责 session 注册、状态快照、命令分发、回执等待、串行执行。

- Review 域
  - 负责审查结果存储、run 列表、issue 详情和事件广播。

内置工具只是这些能力的统一出口，而不直接持有 React 状态。

### 7.3 关键实现入口

为了让这篇总结能直接落回代码，当前关键实现入口如下：

- `src-tauri/src/agent/tools/builtin/mod.rs`
  - 内置工具注册入口。
  - 这里把 DWG 工具和通用 CAD 工具注入到 Agent 工具集合中。

- `src-tauri/src/agent/tools/builtin/dwg.rs`
  - DWG 工具主实现。
  - 包含索引确保、概览、图层、区域、实体、viewer session、viewport、viewer 控制、拾取、截图等工具定义。

- `src-tauri/src/agent/db.rs`
  - DWG 索引存储、旧缓存兼容物化、路径规范化、区域查询、分页过滤的核心实现。

- `src/components/file-viewer/dwg/useDwgIndex.ts`
  - 前端 DWG 解析与索引写入入口。
  - 负责 parser worker、文件 bytes、缓存命中与保存。

- `src/components/file-viewer/dwg/useDwgViewerSession.ts`
  - 前端 viewer session bridge 实现。
  - 负责 session 注册、状态上报、命令监听、串行执行与回执。

- `src/components/file-viewer/dwg/useCadReviewRuns.ts`
  - 审查结果列表与详情联动入口。
  - 负责 run 刷新和 `cad-review/run-created` 事件监听。

## 8. Viewer Session Bridge 的工程思路

Agent 要真正“检查 DWG”，只会查询静态数据还不够，必须能操作 viewer。

所以这里引入了显式的 `DwgViewerSession` 概念：

- 会话 ID 由 `workspaceId + tabId` 派生。
- 前端在 viewer 启动后注册 session。
- 前端持续回传轻量状态快照。
- 后端把命令通过事件总线发给指定 session。
- 前端执行命令后，把结构化结果回传后端。
- 后端等待回执并返回给 Agent。

这个桥的核心价值有三点：

1. 解耦
   - Agent 工具不需要知道 React 组件树。
   - 前后端只通过标准命令和状态快照交互。

2. 可串行
   - 同一 session 的命令按队列串行执行，避免连续 `zoom`、`pan`、`select` 产生竞态。

3. 可观测
   - Agent 能拿到当前视口、缩放、选择集和局部样本，从而判断自己“看到哪里了”。

## 9. 问题定位的一致性设计

这套体系里一个很重要的原则是：人工点击问题项和 Agent 聚焦问题，必须走同一条定位规则。

为此，`focus_issue` 的定位优先级固定为：

```text
entityRefs -> viewportHint -> anchorPoint -> bbox -> noop
```

这样设计的好处是：

- 如果 issue 已经指向具体实体，优先直接框选并适配实体。
- 如果有 `viewportHint`，可以恢复更接近人工审查时的视角。
- 如果没有实体引用，还能退化到 anchor 或 bbox。
- 最后才是无法聚焦时的 no-op。

这避免了“问题卡片点击是一套行为，Agent 调工具又是另一套行为”的分叉。

## 10. 实际踩坑与修复经验

这次实现里，最关键的经验不是“工具调用超时后把超时时间调大”，而是识别出超时背后的真实原因。

### 10.1 超时并不只是性能问题

`cad_ensure_dwg_index` 一度持续报“等待 DWG 索引构建超时”，但真正原因并不只是解析慢，而是索引链路存在兼容断层：

- 前端已经能打开图纸并拿到旧版 `dwg_parse_cache`。
- 后端新工具链却在等 `dwg_documents` / `dwg_entity_envelopes` 这套新索引。
- 两者之间缺了一层自愈式物化。

结果就是：

- 图纸能打开。
- Agent 却认为索引还没准备好。
- 最终表现为一直等待直到超时。

### 10.2 真正有效的修复不是“再等一会儿”

最终有效的修复有三类：

- 路径规范化
  - 统一 DWG 存储键，避免同一路径因表示形式不同而匹配不到缓存。

- 旧缓存自动物化
  - 命中 `dwg_parse_cache` 时，自动补建 `dwg_documents`、`dwg_entity_envelopes`、`dwg_entity_payloads`、`dwg_entity_rtree`。

- Viewer 与索引解耦
  - viewer 能否打开和索引是否可查不再互相阻塞，但两者可以通过 `docId` 和 session state 联动。

这个经验非常重要：DWG 体系里最危险的不是单点错误，而是“前端看似已经成功，后端却卡在另一套状态机里”的隐性断层。

## 11. 推荐的典型调用链

### 11.1 结构化探索

适合 Agent 初次检查一张 DWG：

1. `cad_ensure_dwg_index`
2. `cad_get_dwg_overview`
3. `cad_list_dwg_layers`
4. `cad_query_dwg_entities`
5. `cad_get_dwg_entity_detail`

### 11.2 结合当前 viewer 继续深查

适合图纸已经打开，Agent 需要围绕当前视口继续检查：

1. `cad_get_dwg_viewer_session`
2. `cad_get_dwg_viewport`
3. `cad_inspect_dwg_region`
4. `cad_control_dwg_viewer`
5. `cad_pick_dwg_viewer`

### 11.3 审查结果沉淀

适合完成规则检查后输出结果：

1. `cad_save_review_result`
2. 前端监听 `cad-review/run-created`
3. review run 列表刷新
4. 点击 issue 或调用 `focus_issue` 进行定位回看

## 12. 这套设计的核心价值

从工程角度看，这套 DWG Agent 工具体系真正解决的是三个长期问题。

### 12.1 控制模型上下文

通过 `overview -> layer/region -> envelope -> detail` 的层级设计，把上下文预算优先花在“决策价值最高”的信息上，而不是机械堆数据。

### 12.2 让 Agent 具备操作能力

Agent 不再只是一个“能查数据库的阅读器”，而是一个能感知 viewer、能控制视口、能定位问题、能与人工视角对齐的审图执行体。

### 12.3 为后续规则审查扩展打基础

当索引层、viewer bridge、review 域稳定之后，后续无论是：

- 自动规则审查
- 特定构件识别
- 面向区域的规则 DSL
- 人机协同的复核工作流

都可以建立在这套分层之上，而不需要重新推倒 DWG 基础设施。

## 13. 一句话总结

这次 DWG Agent 能力建设的关键，不是把 DWG 数据“喂给模型”，而是把 DWG 变成一个可以被 Agent 逐层探索、按需展开、可操作 viewer、可沉淀审查结果的工程化系统。

真正有效的设计关键词只有两个：

- 渐进式披露
- 结构化交互

前者保证模型不会被大图纸压垮，后者保证 Agent 不是只会读数据，而是真的能检查 DWG。
