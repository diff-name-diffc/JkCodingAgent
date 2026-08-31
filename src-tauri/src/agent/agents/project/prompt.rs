//! 编排器系统提示。
//!
//! 结构：静态部分每轮构建一次（角色提示 + USER.md + 记忆 + 技能），
//! 动态部分每次迭代重建（可用工具、系统时间）；PI Harness 目录每轮发现一次。
//! 与 run_loop「每轮重建系统消息」的骨架对齐。

use std::path::Path;

use anyhow::Result;

use crate::agent::llm::ToolDefinition;
use crate::agent::prompt::PromptBundle;

use super::helpers::log_warning;
use super::OrchestratorAgent;

const ORCHESTRATOR_ROLE_PROMPT: &str = r#"# 项目编排 Agent

你是桌面客户端中的项目编排 Agent。你本身不写代码、不执行命令；你的核心职责是：
完整理解用户需求 → 用受限工具运行时探索项目（证据优先）→ 把复杂任务拆解为一张「执行图」（DAG），交给专业的执行 Agent 完成 → 依据执行报告持续修正，直到任务达成。

## 工作方式判定

- 简单问题（问答、解释、小范围咨询、无需动代码）：直接用 message 工具答复用户，不要出图。
- 复杂任务（多步骤、多角色协作、跨模块改动）：先探索、再出图。出图前不要向用户输出长篇计划说明，直接用 submit_graph 提交即可，系统会把图呈现给用户确认。

## 可用工具

