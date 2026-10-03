//! 编排器提示词（rig 形态，迁移自旧 `agents/project/prompt.rs`）。
//!
//! 静态提示词 = 角色提示（含图 schema 版本占位符）+ 用户偏好（USER.md）+
//! 记忆 + 技能；运行期再追加 Harness 目录（含节点运行统计）与系统时间。
//! 读取规则（越界符号链接跳过、单文件 64KiB 上限、失败留痕）逐条保留。

use std::path::Path;

use anyhow::Result;

/// 提示词加载告警的持久化出口：打包后 stderr 不落盘，编排器的提示词/学习
/// 回路静默失效必须有可诊断痕迹（迁移自旧 `agents/project/helpers.rs`）。
const MAX_WARNING_LOG_BYTES: u64 = 1024 * 1024;

pub(crate) fn log_warning(message: &str) {
    eprintln!("{message}");
    let Some(log_path) = dirs::home_dir().map(|home| {
        home.join(".jkcodingagent")
            .join("logs")
            .join("orchestrator.log")
    }) else {
        return;
    };
    let entry = format!("{} {message}\n", chrono::Utc::now().to_rfc3339());
    let write = move || {
        if let Some(parent) = log_path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return;
            }
        }
        // 简易滚动：超限后重命名为 .old 再重建，避免日志无限增长。
        if let Ok(meta) = std::fs::metadata(&log_path) {
            if meta.len() > MAX_WARNING_LOG_BYTES {
                let _ = std::fs::rename(&log_path, log_path.with_extension("log.old"));
            }
        }
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            let _ = file.write_all(entry.as_bytes());
        }
    };
    // 同步写：调用点都已在 spawn_blocking 或提示词构建（一次性）路径上。
    write();
}

const ORCHESTRATOR_ROLE_PROMPT: &str = r#"# 项目编排 Agent

你是桌面客户端中的项目编排 Agent。你本身不写代码、不执行命令；你的核心职责是：
完整理解用户需求 → 用受限工具运行时探索项目（证据优先）→ 把复杂任务拆解为一张「执行图」（DAG），交给专业的执行 Agent 完成 → 依据执行报告持续修正，直到任务达成。

## 工作方式判定

- 简单问题（问答、解释、小范围咨询、无需动代码）：直接用 message 工具答复用户，不要出图。
- 复杂任务（多步骤、多角色协作、跨模块改动）：先探索、再出图。出图前不要向用户输出长篇计划说明，直接用 submit_graph 提交即可，系统会把图呈现给用户确认。

## 可用工具

- `run_tool_program`：只读探索的唯一入口。一次调查写一个程序。可调用的名字和字段只看本轮「数据面 SDK」。
- `message`：向用户发送最终答复（简单问题的收口方式）。
- `submit_graph`：提交执行图（复杂任务的收口方式）。每轮最多提交一次；提交后等待用户确认，不要重复提交。
- `graph_plan_report`：读取最近一次执行图的运行报告（验收结论、各节点成败与输出摘要、失败原因、共享 state 键）。
- `graph_result_read`：读取最近一次执行图的执行结果：结果类型、验收结论、完整结论文本（审查类=问题清单，编辑类=执行总结）与修改文件清单。图执行完成后先用它拿完整结论，再决定答复用户或规划后续图；需要节点级成败与失败原因时配合 graph_plan_report。
- `graph_get`：读取执行图的完整定义与状态（节点任务、依赖、state 键、最近运行摘要）。上下文被压缩后图细节可能丢失，需要时先重新感知。
- `graph_node_update`：定点修正待确认（draft）图中单个节点的字段（提供哪个改哪个，含 dependsOn 即改边）。
- `graph_node_add`：向 draft 图新增节点（完整定义）；insertBefore 可把指定节点指向本节点上游的边改写为本节点，实现中间插入。
- `graph_node_delete`：删除 draft 图节点；被下游依赖时默认拒绝并列出传递下游，force=true 级联删除全部下游。

## 执行图 schema

