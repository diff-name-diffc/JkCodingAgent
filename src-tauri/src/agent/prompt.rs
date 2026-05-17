use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

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
- 调查工具支持 `result_mode`：`full` 保留精确信息，`summary` 仅在内容较长时触发高保真压缩并只影响写回主上下文的内容，前端展示文案与详细结果引用会单独保留，`auto` 由系统按工具类型决定。`read_file` / `list_dir` / `glob` / `grep` / `exec` 以及任何代码、配置、精确检索结果都不应指定摘要
- 子任务回流默认只同步任务摘要，不直接回灌完整终端日志；如果主调度仍缺证据，应继续下发更具体的子任务，或本地重新读文件/执行命令。
- 如果 Claude 与 Codex 可以并行推进不同工作流，可以在同一轮同时调用多个 `dispatch_*`；系统支持批量处理。
- 同一 session 内，同一 agent 同时最多只能有一个活跃或待启动子进程；不要对同一 agent 重复 dispatch。
- 继续或退出子进程时，必须使用同一家族的工具：`continue_claude_session` / `continue_codex_session`，`exit_claude_session` / `exit_codex_session`。
- 子任务返回“当前轮完成”不代表进程已退出；如果还要继续推进，应发送后续指令，而不是误判为已结束。
"#;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PromptSection {
    pub label: String,
    pub source: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub(super) struct PromptBundle {
    pub content: String,
    pub sections: Vec<PromptSection>,
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

    let content = sections
        .iter()
        .map(|section| section.content.as_str())
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");

    Ok(PromptBundle { content, sections })
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

#[cfg(test)]
mod tests {
    use std::fs;

    use super::build_system_prompt;

    #[test]
    fn system_prompt_ignores_persisted_tools_file() {
        let root = std::env::temp_dir().join(format!(
            "jkcodingagent-prompt-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).expect("create prompt root");
        fs::write(root.join("SOUL.md"), "# Soul\n").expect("write soul");
        fs::write(root.join("USER.md"), "# User\n").expect("write user");
        fs::write(root.join("TOOLS.md"), "stale hard-coded tool list").expect("write tools");

        let prompt = build_system_prompt(&root).expect("build prompt");

        assert!(prompt.content.contains("# Soul"));
        assert!(prompt.content.contains("# User"));
        assert!(!prompt.content.contains("stale hard-coded tool list"));
        assert!(!prompt
            .sections
            .iter()
            .any(|section| section.label == "TOOLS"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn system_prompt_includes_builtin_dispatch_guidance() {
        let root = std::env::temp_dir().join(format!(
            "jkcodingagent-prompt-builtin-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).expect("create prompt root");

        let prompt = build_system_prompt(&root).expect("build prompt");
        assert!(prompt.content.contains("内置调度规则"));
        assert!(prompt.content.contains("dispatch_claude"));
        assert!(prompt.content.contains("dispatch_codex"));
        assert!(prompt
            .sections
            .iter()
            .any(|section| section.label == "内置调度规则"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn system_prompt_includes_skill_sections() {
        let root = std::env::temp_dir().join(format!(
            "jkcodingagent-prompt-skills-{}",
            uuid::Uuid::new_v4()
        ));
        let skills_dir = root.join("skills").join("my-skill");
        fs::create_dir_all(&skills_dir).expect("create skill dir");
        fs::write(skills_dir.join("SKILL.md"), "# My Skill\n\nDo something useful.")
            .expect("write skill");

        let prompt = build_system_prompt(&root).expect("build prompt");
        assert!(prompt.content.contains("已启用技能"));
        assert!(prompt.content.contains("My Skill"));
        assert!(prompt.content.contains("Do something useful"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn system_prompt_includes_memory_when_present() {
        let root = std::env::temp_dir().join(format!(
            "jkcodingagent-prompt-memory-{}",
            uuid::Uuid::new_v4()
        ));
        let memory_dir = root.join("memory");
        fs::create_dir_all(&memory_dir).expect("create memory dir");
        fs::write(memory_dir.join("MEMORY.md"), "# Memory\n\nRemember this.")
            .expect("write memory");

        let prompt = build_system_prompt(&root).expect("build prompt");
        assert!(prompt.content.contains("记忆"));
        assert!(prompt.content.contains("Remember this"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn system_prompt_omits_memory_when_absent() {
        let root = std::env::temp_dir().join(format!(
            "jkcodingagent-prompt-no-memory-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).expect("create root");

        let prompt = build_system_prompt(&root).expect("build prompt");
        assert!(!prompt
            .sections
            .iter()
            .any(|section| section.label == "记忆"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn system_prompt_omits_skills_when_no_skill_md() {
        let root = std::env::temp_dir().join(format!(
            "jkcodingagent-prompt-no-skills-{}",
            uuid::Uuid::new_v4()
        ));
        let skills_dir = root.join("skills").join("empty-skill");
        fs::create_dir_all(&skills_dir).expect("create skills dir");
        // No SKILL.md inside

        let prompt = build_system_prompt(&root).expect("build prompt");
        assert!(!prompt
            .sections
            .iter()
            .any(|section| section.label == "已启用技能"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn system_prompt_sections_have_correct_labels() {
        let root = std::env::temp_dir().join(format!(
            "jkcodingagent-prompt-labels-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).expect("create root");
        fs::write(root.join("SOUL.md"), "# Soul\n").expect("write soul");
        fs::write(root.join("USER.md"), "# User\n").expect("write user");

        let prompt = build_system_prompt(&root).expect("build prompt");
        let labels: Vec<&str> = prompt.sections.iter().map(|s| s.label.as_str()).collect();
        assert!(labels.contains(&"SOUL"));
        assert!(labels.contains(&"USER"));
        assert!(labels.contains(&"内置调度规则"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn system_prompt_handles_multiple_skills_sorted() {
        let root = std::env::temp_dir().join(format!(
            "jkcodingagent-prompt-multi-skill-{}",
            uuid::Uuid::new_v4()
        ));
        let skill_b = root.join("skills").join("b-skill");
        let skill_a = root.join("skills").join("a-skill");
        fs::create_dir_all(&skill_b).expect("create b dir");
        fs::create_dir_all(&skill_a).expect("create a dir");
        fs::write(skill_b.join("SKILL.md"), "# B Skill\n\nSecond skill.")
            .expect("write b");
        fs::write(skill_a.join("SKILL.md"), "# A Skill\n\nFirst skill.")
            .expect("write a");

        let prompt = build_system_prompt(&root).expect("build prompt");
        let skills_section = prompt
            .sections
            .iter()
            .find(|s| s.label == "已启用技能")
            .expect("skills section exists");

        // Skills should be sorted: a-skill before b-skill
        let a_pos = skills_section.content.find("A Skill").expect("find a");
        let b_pos = skills_section.content.find("B Skill").expect("find b");
        assert!(a_pos < b_pos);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn system_prompt_empty_dir_produces_only_builtin() {
        let root = std::env::temp_dir().join(format!(
            "jkcodingagent-prompt-empty-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).expect("create root");

        let prompt = build_system_prompt(&root).expect("build prompt");
        assert_eq!(prompt.sections.len(), 1);
        assert_eq!(prompt.sections[0].label, "内置调度规则");

        let _ = fs::remove_dir_all(root);
    }
}
