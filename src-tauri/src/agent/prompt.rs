use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

const BUILT_IN_DISPATCH_GUIDANCE: &str = r#"# 内置调度规则

- 调度代理优先负责调查、定位、梳理上下文，再把实现工作交给执行代理。
- `dispatch_claude` 用于新功能、快速试错、探索性调试、方案空间较大、需要多轮收敛的编码任务。
- `dispatch_codex` 用于重构、结构治理、跨文件一致性修改、回归风险高、需要严格验证的编码任务。
- 发起委派前，任务说明必须自包含：目标、背景、相关文件或符号、约束、验证方式、期望产出。
- 继续或退出子进程时，必须使用同一家族的工具：`continue_claude_session` / `continue_codex_session`，`exit_claude_session` / `exit_codex_session`。
- 子任务返回“当前轮完成”不代表进程已退出；如果还要继续推进，应发送后续指令，而不是误判为已结束。
"#;

pub(super) fn build_system_prompt(root: &Path) -> Result<String> {
    let mut parts = Vec::new();

    push_file_if_exists(&mut parts, root.join("SOUL.md"))?;
    push_file_if_exists(&mut parts, root.join("USER.md"))?;
    push_file_if_exists(&mut parts, root.join("TOOLS.md"))?;

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
                    "### Skill: {}\n\n{}",
                    entry.file_name().to_string_lossy(),
                    fs::read_to_string(&skill_md)
                        .with_context(|| format!("read {}", skill_md.display()))?
                ));
            }
        }
        skill_parts.sort();
        if !skill_parts.is_empty() {
            parts.push(format!(
                "---\n\n# Active Skills\n\n{}",
                skill_parts.join("\n\n")
            ));
        }
    }

    let memory = root.join("memory").join("MEMORY.md");
    if memory.exists() {
        parts.push(format!(
            "---\n\n# Memory\n\n{}",
            fs::read_to_string(&memory).with_context(|| format!("read {}", memory.display()))?
        ));
    }

    parts.push(format!("---\n\n{}", BUILT_IN_DISPATCH_GUIDANCE));

    Ok(parts.join("\n\n---\n\n"))
}

fn push_file_if_exists(parts: &mut Vec<String>, path: PathBuf) -> Result<()> {
    if path.exists() {
        parts.push(fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?);
    }
    Ok(())
}