```json
{
  "version": "{graph_definition_version}",
  "title": "图标题",
  "summary": "一句话编排思路",
  "inheritsFrom": { "planId": "修复时继承的图计划 id", "runId": "继承的运行 id" },
  "stateKeys": [{ "key": "snake_case 键名", "description": "用途说明" }],
  "nodes": [{
    "id": "n1",
    "title": "节点标题",
    "role": "该节点 Agent 的角色定位",
    "modelRef": "<当前模型目录中的稳定 id>",
    "baseToolGroup": "read_only 或 coding",
    "task": "自包含的子任务说明",
    "dependsOn": ["上游节点 id"],
    "injectStateKeys": ["需要注入的 state key"],
    "outputKey": "本节点输出写回 state 的 key",
    "expectedFiles": ["预期读写的文件路径（coding 节点建议填写）"],
    "exportPolicy": "summary 或 full",
    "usePlanMode": "可选，默认 false；复杂改造类节点置 true 先计划后执行"
  }]
}
```

- 每个节点只使用一个主模型。模型与基础工具组必须来自本轮 Harness 目录：模型是 Claude Agent 的选型（default 继承登录态默认模型）。所有节点默认以 bypassPermissions 全权限模式运行（子智能体自主执行，客户端的全局权限审查 AI 只把关仍浮出的少数请求）；`usePlanMode: true` 的节点以 plan 模式启动（先产出计划，计划完成后系统自动批准并切回 bypassPermissions）；`read_only` 是只读纪律（系统在其输入中注入「不得写文件/执行副作用命令」约束），与权限模式正交。子智能体执行中自发进入计划模式也是允许的。
- 边由 `dependsOn` 派生，必须构成无环图；`dependsOn` 引用的节点必须存在；`id`、`outputKey` 全局唯一；节点数 ≤ {max_graph_nodes}。
- 节点完成后 `state[outputKey] = 节点输出的「产出摘要」段`（≤4k，全文保留在节点运行记录中）；下游节点通过 `dependsOn` 收到上游输出、通过 `injectStateKeys` 收到指定 state 值。共享 state 只承载结论摘要：确需上游完整产出时用 `dependsOn` + `exportPolicy=full`，不要靠 injectStateKeys 拉全文。
- 节点输入由系统装配：总体需求 + 角色 + 子任务 + 上游输出 + 注入的 state 节选。节点拿不到聊天记录，因此 `task` 必须自包含（目标、背景、相关文件/符号、约束、验证方式、期望产出）。
- `exportPolicy` 控制本节点输出对下游的可见范围：默认 `summary` 只向下游传递「产出摘要」段，深链条更省上下文；确需下游拿到完整产出时用 `full`。
- `inheritsFrom` 仅在修复/续作场景使用：引用会话内已结束的图计划与某次运行，系统会把该运行的共享 state 种入新图，供 `injectStateKeys` 引用。不要凭空填写。

## 节点设计原则

- 单一职责：一个节点只做一件事，调研 / 改造 / 验证分开。
- 上下文最小化 + 显式数据流：先由调研节点产出结论（outputKey），改造节点 inject 该结论后再动手。
- **验证节点强制**：只要图中有 coding（修改）节点，就必须至少有一个 read_only 验证节点依赖其产出（读取改动、运行测试、核对结果），作为收尾。
- **汇总节点收口**：图应以单一「汇总节点」收尾（dependsOn 各分支末端、自身无下游），它是整图的最终输出出口。汇总节点用 `read_only`，其输出即执行结果结论文本 markdown——审查/调研类图写完整问题清单（位置、严重度、建议）；编辑类图写执行总结并**必须包含「修改文件清单」章节**（列出本次全部修改文件及一句话说明）。多分支不收口时系统只能按启发式猜测结论节点，执行结果展示会退化为无结论。
- **并行写冲突**：互不依赖、可能并行的两个 coding 节点不得修改同一文件；若 `expectedFiles` 相交，请用 `dependsOn` 串行化。coding 节点请如实填写 `expectedFiles` 以便系统预检。
- 根据任务性质选择主模型（含历史成功率参考）；只读任务优先 `read_only`，确需修改或命令时使用 `coding`。
- Harness Engineering：基础工具保持最小，只读任务用 `read_only`（只读纪律 + 审查把关），确需修改或命令时用 `coding`。复杂改造类 coding 节点（多文件联动、影响面大、方案有分歧）建议 `usePlanMode: true` 让子智能体先计划再执行；简单任务保持默认即可，不要滥用。
- 无依赖关系的节点会并行执行（最多 {max_parallel_nodes} 个并发）；可并行的子任务请拆成平行节点。
- 禁止引用 Harness 目录之外的模型；不要生成 subAgent、Claude CLI 或 Codex CLI 节点。

## 修复与迭代纪律

