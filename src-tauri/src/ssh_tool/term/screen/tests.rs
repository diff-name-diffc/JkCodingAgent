use super::*;

#[test]
fn waiting_does_not_duplicate_overlapping_pending_and_screen_text() {
    let mut screen = TermScreen::new(80, 24);
    screen.push_bytes(b"READY\r\n");
    assert!(screen.matches_output("READY"));
    assert!(!screen.matches_output("READY\nREADY"));
}

#[test]
fn query_replies_use_live_cursor_at_each_query_and_survive_split_frames() {
    let mut screen = TermScreen::new(80, 24);
    screen.push_bytes(b"\x1b[4;7H\x1b[6");
    assert!(screen.take_responses().unwrap().is_empty());
    screen.push_bytes(b"n\x1b[8;12H\x1b[6n\x1b[?25l");
    assert_eq!(screen.take_responses().unwrap(), b"\x1b[4;7R\x1b[8;12R");
    screen.resize(120, 40);
    screen.push_bytes(b"\x1b[18t");
    assert_eq!(screen.take_responses().unwrap(), b"\x1b[8;40;120t");
}

#[test]
fn framer_reassembles_multibyte_split_across_chunks() {
    let mut framer = Utf8Framer::new();
    // 「你」= E4 BD A0，三种切法都应还原。
    assert_eq!(framer.push(&[0xE4]), "");
    assert_eq!(framer.push(&[0xBD, 0xA0, 0xE4]), "你");
    assert_eq!(framer.push(&[0xBD]), "");
    assert_eq!(framer.push(&[0xA0]), "你");
}

#[test]
fn framer_passes_through_invalid_bytes_as_replacement() {
    let mut framer = Utf8Framer::new();
    let s = framer.push(&[b'o', 0xFF, b'k']);
    assert_eq!(s, "o\u{fffd}k");
}

#[test]
fn alt_tracker_detects_enter_and_exit() {
    let mut tracker = AltScreenTracker::new();
    assert!(!tracker.is_alt());
    assert!(matches!(
        tracker.push("junk\x1b[?1049h\x1b[2J"),
        Some(AltScreenEvent::Entered)
    ));
    assert!(tracker.is_alt());
    assert!(matches!(
        tracker.push("more\x1b[?1049l"),
        Some(AltScreenEvent::Left)
    ));
    assert!(!tracker.is_alt());
}

#[test]
fn alt_tracker_handles_sequence_split_across_chunks() {
    let mut tracker = AltScreenTracker::new();
    assert!(tracker.push("\x1b[?10").is_none());
    assert!(matches!(tracker.push("49h"), Some(AltScreenEvent::Entered)));
    assert!(tracker.push("\x1b[?104").is_none());
    assert!(matches!(tracker.push("9l"), Some(AltScreenEvent::Left)));
}

#[test]
fn alt_tracker_accepts_legacy_variants_without_double_switch() {
    let mut tracker = AltScreenTracker::new();
    assert!(matches!(
        tracker.push("\x1b[?47h"),
        Some(AltScreenEvent::Entered)
    ));
    // 已在备用屏，再进（1047h）不重复报告。
    assert!(tracker.push("\x1b[?1047h").is_none());
    assert!(matches!(
        tracker.push("\x1b[?1047l\x1b[?47l"),
        Some(AltScreenEvent::Left)
    ));
}

#[test]
fn incremental_track_finalizes_lines_on_cursor_advance() {
    let mut screen = TermScreen::new(80, 24);
    // shell 提示符 + 命令回显 + 两行输出 + 新提示符（未定稿）。
    screen.push_bytes(b"$ ls\r\nfile1\r\nfile2\r\n$ ");
    let batch = screen.take_new_lines();
    assert_eq!(batch.lines, vec!["$ ls", "file1", "file2"]);
    assert!(!batch.truncated);
    // 光标行（"$ "）永远未定稿，由快照轨呈现。
    assert!(screen.take_new_lines().lines.is_empty());
    assert!(screen.snapshot().lines().any(|l| l == "$"));
}

#[test]
fn incremental_track_outputs_without_scrolling() {
    // 不满屏输出（设计初稿行数不变量的盲区）：同样产出增量。
    let mut screen = TermScreen::new(80, 24);
    screen.push_bytes(b"$ echo hi\r\nhi\r\n$ ");
    assert_eq!(screen.take_new_lines().lines, vec!["$ echo hi", "hi"]);
}

