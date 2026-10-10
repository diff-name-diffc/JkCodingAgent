//! 读屏的输入提示：仅依据当前光标行，不把旧日志或单纯静默当作等待输入。

const PROMPT_IDLE_MS: u64 = 1_000;

pub(super) fn input_hint(
    screen: &str,
    cursor_row: usize,
    idle_ms: u64,
    exited: bool,
) -> Option<&'static str> {
    if exited || idle_ms < PROMPT_IDLE_MS {
        return None;
    }
    let line = screen.lines().nth(cursor_row)?.trim().to_ascii_lowercase();
    if line.is_empty() {
        return None;
    }
    if [
        "password",
        "passphrase",
        "密码",
        "口令",
        "verification code",
        "one-time password",
    ]
    .iter()
    .any(|needle| line.contains(needle))
        && (line.ends_with(':') || line.ends_with('：') || line.ends_with('?'))
    {
        return Some("疑似等待密码/口令/验证码；禁止通过终端发送凭据，请改用 ssh_exec sudo=true 等非交互路径或由用户介入");
    }
    if line.contains("(yes/no/[fingerprint])")
        || line.contains("continue connecting")
        || line.contains("host key verification")
    {
        return Some("疑似 SSH 主机指纹确认；先由用户通过可信渠道核对指纹，不可套用普通 y/n 模板自动接受。未核实时可用 ssh_term_send text=\"\\u0003\" 取消");
    }
    if ["[y/n]", "(y/n)", "[yes/no]", "(yes/no)"]
        .iter()
        .any(|needle| line.contains(needle))
    {
        return Some("疑似等待确认或按键；应答模板：ssh_term_send 使用 text=\"y\"（yes/no 提示用 \"yes\"）或 text=\"n\"（\"no\"），text_mode=\"literal\"、enter=true，并填写 intent。先结合任务与 screen 决定是否同意，每次应答仍需审查");
    }
    if [
        "press enter",
        "press return",
        "press <enter>",
        "press <return>",
        "hit enter",
        "hit return",
        "hit <enter>",
        "hit <return>",
        "按回车",
    ]
    .iter()
    .any(|needle| line.contains(needle))
    {
        return Some("疑似等待确认或按键；回车应答模板：ssh_term_send 使用 text=\"\"、text_mode=\"literal\"、enter=true，并填写 intent。不要发送字面 \\r；先确认继续操作符合任务，每次应答仍需审查");
    }
    if [
        "(yes/no",
        "are you sure",
        "continue?",
        "press any key",
        "save modified buffer",
        "(e)dit anyway",
        "按任意键",
        "是否继续",
    ]
    .iter()
    .any(|needle| line.contains(needle))
        || (line.contains("save changes") && line.ends_with('?'))
    {
        return Some("疑似等待确认或按键；请结合 screen 与本轮任务判断输入内容");
    }
    if line == ">"
        || line.ends_with('$')
        || line.ends_with('#')
        || [">>>", "...", "mysql>", "sqlite>"]
            .iter()
            .any(|suffix| line.ends_with(suffix))
    {
        return Some(
            "疑似停在 shell/REPL 提示符等待输入；这是读屏启发式判断，不能证明前一条命令成功",
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::input_hint;

    #[test]
    fn identifies_current_prompt_after_idle() {
        for line in [
            "root@server:~# ",
            "$ ",
            ">>> ",
            "mysql> ",
            "Continue? [Y/n] ",
            "Press Enter to continue",
        ] {
            assert!(input_hint(line, 0, 1_000, false).is_some(), "{line}");
        }
    }

    #[test]
    fn credentials_prompt_requires_user_or_noninteractive_path() {
        for line in [
            "Password:",
            "Enter passphrase for key '/tmp/test':",
            "密码：",
        ] {
            assert!(input_hint(line, 0, 1_000, false).unwrap().contains("禁止"));
        }
    }

    #[test]
    fn confirmation_templates_do_not_accept_unknown_host_keys() {
        let confirm = input_hint("Continue? [Y/n]", 0, 1_000, false).unwrap();
        assert!(confirm.contains("text=\"y\""));
        assert!(confirm.contains("enter=true"));
        let enter = input_hint("Press RETURN to continue", 0, 1_000, false).unwrap();
        assert!(enter.contains("text=\"\""));
        for prompt in [
            "Are you sure you want to continue connecting (yes/no/[fingerprint])?",
            "Are you sure you want to continue connecting (yes/no)?",
        ] {
            let hint = input_hint(prompt, 0, 1_000, false).unwrap();
            assert!(hint.contains("可信渠道核对指纹"));
            assert!(!hint.contains("text=\"y\""));
        }
    }

    #[test]
    fn nested_pager_and_editor_prompts_require_a_current_idle_line() {
        for line in [
            "Pattern not found (press RETURN)",
            "Press RETURN to continue",
            "Press <Enter> to continue",
            "Press ENTER or type command to continue",
            "Hit ENTER or type command to continue",
            "Hit <Return> to continue",
            "Save modified buffer?",
            "Save changes to \"notes.txt\"?",
            "(O)pen Read-Only, (E)dit anyway, (R)ecover, (Q)uit, (A)bort:",
        ] {
            assert!(
                input_hint(line, 0, 1_000, false)
                    .unwrap()
                    .contains("确认或按键"),
                "{line}"
            );
            assert!(input_hint(line, 0, 999, false).is_none(), "{line}");
            assert!(input_hint(line, 0, 1_000, true).is_none(), "{line}");
            let screen = format!("{line}\nrunning backup");
            assert!(input_hint(&screen, 1, 90_000, false).is_none(), "{line}");
        }
    }

    #[test]
    fn idle_or_historical_prompts_do_not_establish_waiting() {
        assert!(input_hint("running backup", 0, 90_000, false).is_none());
        assert!(input_hint("Password:\nworking\n", 1, 90_000, false).is_none());
        assert!(input_hint("$ ", 0, 999, false).is_none());
        assert!(input_hint("$ ", 0, 1_000, true).is_none());
        assert!(input_hint("password updated", 0, 1_000, false).is_none());
        assert!(input_hint("$ \n\n[tmux status]", 1, 90_000, false).is_none());
    }
}