- 出图被执行、用户回报结果或上一轮图失败后，若需要继续处理：先用 `graph_result_read` 读取执行结果（审查类=完整问题清单、编辑类=执行总结与修改清单）；需要节点级成败、失败原因与共享 state 时再用 `graph_plan_report`。
- 审查/调研图产出的结论是后续行动的输入：据问题清单规划修复图（coding 节点逐项修复，验证节点复核），据执行总结决定是否需要补充图或直接 `message` 收口。
- 基于报告做**最小修复**：提交新图时用 `inheritsFrom` 继承上次运行的共享 state，只新增/重做失败与缺失的部分，成功节点的成果通过 `injectStateKeys` 复用，不要整图重做。
- 若报告表明任务已完成或无法推进，用 `message` 如实答复用户。
- 上下文过长被压缩后，图的 plan_id 与节点细节可能丢失：继续处理图相关任务前先用 `graph_get` 重新感知最新定义与状态，不要凭记忆改图。
- 用户确认前对 draft 图的节点级调整（改属性、增删节点、中间插入步骤）一律用节点工具完成（update / add / delete），不要为局部调整重新提交整图；删除被依赖的节点前先评估下游，确需连带清理才用 force 级联。

## 探索纪律

一次调查只写一个 `run_tool_program`。不要直接调用 `read_file` / `list_dir` / `glob` / `grep`，也不要把写入、命令、浏览器或协议工具写进程序。用 `await tools.name({ ... })` 组合调用；互不依赖的只读调用放进 `Promise.all`。单次调用失败抛 `ToolCallError`，可以 `try/catch` 后继续。只有 `return` 的值和 `console.log` / `console.error` 回到上下文。字段名以「数据面 SDK」为准。

## 输出语言

默认简体中文，面向有经验的开发者，结论直接清晰。
"#;

/// 静态提示词：角色 + 用户偏好（USER.md）+ 记忆 + 技能。
/// 每轮构建一次（文件读取走 spawn_blocking）。
pub(crate) async fn build_static_prompt(root_dir: &Path) -> Result<String> {
    let root = root_dir.to_path_buf();
    let extra = tokio::task::spawn_blocking(move || load_prompt_files(&root))
        .await
        .map_err(|error| anyhow::anyhow!("读取编排器提示词文件失败：{error}"))?;

    // 版本/节点数/并发数上限占位符由常量生成：提示词示例、工具 schema、校验与
    // 调度实现同源，契约升级时不再需要手工同步提示词里的示例值。
    let mut prompt = ORCHESTRATOR_ROLE_PROMPT
        .replace(
            "\"version\": \"{graph_definition_version}\"",
            &format!(
                "\"version\": {}",
                crate::agent::graph::types::GRAPH_DEFINITION_VERSION
            ),
        )
        .replace(
            "{max_graph_nodes}",
            &crate::agent::graph::validate::MAX_GRAPH_NODES.to_string(),
        )
        .replace(
            "{max_parallel_nodes}",
            &crate::agent::graph::scheduler::MAX_PARALLEL_NODES.to_string(),
        );
    if !extra.is_empty() {
        prompt.push_str("\n\n---\n\n");
        prompt.push_str(&extra);
    }
    Ok(prompt)
}

/// 每次迭代重建的完整系统提示：静态内容 + 动态分片（可用工具块 + 系统时间）。
/// 静态部分按引用直接装配进最终 format!，避免逐迭代对 static_content
/// 做大块中间克隆（最终 String 是唯一分配点，审查项 G8-21）。
pub(crate) fn build_iteration_system_prompt(
    static_content: &str,
    tool_definitions: &[rig::completion::ToolDefinition],
    data_plane_sdk: &str,
) -> String {
    let tools_block = render_available_tools_block(tool_definitions);
    let local_time = crate::agent::prompt::current_local_time();
    let mut prompt = static_content.to_string();
    if !tools_block.is_empty() {
        prompt.push_str("\n\n---\n\n");
        prompt.push_str(&tools_block);
    }
    if !data_plane_sdk.is_empty() {
        prompt.push_str("\n\n---\n\n");
        prompt.push_str(data_plane_sdk);
    }
    prompt.push_str("\n\n---\n\n# 系统时间\n\n当前本地时间：");
    prompt.push_str(&local_time);
    prompt
}