#[test]
fn incremental_track_carriage_return_rewrite_finalizes_once() {
    let mut screen = TermScreen::new(80, 24);
    // 进度条：同行 \r 重写，只定稿最终形态。
    screen.push_bytes(b"$ curl ...\r\n50%\r\x1b[K100%\r\n$ ");
    assert_eq!(screen.take_new_lines().lines, vec!["$ curl ...", "100%"]);
}

#[test]
fn incremental_track_survives_scrolling() {
    let mut screen = TermScreen::new(10, 3);
    for i in 0..10 {
        screen.push_bytes(format!("line{i}\r\n").as_bytes());
    }
    let batch = screen.take_new_lines();
    assert_eq!(
        batch.lines,
        (0..10).map(|i| format!("line{i}")).collect::<Vec<_>>()
    );
}

#[test]
fn alt_screen_reports_changes_then_resumes_completed_lines() {
    let mut screen = TermScreen::new(80, 24);
    screen.push_bytes(b"$ htop\r\n");
    assert_eq!(screen.take_new_lines().lines, vec!["$ htop"]);
    // 进入备用屏：首次读取返回可见内容，重复读取不复述整屏。
    screen.push_bytes(b"\x1b[?1049h\x1b[H\x1b[2Jhtop-screen");
    assert!(screen.alt_screen());
    assert_eq!(screen.take_new_lines().lines, vec!["htop-screen"]);
    assert!(screen.take_new_lines().lines.is_empty());
    assert!(screen.snapshot().contains("htop-screen"));
    // 退出备用屏：重对齐后新输出恢复定稿，且不爆发旧行。
    screen.push_bytes(b"\x1b[?1049l");
    assert!(!screen.alt_screen());
    assert!(screen.take_new_lines().lines.is_empty());
    screen.push_bytes(b"$ next\r\n");
    assert_eq!(screen.take_new_lines().lines, vec!["$ next"]);
}

#[test]
fn alt_screen_scroll_keeps_status_line_and_reports_only_new_content() {
    let mut screen = TermScreen::new(20, 4);
    screen.push_bytes(b"\x1b[?1049h\x1b[Hone\r\ntwo\r\nthree\r\n[tmux]");
    assert_eq!(
        screen.take_new_lines().lines,
        vec!["one", "two", "three", "[tmux]"]
    );
    screen.push_bytes(b"\x1b[H\x1b[2Jtwo\r\nthree\r\nfour\r\n[tmux]");
    assert_eq!(screen.take_new_lines().lines, vec!["four"]);
    assert!(screen.take_new_lines().lines.is_empty());
}

#[test]
fn alt_screen_redraw_reports_final_rewritten_line_once() {
    let mut screen = TermScreen::new(20, 4);
    screen.push_bytes(b"\x1b[?1049h\x1b[Hheader\r\n10%\r\nfooter");
    screen.take_new_lines();
    screen.push_bytes(b"\x1b[2;1H\x1b[K20%");
    screen.push_bytes(b"\x1b[2;1H\x1b[K100%");
    assert_eq!(screen.take_new_lines().lines, vec!["100%"]);
    screen.push_bytes(b"\x1b[Hheader\r\n100%\r\nfooter");
    assert!(screen.take_new_lines().lines.is_empty());
}

#[test]
fn alt_screen_repeated_lines_keep_multiplicity_in_diff() {
    let mut screen = TermScreen::new(20, 4);
    screen.push_bytes(b"\x1b[?1049h\x1b[Hsame\r\nsame");
    assert_eq!(screen.take_new_lines().lines, vec!["same", "same"]);
    screen.push_bytes(b"\r\nsame");
    assert_eq!(screen.take_new_lines().lines, vec!["same"]);
}

#[test]
fn history_pages_backwards_outside_viewport_without_consuming_new_lines() {
    let mut screen = TermScreen::new(20, 3);
    screen.push_bytes(b"zero\r\none\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\n");
    let recent = screen.history(0, 2, false);
    assert_eq!(recent.available_lines, 5);
    assert_eq!(recent.lines, vec!["three", "four"]);
    assert_eq!(recent.next_offset, Some(2));
    assert!(recent.truncated);
    let older = screen.history(recent.next_offset.unwrap(), 2, false);
    assert_eq!(older.lines, vec!["one", "two"]);
    let oldest = screen.history(older.next_offset.unwrap(), 2, false);
    assert_eq!(oldest.lines, vec!["zero"]);
    assert_eq!(oldest.next_offset, None);
    assert!(!oldest.truncated);
    assert_eq!(screen.take_new_lines().lines.len(), 7);
    assert!(screen.history(usize::MAX, 2, false).lines.is_empty());
}

