use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Local;
use serde::Serialize;

pub(super) fn current_local_time() -> String {
    Local::now().format("%Y-%m-%d %H:%M").to_string()
}

const BUILT_IN_DISPATCH_GUIDANCE: &str = r#"# 内置调度规则

- 调度代理优先负责拆解、调查、定位、梳理上下文，再把实现工作交给执行代理。
- 对复杂度高、步骤多、会委派子 Agent 的任务，如果决定使用 `update_plan`，必须先调用 `update_plan` 创建本次任务规划步骤，再开始 glob/grep/read_file/exec 等探索或实践；简单任务可以完全跳过 Checklist。
- `dispatch_claude` 用于新功能、快速试错、探索性调试、方案空间较大、需要多轮收敛的编码任务。
- `dispatch_codex` 用于重构、结构治理、跨文件一致性修改、回归风险高、需要严格验证的编码任务。
- 探索代码时优先遵循 `必要时先 update_plan → glob 缩小范围 → grep 精确匹配 → read_file 加载确认 → 循环直到证据充分`，不要一上来大面积读文件。
- 独立的只读探索应尽量在同一轮返回多个工具调用；系统会按顺序安全地并发执行连续的 `read_file` / `list_dir` / `glob` / `grep` 调用。
- 调查工具使用数组参数：`read_file` / `list_dir` 使用 `paths`，`glob` / `grep` 使用 `patterns` 与 `paths`，结果会按路径或模式分段返回。
- 发起委派前，任务说明必须自包含：目标、背景、相关文件或符号、约束、验证方式、期望产出, 委派指令要精简准确。
- 实施已确认的 Plan 计划书时，不要重新调用 `update_plan` 做步骤规划；直接按计划书实际内容和 Claude/Codex 擅长点拆分委派，让子 Agent 读取计划 MD 并编码，然后等待执行结果、验收、调用 `mark_plan_implemented` 收口。
- 所有调查工具支持 `compress`（是否语义压缩）和 `compress_intent`（提取意图，一句话）两个顶层参数。结果超过 1000 字符时系统会强制压缩；`compress=true` 时，必须同时填写 `compress_intent` 说明本次调用期望提取什么信息。分析代码、配置或需要精确匹配时，保持 `compress=false` 让系统根据长度自动判断；命令输出、冗余日志较多时，推荐显式 `compress=true` 并写明 `compress_intent`，例如"确认 pnpm test 是否全部通过"。
- 子任务回流默认只同步任务摘要，不直接回灌完整终端日志；如果主调度仍缺证据，应继续下发更具体的子任务，或本地重新读文件/执行命令。
- 如果 Claude 与 Codex 可以并行推进不同工作流，可以在同一轮同时调用多个 `dispatch_*`；系统支持批量处理。
- 同一 session 内，同一 agent 同时最多只能有一个活跃或待启动子进程；不要对同一 agent 重复 dispatch。
- 继续或退出子进程时，必须使用同一家族的工具：`continue_claude_session` / `continue_codex_session`，`exit_claude_session` / `exit_codex_session`。
- 子任务返回“当前轮完成”不代表进程已退出；如果还要继续推进，应发送后续指令，而不是误判为已结束。
"#;

#[derive(Debug, Clone, Serialize)]
struct PromptSection {
    label: String,
    source: String,
    content: String,
}

#[derive(Debug, Clone)]
pub(super) struct PromptBundle {
    /// Content of static-only sections (no system time, no runtime state).
    /// Suitable for caching within a single `run()` / `continue_after_dispatch()` turn.
    pub static_content: String,
}

pub(super) fn build_system_prompt(root: &Path) -> Result<PromptBundle> {
    let mut sections = Vec::new();

    push_file_if_exists(&mut sections, "SOUL", root.join("SOUL.md"))?;
    push_file_if_exists(&mut sections, "USER", root.join("USER.md"))?;

    let skills_dir = root.join("skills");
    if skills_dir.exists() {
        let mut skill_parts = Vec::new();
        for entry in
            fs::read_dir(&skills_dir).with_context(|| format!("read {}", skills_dir.display()))?
        {
            let entry = entry?;
            let skill_md = entry.path().join("SKILL.md");
            if skill_md.exists() {
                skill_parts.push(format!(
                    "### 技能：{}\n\n{}",
                    entry.file_name().to_string_lossy(),
                    fs::read_to_string(&skill_md)
                        .with_context(|| format!("read {}", skill_md.display()))?
                ));
            }
        }
        skill_parts.sort();
        if !skill_parts.is_empty() {
            sections.push(PromptSection {
                label: "已启用技能".to_string(),
                source: skills_dir.display().to_string(),
                content: format!("---\n\n# 已启用技能\n\n{}", skill_parts.join("\n\n")),
            });
        }
    }

    let memory = root.join("memory").join("MEMORY.md");
    if memory.exists() {
        sections.push(PromptSection {
            label: "记忆".to_string(),
            source: memory.display().to_string(),
            content: format!(
                "---\n\n# 记忆\n\n{}",
                fs::read_to_string(&memory)
                    .with_context(|| format!("read {}", memory.display()))?
            ),
        });
    }

    sections.push(PromptSection {
        label: "内置调度规则".to_string(),
        source: "builtin".to_string(),
        content: format!("---\n\n{}", BUILT_IN_DISPATCH_GUIDANCE),
    });

    // Snapshot static content before adding dynamic "系统时间" section
    let static_content = sections
        .iter()
        .map(|section| section.content.as_str())
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");

    sections.push(PromptSection {
        label: "系统时间".to_string(),
        source: "builtin".to_string(),
        content: format!(
            "---\n\n# 系统时间\n\n当前本地时间：{}",
            current_local_time()
        ),
    });

    Ok(PromptBundle { static_content })
}

fn push_file_if_exists(
    sections: &mut Vec<PromptSection>,
    label: &str,
    path: PathBuf,
) -> Result<()> {
    if path.exists() {
        sections.push(PromptSection {
            label: label.to_string(),
            source: path.display().to_string(),
            content: fs::read_to_string(&path)
                .with_context(|| format!("read {}", path.display()))?,
        });
    }
    Ok(())
}
