//! 工作目录提示直接来自本次工具执行上下文，不依赖用户配置或模型猜测。
use crate::agent::llm::{ChatMessage, ToolDefinition};
use crate::agent::tools::{local_zsh_dir, ToolContext};
use anyhow::Result;
use std::path::{Path, PathBuf};

pub(crate) fn append(
    messages: &mut Vec<ChatMessage>,
    context: &ToolContext,
    definitions: &[ToolDefinition],
) -> Result<()> {
    let block = render(
        &context.workspace,
        context.restrict_to_workspace,
        &context.extra_allowed_dirs,
        definitions
            .iter()
            .any(|tool| tool.function.name == "local_zsh"),
    )
    .map_err(anyhow::Error::msg)?;
    if let Some(system) = messages
        .first_mut()
        .filter(|message| message.role == "system")
    {
        system.content.push_str(&block);
    } else {
        messages.insert(0, ChatMessage::system(block));
    }
    Ok(())
}

fn render(
    workspace: &Path,
    restricted: bool,
    extra: &[PathBuf],
    local_zsh: bool,
) -> Result<String, String> {
    let mut prompt = format!(
        "\n\n## 本次运行的工作目录与路径权限\n\n\
         - 当前文件工作区（绝对路径）：{}\n\
         - 文件工具与 sync_directory.source 的相对路径均以该工作区为基准；exec 默认在该工作区执行。\n",
        workspace.display()
    );
    if restricted {
        prompt.push_str("- 文件路径限制：只允许当前工作区及下列额外授权路径；上级目录、其他项目目录不会自动获得授权。\n");
    } else {
        prompt.push_str("- 工作区边界限制已关闭；应用敏感路径保护与命令安全审查仍生效。\n");
    }
    if extra.is_empty() {
        prompt.push_str("- 额外授权路径：无。\n");
    } else {
        prompt.push_str("- 额外授权路径（目录内或精确文件）：\n");
        for path in extra {
            prompt.push_str(&format!("  - {}\n", path.display()));
        }
    }
    if local_zsh {
        let directory = local_zsh_dir(workspace)?;
        prompt.push_str(&format!(
            "- local_zsh 实际命令工作目录：{}。它与文件工作区不同，且不能使用 cd。命令内的相对路径以此目录为基准。\n\
             - 上传 local_zsh 生成的文件时，使用该执行目录下产物子目录的绝对路径作为 source，不要将相对路径误当成文件工作区下的路径。\n",
            directory.display()
        ));
    }
    prompt.push_str(
        "- sync_directory.destination 是远端目录，不受本地工作区前缀约束；source 必须是本地已有的产物目录。\n\
         - 不要上传整个 .jkcodingagent 配置根目录或 .git 元数据目录；会话工作区和 local_zsh 的产物子目录可按现有路径权限使用。\n\
         - 不要猜测 /Users、用户主目录、/releases 等目录。路径越界时说明当前工作区和所需路径，让用户在对应项目会话操作或将文件放入允许目录；不要换工具绕过限制或重复尝试同一个被拒绝路径。\n"
    );
    Ok(prompt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declares_exact_workspace_and_permissions() {
        let text = render(
            Path::new("/projects/release"),
            true,
            &[PathBuf::from("/allowed/images")],
            false,
        )
        .unwrap();
        assert!(text.contains("当前文件工作区（绝对路径）：/projects/release"));
        assert!(text.contains("/allowed/images"));
        assert!(text.contains("上级目录、其他项目目录不会自动获得授权"));
        assert!(!text.contains("local_zsh 实际命令工作目录"));
    }

    #[test]
    fn distinguishes_session_workspace_from_shell_directory() {
        let workspace = Path::new("/home/test/.jkcodingagent/plain-chat-browser/session-one");
        let text = render(workspace, true, &[], true).unwrap();
        assert!(text.contains("额外授权路径：无"));
        assert!(text.contains(&format!(
            "local_zsh 实际命令工作目录：{}",
            local_zsh_dir(workspace).unwrap().display()
        )));
        assert!(text.contains("session-one/.jkcodingagent/local_env/zsh"));
        assert!(text.contains("绝对路径作为 source"));
    }

    #[test]
    fn unrestricted_mode_does_not_claim_workspace_sandbox() {
        let text = render(Path::new("/project"), false, &[], false).unwrap();
        assert!(text.contains("工作区边界限制已关闭"));
        assert!(!text.contains("只允许当前工作区"));
    }
}