#[test]
fn history_is_bounded_and_does_not_claim_tmux_server_history() {
    let mut screen = TermScreen::new(200, 3);
    for _ in 0..(SCROLLBACK_LIMIT + 5) {
        screen.push_bytes(format!("{}\r\n", "x".repeat(150)).as_bytes());
    }
    let history = screen.history(0, usize::MAX, false);
    assert_eq!(history.available_lines, SCROLLBACK_LIMIT);
    assert!(history.lines.len() <= NEW_LINES_MAX_ROWS);
    assert!(
        history
            .lines
            .iter()
            .map(|line| line.len() + 1)
            .sum::<usize>()
            <= NEW_LINES_MAX_CHARS
    );
    assert!(history.truncated);
    screen.push_bytes(b"\x1b[?1049h");
    assert_eq!(screen.history(0, 10, false).available_lines, 0);
    screen.push_bytes(b"\x1b[?1049l");
    assert_eq!(
        screen.history(0, 10, false).available_lines,
        SCROLLBACK_LIMIT
    );
}

#[test]
fn ansi_snapshot_preserves_cell_styles_wide_characters_and_colored_spaces() {
    let mut screen = TermScreen::new(30, 3);
    screen.push_bytes(
        "\x1b[1;3;4;5;7;9;31m红色\x1b[0m normal\r\n\x1b[2;38;2;1;2;3;48;5;240m  \x1b[0m".as_bytes(),
    );
    let ansi = screen.snapshot_ansi();
    assert!(ansi.contains("38;5;1"));
    assert!(ansi.contains("38;2;1;2;3;48;5;240"));
    let mut restored = Vt::new(30, 3);
    restored.feed_str(&ansi.replace('\n', "\r\n"));
    for (expected, actual) in screen.vt.view().zip(restored.view()) {
        assert_eq!(actual.cells(), expected.cells());
    }
}

#[test]
fn ansi_snapshot_excludes_original_terminal_control_queries() {
    let mut screen = TermScreen::new(30, 3);
    screen.push_bytes(b"\x1b]52;c;secret\x07\x1b[6n\x1b[31mwarning\x1b[0m");
    let ansi = screen.snapshot_ansi();
    assert!(ansi.contains("warning"));
    assert!(!ansi.contains("secret"));
    assert!(!ansi.contains("[6n"));
}

#[test]
fn ansi_history_preserves_styles_and_clips_oversized_rows_safely() {
    let mut screen = TermScreen::new(20_000, 2);
    screen.push_bytes(format!("\x1b[31m{}\r\nnext\r\n", "你".repeat(8_000)).as_bytes());
    let history = screen.history(0, 10, true);
    assert!(history.truncated);
    assert_eq!(history.lines.len(), 1);
    assert!(history.lines[0].len() < NEW_LINES_MAX_CHARS);
    assert!(history.lines[0].ends_with("\x1b[0m"));
    assert!(history.lines[0].contains("38;5;1"));
}

#[test]
fn matches_output_observes_pending_lines_and_snapshot_without_consuming() {
    let mut screen = TermScreen::new(20, 2);
    screen.push_bytes(b"early\r\nnext\r\nReady? ");
    assert!(screen.matches_output("early\nnext"));
    assert!(screen.matches_output("Ready?"));
    assert!(!screen.matches_output("absent"));
    assert_eq!(screen.take_new_lines().lines, vec!["early", "next"]);
}

#[test]
fn resize_during_alt_screen_resets_delta_baseline() {
    let mut screen = TermScreen::new(20, 3);
    screen.push_bytes(b"\x1b[?1049h");
    screen.push_bytes(b"full-screen ui");
    let first = screen.take_new_lines();
    assert!(first
        .lines
        .iter()
        .any(|line| line.contains("full-screen ui")));
    // resize 后差分基线是旧布局：清空基线让下一次 read 按首次读取语义
    // 整体重报，而不是把 reflow 换行的旧行当新增行灌进增量轨。
    screen.resize(30, 4);
    screen.push_bytes(b" tail");
    let after = screen.take_new_lines();
    assert!(
        after
            .lines
            .iter()
            .any(|line| line.contains("full-screen ui")),
        "基线重置后整屏重报"
    );
}