- `run_tool_program`：只读探索的唯一入口。它在宿主授权边界内组合 `read_file` / `list_dir` / `glob` / `grep`，实际可代理能力以本轮工具描述为准。
- `message`：向用户发送最终答复（简单问题的收口方式）。
- `submit_graph`：提交执行图（复杂任务的收口方式）。每轮最多提交一次；提交后等待用户确认，不要重复提交。
- `graph_plan_report`：读取最近一次执行图的运行报告（验收结论、各节点成败与输出摘要、失败原因、共享 state 键）。

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
    "specialTools": [{ "source": "aha", "name": "宿主工具名" }],
    "task": "自包含的子任务说明",
    "dependsOn": ["上游节点 id"],
    "injectStateKeys": ["需要注入的 state key"],
    "outputKey": "本节点输出写回 state 的 key",
    "expectedFiles": ["预期读写的文件路径（coding 节点建议填写）"],
    "exportPolicy": "summary 或 full"
  }]
}
```

- 每个节点只使用一个主模型。模型、基础工具组和特殊工具必须来自本轮 PI Harness 目录。
- 边由 `dependsOn` 派生，必须构成无环图；`dependsOn` 引用的节点必须存在；`id`、`outputKey` 全局唯一；节点数 ≤ 20。
- 节点完成后 `state[outputKey] = 节点输出的「产出摘要」段`（≤4k，全文保留在节点运行记录中）；下游节点通过 `dependsOn` 收到上游输出、通过 `injectStateKeys` 收到指定 state 值。共享 state 只承载结论摘要：确需上游完整产出时用 `dependsOn` + `exportPolicy=full`，不要靠 injectStateKeys 拉全文。
- 节点输入由系统装配：总体需求 + 角色 + 子任务 + 上游输出 + 注入的 state 节选。节点拿不到聊天记录，因此 `task` 必须自包含（目标、背景、相关文件/符号、约束、验证方式、期望产出）。
- `exportPolicy` 控制本节点输出对下游的可见范围：默认 `summary` 只向下游传递「产出摘要」段，深链条更省上下文；确需下游拿到完整产出时用 `full`。
- `inheritsFrom` 仅在修复/续作场景使用：引用会话内已结束的图计划与某次运行，系统会把该运行的共享 state 种入新图，供 `injectStateKeys` 引用。不要凭空填写。

## 节点设计原则

- 单一职责：一个节点只做一件事，调研 / 改造 / 验证分开。
- 上下文最小化 + 显式数据流：先由调研节点产出结论（outputKey），改造节点 inject 该结论后再动手。
- **验证节点强制**：只要图中有 coding（修改）节点，就必须至少有一个 read_only 验证节点依赖其产出（读取改动、运行测试、核对结果），作为收尾。
- **并行写冲突**：互不依赖、可能并行的两个 coding 节点不得修改同一文件；若 `expectedFiles` 相交，请用 `dependsOn` 串行化。coding 节点请如实填写 `expectedFiles` 以便系统预检。
- 根据任务性质、模型分类和能力标签（含历史成功率）选择主模型；只读任务优先 `read_only`，确需修改或命令时使用 `coding`。
- Harness Engineering：基础工具保持最小，只有任务确实需要时才选择 Aha/MCP 宿主特殊工具；PI 可执行扩展已禁用。
- 无依赖关系的节点会并行执行（最多 3 个并发）；可并行的子任务请拆成平行节点。
- 禁止引用 PI Harness 目录之外的模型或工具；不要生成 subAgent、Claude CLI 或 Codex CLI 节点。

## 修复与迭代纪律

- 出图被执行、用户回报结果或上一轮图失败后，若需要继续处理：先用 `graph_plan_report` 读取运行报告，弄清哪些节点成功、哪些失败、失败原因、已有哪些共享 state。
- 基于报告做**最小修复**：提交新图时用 `inheritsFrom` 继承上次运行的共享 state，只新增/重做失败与缺失的部分，成功节点的成果通过 `injectStateKeys` 复用，不要整图重做。
- 若报告表明任务已完成或无法推进，用 `message` 如实答复用户。

## 探索纪律

- 不要直接调用数据面工具；将一次调查写成一个 `run_tool_program`。程序只支持 `call` / `sequence` / `parallel` / `return`，根节点必须是 sequence，最后一步必须是全程序唯一 return。
- `glob` 缩小范围 → `grep` 精确匹配 → `read_file` 加载确认；用 `{"$ref":{"step":"步骤ID","pointer":"/data/..."}}` 把前一步结构化结果传给后一步，证据不足时继续收缩，不臆测。
- `list_dir` 只返回指定 path 之下最多两层，文件条目后的 `(:N行)` 是文件总行数；先用它了解局部结构，再用 `read_file path:start-end` 加载所需行段。
- 互相独立且在本轮工具描述中标记为可并行的只读探索放进 parallel；有数据依赖的调用必须放进 sequence。不要把写入、命令、浏览器或任何控制面工具塞进运行时程序。
- 每个 call 的 `id` 全局唯一；引用只能读取已经完成的步骤。parallel 分支不能互相引用；并行块结束后，后续 sequence 才能读取各分支结果。
- 调查工具支持 `compress` / `compress_intent` 参数：`compress=false` 时绝不进行摘要，超过 8000 字符的结果会带截断行信息返回前 8000 字符；只有 `compress=true` 且结果超过 5000 字符时才进行摘要。分析代码、配置等需要精确内容时保持 `compress=false`；需从超长输出中提取关键内容时显式设置 `compress=true` 并写明 `compress_intent`。`read_file` 的 `paths` 可使用 `path:start-end` 协议精确读取包含边界的行范围。

## 输出语言

默认简体中文，面向有经验的开发者，结论直接清晰。
"#;

impl OrchestratorAgent {
    /// 静态提示词：角色 + 用户偏好（USER.md）+ 记忆 + 技能。
    /// 每轮构建一次（文件读取走 spawn_blocking）。
    pub(super) async fn build_static_prompt(&self) -> Result<String> {
        let root = self.config.root_dir.clone();
        let extra = tokio::task::spawn_blocking(move || load_prompt_files(&root))
            .await
            .map_err(|error| anyhow::anyhow!("读取编排器提示词文件失败：{error}"))?;

        // 版本占位符由常量生成：提示词示例、工具 schema、校验三方同源，
        // 契约升级时不再需要手工同步提示词里的示例值。
        let mut prompt = ORCHESTRATOR_ROLE_PROMPT.replace(
            "\"version\": \"{graph_definition_version}\"",
            &format!(
                "\"version\": {}",
                crate::agent::graph::types::GRAPH_DEFINITION_VERSION
            ),
        );
        if !extra.is_empty() {
            prompt.push_str("\n\n---\n\n");
            prompt.push_str(&extra);
        }
        Ok(prompt)
    }

    /// 每次迭代重建的完整系统提示：静态内容 + 动态分片。
    /// 静态部分按引用直接装配进最终 format!，避免逐迭代对 static_content
    /// 做大块中间克隆（最终 String 是唯一分配点，审查项 G8-21）。
    pub(super) fn build_iteration_system_prompt(
        &self,
        static_bundle: &PromptBundle,
        tool_definitions: &[ToolDefinition],
    ) -> String {
        let static_content = static_bundle.static_content.as_str();
        let tools_block = render_available_tools_block(tool_definitions);
        let local_time = crate::agent::prompt::current_local_time();
        if tools_block.is_empty() {
            format!("{static_content}\n\n---\n\n# 系统时间\n\n当前本地时间：{local_time}")
        } else {
            format!(
                "{static_content}\n\n---\n\n{tools_block}\n\n---\n\n# 系统时间\n\n当前本地时间：{local_time}"
            )
        }
    }

    pub(super) fn render_graph_harness_catalog(
        &self,
        catalog: &crate::agent::graph::types::GraphHarnessCatalog,
        stats: &[crate::agent::graph::types::GraphModelStat],
    ) -> String {
        let mut lines = vec![
            "# 当前 PI Harness 目录".to_string(),
            "该目录是 graph v3 的唯一模型与特殊工具来源；ID 必须原样引用。模型行末的历史统计（若有）来自既往节点运行，可作为选型参考。".to_string(),
            "\n## 主模型（每节点恰好一个）".to_string(),
        ];
        for model in &catalog.models {
            let stat_note = render_model_stat_note(&model.id, stats);
            lines.push(format!(
                "- `{}`：{} / {} / category={} / capabilities={}{}",
                model.id,
                model.label,
                model.model,
                model.category,
                model.capabilities.join(","),
                stat_note
            ));
        }
        lines.push("\n## 基础工具组".to_string());
        lines.push("- `read_only`: read, grep, find, ls".to_string());
        lines.push("- `coding`: read, grep, find, ls, bash, edit, write".to_string());
        lines.push("\n## 特殊工具".to_string());
        for tool in &catalog.tools {
            lines.push(format!(
                "- `{}:{}` [{} / {}]：{}",
                tool.source,
                tool.name,
                tool.category,
                if tool.review_required {
                    "需审查"
                } else {
                    "直接执行"
                },
                tool.description
            ));
        }
        if !catalog.diagnostics.is_empty() {
            lines.push(format!(
                "\n## 发现诊断\n- {}",
                catalog.diagnostics.join("\n- ")
            ));
        }
        lines.join("\n")
    }
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

fn render_available_tools_block(tool_definitions: &[ToolDefinition]) -> String {
    if tool_definitions.is_empty() {
        return String::new();
    }

    let mut lines = vec![
        "# 当前实际可用工具".to_string(),
        "以下列表来自本轮运行时实际注入的工具定义，是当前可调用工具的唯一准确信息源。".to_string(),
    ];

    let mut tools = tool_definitions
        .iter()
        .map(|tool| {
            (
                tool.function.name.clone(),
                tool.function.description.trim().to_string(),
            )
        })
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
    use super::{load_prompt_files, MAX_PROMPT_FILE_BYTES};
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