/// 渲染 Harness 目录（图节点模型表）+ 既往运行统计注记。
pub(crate) fn render_graph_harness_catalog(
    catalog: &crate::agent::graph::types::GraphHarnessCatalog,
    stats: &[crate::agent::graph::types::GraphModelStat],
) -> String {
    let mut lines = vec![
            "# 当前 Harness 目录".to_string(),
            "图节点由 Claude Agent（claude-agent-acp）执行。该目录是 graph v4 的唯一模型来源；ID 必须原样引用。模型行末的历史统计（若有）来自既往节点运行，可作为选型参考。".to_string(),
            "\n## 主模型（每节点恰好一个）".to_string(),
        ];
    for model in &catalog.models {
        let stat_note = render_model_stat_note(&model.id, stats);
        lines.push(format!(
            "- `{}`：{} — {}{}",
            model.id, model.label, model.model, stat_note
        ));
    }
    lines.push("\n## 基础工具组（节点的执行纪律，与权限模式正交）".to_string());
    lines.push(
        "- `read_only`: 只读纪律——不得创建、修改、删除文件或执行有副作用的命令，产出分析/审查结论；纪律经节点输入的「运行约束」软提示承载，会话权限模式不受影响"
            .to_string(),
    );
    lines.push("- `coding`: 编码执行——允许文件编辑与副作用操作。".to_string());
    lines.push(
        "权限模式由节点级 usePlanMode 决定（true=plan 先计划，缺省/false=bypassPermissions 直接执行），与基础工具组无关。"
            .to_string(),
    );
    if !catalog.diagnostics.is_empty() {
        lines.push(format!(
            "\n## 发现诊断\n- {}",
            catalog.diagnostics.join("\n- ")
        ));
    }
    lines.join("\n")
}

/// 模型历史统计注记（轻量学习回路）：聚合同一 model_ref 跨工具组的运行记录，
/// 给出成功率提示；无历史数据时返回空串。
fn render_model_stat_note(
    model_id: &str,
    stats: &[crate::agent::graph::types::GraphModelStat],
) -> String {
    let mut runs = 0i64;
    let mut failures = 0i64;
    for stat in stats {
        if stat.model_ref == model_id {
            runs += stat.runs;
            failures += stat.failures;
        }
    }
    if runs == 0 {
        return String::new();
    }
    let success = runs - failures;
    format!(" ｜ 历史 {runs} 次节点运行 / 成功 {success}")
}

fn render_available_tools_block(tool_definitions: &[rig::completion::ToolDefinition]) -> String {
    if tool_definitions.is_empty() {
        return String::new();
    }

    let mut lines = vec![
        "# 当前实际可用工具".to_string(),
        "以下列表来自本轮运行时实际注入的工具定义，是当前可调用工具的唯一准确信息源。".to_string(),
    ];

    let mut tools = tool_definitions
        .iter()
        .map(|tool| (tool.name.clone(), tool.description.trim().to_string()))
        .collect::<Vec<_>>();
    tools.sort_by(|left, right| left.0.cmp(&right.0));

    for (name, description) in tools {
        lines.push(format!("- `{name}`：{description}"));
    }

    lines.join("\n")
}

/// 单个提示词文件（USER.md / SKILL.md / MEMORY.md）的大小上限：
/// 这些内容会拼入系统提示并在每次迭代整体发送给 LLM，超大文件必须跳过告警，
/// 防止撑爆上下文窗口与失控的 token 费用（审查项 G8-20）。
const MAX_PROMPT_FILE_BYTES: u64 = 64 * 1024;

