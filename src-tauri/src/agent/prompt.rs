use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

const BUILT_IN_DISPATCH_GUIDANCE: &str = r#"# Built-in Dispatch Guidance

- Use `dispatch_claude` for tasks where speed matters more: greenfield features, algorithm experiments, debugging exploration, and broad implementation search.
- Use `dispatch_codex` for tasks where extra care matters more: refactoring, structural cleanup, consistency passes, and regression-sensitive changes.
- When continuing or exiting a subprocess, always use the matching tool for the same agent family (`continue_claude_session` / `continue_codex_session`, `exit_claude_session` / `exit_codex_session`).
- Dispatcher may receive either a round-complete update or a process-exited update from a delegated subprocess. Round-complete means the subprocess is still running and can accept more instructions via `continue_*_session`; do not treat it as the subprocess having exited.
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
