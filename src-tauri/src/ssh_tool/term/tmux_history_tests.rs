use super::{capture_command, parse_capture};
use crate::ssh_tool::term::screen::NEW_LINES_MAX_CHARS;

#[test]
fn capture_targets_exact_session_and_only_negative_history_rows() {
    assert_eq!(
        capture_command("jkagent-work", 5, 3, true),
        "tmux display-message -p -t =jkagent-work: '#{history_size}' \\; capture-pane -p -e -t =jkagent-work: -S -8 -E -6"
    );
    assert!(capture_command("jkagent-work", usize::MAX, 200, false)
        .ends_with("-S -2147483647 -E -2147483647"));
}

#[test]
fn pages_are_old_to_new_with_offsets_from_latest_history() {
    let page = parse_capture(b"8\n5\n6\n7\n", 0, 3, false).unwrap();
    assert_eq!(page.source, "tmux");
    assert_eq!(page.available_lines, 8);
    assert_eq!(page.lines, ["5", "6", "7"]);
    assert_eq!(page.next_offset, Some(3));
    let page = parse_capture(b"8\n0\n1\n2\n", 5, 3, false).unwrap();
    assert_eq!(page.lines, ["0", "1", "2"]);
    assert_eq!(page.next_offset, None);
    assert!(!page.truncated);
}

#[test]
fn tmux_clamped_viewport_or_oldest_line_is_not_mistaken_for_history() {
    for (raw, offset) in [
        (b"0\ncurrent viewport\n".as_slice(), 0),
        (b"8\noldest\n", 50),
    ] {
        let page = parse_capture(raw, offset, 3, false).unwrap();
        assert!(page.lines.is_empty());
        assert_eq!(page.next_offset, None);
    }
    let page = parse_capture(b"1\n\n", 0, 1, false).unwrap();
    assert_eq!(page.lines, [""]);
}

#[test]
fn malformed_or_incomplete_history_is_an_error() {
    for raw in [
        b"".as_slice(),
        b"not history\n",
        b"3\nonly one\n",
        b"1\n\xff\n",
    ] {
        assert!(parse_capture(raw, 0, 3, false).is_err());
    }
}

#[test]
fn ansi_rebuild_preserves_wide_long_rows_and_removes_terminal_controls() {
    let content = "你好".repeat(400);
    let raw = format!("1\n\x1b]52;c;secret\x07\x1b[6n\x1b[31m{content}\x1b[39m\n");
    let plain = parse_capture(raw.as_bytes(), 0, 10, false).unwrap();
    assert_eq!(plain.lines, [content.as_str()]);
    let ansi = parse_capture(raw.as_bytes(), 0, 10, true).unwrap();
    assert!(ansi.lines[0].contains(&content));
    assert!(ansi.lines[0].contains("38;5;1"));
    assert!(ansi.lines[0].ends_with("\x1b[0m"));
    assert!(!ansi.lines[0].contains("secret"));
    assert!(!ansi.lines[0].contains("[6n"));
}

#[test]
fn history_byte_budget_keeps_recent_rows_and_reports_consumed_offset() {
    let raw = format!(
        "4\n{}\n{}\n{}\n{}\n",
        "a".repeat(5000),
        "b".repeat(5000),
        "c".repeat(5000),
        "d".repeat(5000)
    );
    let page = parse_capture(raw.as_bytes(), 0, 4, false).unwrap();
    assert_eq!(page.lines, ["c".repeat(5000), "d".repeat(5000)]);
    assert_eq!(page.next_offset, Some(2));
    assert!(page.truncated);
    let raw = format!("1\n\x1b[31m{}\x1b[0m\n", "你".repeat(20_000));
    let page = parse_capture(raw.as_bytes(), 0, 1, true).unwrap();
    assert!(page.lines[0].len() < NEW_LINES_MAX_CHARS);
    assert!(page.lines[0].ends_with("\x1b[0m"));
    assert!(page.truncated);
}

#[test]
fn ansi_styles_continue_across_full_width_lines_and_follow_later_changes() {
    let red = "R".repeat(40);
    let green = "G".repeat(40);
    // 真 tmux 满宽行 capture-pane -e：仅样式变化时写 SGR，不保证每行独立带颜色。
    let raw = format!("5\n\x1b[31m{red}\n{red}\n\x1b[32m{green}\n{green}\x1b[0m\nplain\n");
    let page = parse_capture(raw.as_bytes(), 0, 5, true).unwrap();
    for line in &page.lines[..2] {
        assert!(line.contains("38;5;1"), "红色应跨行沿用：{line:?}");
    }
    for line in &page.lines[2..4] {
        assert!(
            line.contains("38;5;2"),
            "中途变化后的绿色应跨行沿用：{line:?}"
        );
    }
    assert!(!page.lines[4].contains("38;5;"));
    assert!(page.lines[4].contains("plain"));
}

#[test]
fn clipped_older_line_still_updates_styles_for_returned_recent_lines() {
    let raw = format!("2\n\x1b[31m{}\x1b[32m\nrecent\n", "x".repeat(20_000));
    let page = parse_capture(raw.as_bytes(), 0, 2, true).unwrap();
    // 较早长行不在本页输出预算内，但其末尾绿色仍支配较新行。
    assert_eq!(page.lines.len(), 1);
    assert!(page.lines[0].contains("38;5;2"));
    assert!(!page.lines[0].contains("38;5;1"));
    assert!(page.lines[0].contains("recent"));
}