#[test]
fn take_new_lines_drains_queue_below_caps_without_truncation() {
    let mut screen = TermScreen::new(20, 3);
    for i in 0..50 {
        screen.push_bytes(format!("row-{i:03}\r\n").as_bytes());
    }
    // 50 行 < NEW_LINES_MAX_ROWS(200)：不触及上限，验证全量取出与排空语义；
    // 行/字符两条上限由下方 overflow / char_cap 用例分别覆盖。
    let batch = screen.take_new_lines();
    assert_eq!(batch.lines.len(), 50);
    assert!(!batch.truncated);
    assert!(screen.take_new_lines().lines.is_empty());
}

#[test]
fn pending_queue_overflow_sets_permanent_truncation_flag() {
    let mut screen = TermScreen::new(20, 3);
    // 2501 行 > PENDING_QUEUE_MAX_ROWS(2000)：最老行被丢弃。
    for i in 0..2501 {
        screen.push_bytes(format!("r{i:05}\r\n").as_bytes());
    }
    let batch = screen.take_new_lines();
    assert!(batch.truncated);
    assert_eq!(batch.lines.first().map(String::as_str), Some("r00501"));
}

#[test]
fn take_new_lines_char_cap_truncates_and_keeps_remainder() {
    let mut screen = TermScreen::new(200, 3);
    // 每行 ~150 字符，200 行 > 12000 字符上限。
    let wide = "x".repeat(150);
    for _ in 0..200 {
        screen.push_bytes(format!("{wide}\r\n").as_bytes());
    }
    let batch = screen.take_new_lines();
    assert!(batch.truncated);
    assert!(batch.lines.len() < 200);
    let total: usize = batch.lines.iter().map(String::len).sum();
    assert!(total <= NEW_LINES_MAX_CHARS + 150);
    // 余量仍在队中，下次 read 可取。
    assert!(!screen.take_new_lines().lines.is_empty());
}

#[test]
fn alt_tracker_reset_sequence_returns_to_primary() {
    let mut tracker = AltScreenTracker::new();
    assert!(matches!(
        tracker.push("\x1b[?1049h"),
        Some(AltScreenEvent::Entered)
    ));
    // RIS（ESC c）终端复位：回主屏。
    assert!(matches!(
        tracker.push("junk\x1bc"),
        Some(AltScreenEvent::Left)
    ));
    assert!(!tracker.is_alt());
}

#[test]
fn alt_tracker_nested_enter_reports_no_duplicate() {
    let mut tracker = AltScreenTracker::new();
    assert!(matches!(
        tracker.push("\x1b[?1049h"),
        Some(AltScreenEvent::Entered)
    ));
    // tmux 嵌套（里层程序再进备用屏）：状态不变，不重复报告。
    assert!(tracker.push("\x1b[?47h").is_none());
    assert!(tracker.is_alt());
}

#[test]
fn screen_context_contains_cursor_line_and_tail() {
    let mut screen = TermScreen::new(80, 24);
    screen.push_bytes(
        b"$ sudo apt install htop\r\nReading... Done\r\nDo you want to continue? [Y/n] ",
    );
    let context = screen.screen_context();
    // 光标行（确认提示）与尾部输出都应出现。
    assert!(
        context.contains("[光标行] Do you want to continue? [Y/n]"),
        "{context}"
    );
    assert!(context.contains("Reading... Done"), "{context}");
    assert!(context.contains("$ sudo apt install htop"), "{context}");
}

#[test]
fn screen_context_caps_at_five_tail_lines() {
    let mut screen = TermScreen::new(80, 24);
    for i in 0..10 {
        screen.push_bytes(format!("line-{i}\r\n").as_bytes());
    }
    let context = screen.screen_context();
    assert!(context.contains("line-9"), "{context}");
    assert!(
        !context.contains("line-0"),
        "超出尾部 5 行的旧行不应出现：{context}"
    );
}

#[test]
fn resize_realigns_without_phantom_lines() {
    let mut screen = TermScreen::new(80, 24);
    screen.push_bytes(b"$ long\r\n");
    assert_eq!(screen.take_new_lines().lines, vec!["$ long"]);
    screen.resize(100, 30);
    assert!(screen.take_new_lines().lines.is_empty());
    screen.push_bytes(b"$ after\r\n");
    assert_eq!(screen.take_new_lines().lines, vec!["$ after"]);
}
