use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

const BUILT_IN_DISPATCH_GUIDANCE: &str = r#"# 内置调度规则

- 调度代理优先负责调查、定位、梳理上下文，再把实现工作交给执行代理。
- `dispatch_claude` 用于新功能、快速试错、探索性调试、方案空间较大、需要多轮收敛的编码任务。
- `dispatch_codex` 用于重构、结构治理、跨文件一致性修改、回归风险高、需要严格验证的编码任务。
- 发起委派前，任务说明必须自包含：目标、背景、相关文件或符号、约束、验证方式、期望产出, 委派指令要精简准确。
- 调查工具支持 `result_mode`：`full` 保留精确信息，`summary` 仅在内容较长时触发高保真压缩并只影响写回主上下文的内容，前端展示文案与详细结果引用会单独保留，`auto` 由系统按工具类型决定。`read_file` / `list_dir` / `glob`/ `exec` 以及任何代码、配置、精确检索结果都不应指定摘要
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
    push_file_if_exists(&mut sections, "TOOLS", root.join("TOOLS.md"))?;

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