/// 读取用户级提示词文件（USER.md / 记忆 / 技能）。
/// 刻意不读 SOUL.md：其内容是旧调度 Agent 的角色设定，与编排器角色冲突。
///
/// 安全与健壮性契约（审查项 G8-19/G8-20）：
/// - 所有文件 canonicalize 后必须仍位于 root 内——指向工作区外的符号链接
///   会把外部敏感内容注入系统提示并随请求发给（可能是云端的）LLM，一律跳过；
/// - 超过大小上限的文件跳过并告警；
/// - 任一文件读取失败（非 UTF-8 / 权限 / 遍历中消失）只跳过该文件并留下
///   持久化警告，整体构建不因单文件失败而报错，保证编排器可启动。
fn load_prompt_files(root: &Path) -> String {
    let root_canonical = match root.canonicalize() {
        Ok(path) => path,
        Err(error) => {
            log_warning(&format!(
                "[prompt] 解析提示词根目录 {} 失败，跳过用户级提示词：{error}",
                root.display()
            ));
            return String::new();
        }
    };
    let mut sections: Vec<String> = Vec::new();

    let user = root.join("USER.md");
    if let Some(content) = read_prompt_file(&root_canonical, &user) {
        sections.push(content);
    }

    let skills_dir = root.join("skills");
    if skills_dir.exists() {
        let mut skill_parts = Vec::new();
        match std::fs::read_dir(&skills_dir) {
            Ok(entries) => {
                for entry in entries {
                    let entry = match entry {
                        Ok(entry) => entry,
                        Err(error) => {
                            log_warning(&format!(
                                "[prompt] 读取 {} 的目录条目失败，跳过该条目：{error}",
                                skills_dir.display()
                            ));
                            continue;
                        }
                    };
                    // 技能条目本身可能是越界符号链接：read_prompt_file 会对
                    // SKILL.md 的真实路径做 root 包含校验，越界即跳过。
                    let skill_md = entry.path().join("SKILL.md");
                    let Some(content) = read_prompt_file(&root_canonical, &skill_md) else {
                        continue;
                    };
                    skill_parts.push(format!(
                        "### 技能：{}\n\n{}",
                        entry.file_name().to_string_lossy(),
                        content
                    ));
                }
            }
            Err(error) => {
                log_warning(&format!(
                    "[prompt] 读取 {} 失败，跳过技能加载：{error}",
                    skills_dir.display()
                ));
            }
        }
        skill_parts.sort();
        if !skill_parts.is_empty() {
            sections.push(format!("# 已启用技能\n\n{}", skill_parts.join("\n\n")));
        }
    }

    let memory = root.join("memory").join("MEMORY.md");
    if let Some(content) = read_prompt_file(&root_canonical, &memory) {
        sections.push(format!("# 记忆\n\n{content}"));
    }

    sections
        .into_iter()
        .map(|section| section.trim().to_string())
        .filter(|section| !section.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n---\n\n")
}

/// 读取单个提示词文件：canonicalize 后必须仍位于 root_canonical 内（符号链接
/// 越界跳过）、超过大小上限跳过、读取失败跳过——三种跳过均留持久化警告，
/// 返回 None。文件不存在静默返回 None（用户未创建属正常状态）。
fn read_prompt_file(root_canonical: &Path, path: &Path) -> Option<String> {
    if !path.exists() {
        return None;
    }
    let canonical = match path.canonicalize() {
        Ok(path) => path,
        Err(error) => {
            log_warning(&format!(
                "[prompt] 解析 {} 失败，已跳过：{error}",
                path.display()
            ));
            return None;
        }
    };
    if !canonical.starts_with(root_canonical) {
        log_warning(&format!(
            "[prompt] {} 解析后指向 {} ，超出根目录 {}，疑似符号链接越界，已跳过",
            path.display(),
            canonical.display(),
            root_canonical.display()
        ));
        return None;
    }
    match std::fs::metadata(&canonical) {
        Ok(meta) if meta.len() > MAX_PROMPT_FILE_BYTES => {
            log_warning(&format!(
                "[prompt] 跳过超大提示词文件 {}（{} 字节，上限 {} 字节）",
                canonical.display(),
                meta.len(),
                MAX_PROMPT_FILE_BYTES
            ));
            return None;
        }
        Ok(_) => {}
        Err(error) => {
            log_warning(&format!(
                "[prompt] 读取 {} 元信息失败，已跳过：{error}",
                canonical.display()
            ));
            return None;
        }
    }
    match std::fs::read_to_string(&canonical) {
        Ok(content) => Some(content),
        Err(error) => {
            log_warning(&format!(
                "[prompt] 读取 {} 失败（非 UTF-8 或权限不足等），已跳过：{error}",
                canonical.display()
            ));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{load_prompt_files, MAX_PROMPT_FILE_BYTES, ORCHESTRATOR_ROLE_PROMPT};

    #[test]
    fn exploration_prompt_states_the_program_contract() {
        assert!(ORCHESTRATOR_ROLE_PROMPT.contains("数据面 SDK"));
        assert!(ORCHESTRATOR_ROLE_PROMPT.contains("Promise.all"));
        assert!(ORCHESTRATOR_ROLE_PROMPT.contains("ToolCallError"));
        assert!(!ORCHESTRATOR_ROLE_PROMPT.contains("$ref"));
        assert!(!ORCHESTRATOR_ROLE_PROMPT.contains("\"op\":\"sequence\""));
        assert!(!ORCHESTRATOR_ROLE_PROMPT.contains("/data/files"));
        assert!(!ORCHESTRATOR_ROLE_PROMPT.contains("32000"));
    }

    #[test]
    fn iteration_prompt_places_the_sdk_between_tools_and_time() {
        let prompt = super::build_iteration_system_prompt(
            "ROLE",
            &[rig::completion::ToolDefinition {
                name: "run_tool_program".to_string(),
                description: "探索".to_string(),
                parameters: serde_json::json!({ "type": "object" }),
            }],
            "# 数据面 SDK\npaths: string[]\n32000",
        );
        let tools = prompt.find("当前实际可用工具").expect("tools");
        let sdk = prompt.find("# 数据面 SDK").expect("sdk");
        let time = prompt.find("# 系统时间").expect("time");
        assert!(tools < sdk && sdk < time);
        assert!(prompt.contains("paths: string[]"));
        assert!(prompt.contains("32000"));
        assert!(!prompt.contains("$ref"));
    }

    use std::path::PathBuf;

    fn unique_temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "jk-prompt-test-{tag}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn cleanup(dir: &PathBuf) {
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn loads_user_skills_memory_sections() {
        let root = unique_temp_dir("basic");
        std::fs::write(root.join("USER.md"), "用户偏好内容").unwrap();
        std::fs::create_dir_all(root.join("skills").join("alpha")).unwrap();
        std::fs::write(
            root.join("skills").join("alpha").join("SKILL.md"),
            "技能内容",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("memory")).unwrap();
        std::fs::write(root.join("memory").join("MEMORY.md"), "记忆内容").unwrap();

        let prompt = load_prompt_files(&root);
        assert!(prompt.contains("用户偏好内容"));
        assert!(prompt.contains("# 已启用技能"));
        assert!(prompt.contains("技能：alpha"));
        assert!(prompt.contains("技能内容"));
        assert!(prompt.contains("# 记忆"));
        assert!(prompt.contains("记忆内容"));
        cleanup(&root);
    }

    #[test]
    fn skips_oversized_file_with_warning() {
        let root = unique_temp_dir("oversized");
        let big = "x".repeat((MAX_PROMPT_FILE_BYTES + 1) as usize);
        std::fs::write(root.join("USER.md"), big).unwrap();

        let prompt = load_prompt_files(&root);
        assert!(prompt.is_empty(), "超大 USER.md 应被跳过");
        cleanup(&root);
    }

    #[test]
    fn skips_invalid_utf8_file_without_failing() {
        let root = unique_temp_dir("bad-utf8");
        std::fs::write(root.join("USER.md"), [0xffu8, 0xfe, 0x00, 0x80]).unwrap();
        std::fs::create_dir_all(root.join("memory")).unwrap();
        std::fs::write(root.join("memory").join("MEMORY.md"), "记忆内容").unwrap();

        let prompt = load_prompt_files(&root);
        assert!(prompt.contains("# 记忆"), "单文件损坏不应阻断其余提示词");
        assert!(prompt.contains("记忆内容"));
        cleanup(&root);
    }

    #[cfg(unix)]
    #[test]
    fn skips_symlink_escaping_root() {
        let root = unique_temp_dir("symlink-root");
        let outside = unique_temp_dir("symlink-outside");
        let secret = outside.join("SECRET.md");
        std::fs::write(&secret, "工作区外的敏感内容").unwrap();

        // skills/evil/SKILL.md -> 工作区外文件：必须被跳过，不得注入提示词。
        std::fs::create_dir_all(root.join("skills").join("evil")).unwrap();
        std::os::unix::fs::symlink(&secret, root.join("skills").join("evil").join("SKILL.md"))
            .unwrap();

        let prompt = load_prompt_files(&root);
        assert!(
            !prompt.contains("工作区外的敏感内容"),
            "符号链接越界文件不得进入提示词"
        );
        cleanup(&root);
        cleanup(&outside);
    }

    #[cfg(unix)]
    #[test]
    fn skips_user_md_symlink_escaping_root() {
        let root = unique_temp_dir("symlink-user-root");
        let outside = unique_temp_dir("symlink-user-outside");
        let secret = outside.join("SECRET.md");
        std::fs::write(&secret, "外部用户档案").unwrap();
        std::os::unix::fs::symlink(&secret, root.join("USER.md")).unwrap();

        let prompt = load_prompt_files(&root);
        assert!(!prompt.contains("外部用户档案"));
        cleanup(&root);
        cleanup(&outside);
    }

    #[test]
    fn missing_root_returns_empty() {
        let root = std::env::temp_dir().join(format!("jk-prompt-missing-{}", uuid::Uuid::new_v4()));
        assert!(load_prompt_files(&root).is_empty());
    }
}
