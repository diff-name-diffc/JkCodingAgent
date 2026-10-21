//! apply-patch 引擎测试：用例移植自 codex `parser.rs` / `seek_sequence.rs` /
//! `file_update_tests.rs` / `lib.rs` 内嵌测试，断言按本地引擎形态改写
//! （std::fs + TempDirGuard，无 ExecutorFileSystem 抽象）。

use super::{derive_new_contents, parse_patch, seek_sequence, Hunk, ParseError, UpdateFileChunk};
use crate::test_util::TempDirGuard;
use std::path::PathBuf;

fn wrap_patch(body: &str) -> String {
    format!("*** Begin Patch\n{body}\n*** End Patch")
}

#[test]
fn parse_rejects_missing_boundaries() {
    assert_eq!(
        parse_patch("bad"),
        Err(ParseError::InvalidPatchError(
            "The first line of the patch must be '*** Begin Patch'".to_string()
        ))
    );
    assert_eq!(
        parse_patch("*** Begin Patch\nbad"),
        Err(ParseError::InvalidPatchError(
            "The last line of the patch must be '*** End Patch'".to_string()
        ))
    );
}

#[test]
fn parse_full_patch_with_all_hunk_kinds() {
    let hunks = parse_patch(
        "*** Begin Patch\n\
         *** Add File: path/add.py\n\
         +abc\n\
         +def\n\
         *** Delete File: path/delete.py\n\
         *** Update File: path/update.py\n\
         *** Move to: path/update2.py\n\
         @@ def f():\n\
         -    pass\n\
         +    return 123\n\
         *** End Patch",
    )
    .unwrap();
    assert_eq!(
        hunks,
        vec![
            Hunk::AddFile {
                path: PathBuf::from("path/add.py"),
                contents: "abc\ndef\n".to_string(),
            },
            Hunk::DeleteFile {
                path: PathBuf::from("path/delete.py"),
            },
            Hunk::UpdateFile {
                path: PathBuf::from("path/update.py"),
                move_path: Some(PathBuf::from("path/update2.py")),
                chunks: vec![UpdateFileChunk {
                    change_context: Some("def f():".to_string()),
                    old_lines: vec!["    pass".to_string()],
                    new_lines: vec!["    return 123".to_string()],
                    context_line_indices: vec![],
                    is_end_of_file: false,
                }],
            },
        ]
    );
}

#[test]
fn parse_update_without_at_at_header_treats_context_prefix() {
    let hunks = parse_patch(
        r#"*** Begin Patch
*** Update File: file2.py
 import foo
+bar
*** End Patch"#,
    )
    .unwrap();
    assert_eq!(
        hunks,
        vec![Hunk::UpdateFile {
            path: PathBuf::from("file2.py"),
            move_path: None,
            chunks: vec![UpdateFileChunk {
                change_context: None,
                old_lines: vec!["import foo".to_string()],
                new_lines: vec!["import foo".to_string(), "bar".to_string()],
                context_line_indices: vec![(0, 0)],
                is_end_of_file: false,
            }],
        }]
    );
}

#[test]
fn parse_preserves_end_of_file_marker() {
    let hunks = parse_patch(
        "*** Begin Patch\n*** Update File: file.txt\n@@\n+quux\n*** End of File\n*** End Patch",
    )
    .unwrap();
    assert_eq!(
        hunks,
        vec![Hunk::UpdateFile {
            path: PathBuf::from("file.txt"),
            move_path: None,
            chunks: vec![UpdateFileChunk {
                change_context: None,
                old_lines: Vec::new(),
                new_lines: vec!["quux".to_string()],
                context_line_indices: vec![],
                is_end_of_file: true,
            }],
        }]
    );
}

#[test]
fn parse_empty_update_hunk_is_rejected() {
    assert_eq!(
        parse_patch("*** Begin Patch\n*** Update File: test.py\n*** End Patch"),
        Err(ParseError::InvalidHunkError {
            message: "Update file hunk for path 'test.py' is empty".to_string(),
            line_number: 2,
        })
    );
}

#[test]
fn parse_empty_patch_has_no_hunks() {
    assert_eq!(
        parse_patch("*** Begin Patch\n*** End Patch").unwrap(),
        vec![]
    );
}

#[test]
fn parse_strips_heredoc_wrapper_leniently() {
    let patch_text = r#"*** Begin Patch
*** Update File: file2.py
 import foo
+bar
*** End Patch"#;
    for wrapper in ["<<EOF", "<<'EOF'", "<<\"EOF\""] {
        let wrapped = format!("{wrapper}\n{patch_text}\nEOF\n");
        let hunks = parse_patch(&wrapped)
            .unwrap_or_else(|error| panic!("{wrapper} 包裹应解析成功：{error}"));
        assert_eq!(hunks.len(), 1, "{wrapper}");
    }
    // 引号不配对的包裹不剥离。
    let mismatched = format!("<<\"EOF'\n{patch_text}\nEOF\n");
    assert!(parse_patch(&mismatched).is_err());
}

#[test]
fn parse_accepts_and_ignores_environment_id_preamble() {
    let hunks = parse_patch(
        "*** Begin Patch\n*** Environment ID: remote\n*** Add File: hello.txt\n+hello\n*** End Patch",
    )
    .unwrap();
    assert_eq!(
        hunks,
        vec![Hunk::AddFile {
            path: PathBuf::from("hello.txt"),
            contents: "hello\n".to_string(),
        }]
    );
}

// ── seek_sequence（移植 codex seek_sequence.rs 测试）─────────────────

fn to_vec(strings: &[&str]) -> Vec<String> {
    strings.iter().map(ToString::to_string).collect()
}

#[test]
fn seek_finds_exact_sequence() {
    let lines = to_vec(&["foo", "bar", "baz"]);
    let pattern = to_vec(&["bar", "baz"]);
    assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(1));
}

#[test]
fn seek_matches_ignoring_trailing_whitespace() {
    let lines = to_vec(&["foo   ", "bar\t\t"]);
    let pattern = to_vec(&["foo", "bar"]);
    assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
}

#[test]
fn seek_matches_ignoring_surrounding_whitespace() {
    let lines = to_vec(&["    foo   ", "   bar\t"]);
    let pattern = to_vec(&["foo", "bar"]);
    assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
}

#[test]
fn seek_normalizes_unicode_punctuation() {
    // 原文含 EN DASH 与 NON-BREAKING HYPHEN，补丁用 ASCII 连字符。
    let lines = to_vec(&["import asyncio  # local import \u{2013} avoids top\u{2011}level dep"]);
    let pattern = to_vec(&["import asyncio  # local import - avoids top-level dep"]);
    assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
}

#[test]
fn seek_pattern_longer_than_input_returns_none() {
    let lines = to_vec(&["just one line"]);
    let pattern = to_vec(&["too", "many", "lines"]);
    assert_eq!(seek_sequence(&lines, &pattern, 0, false), None);
}

#[test]
fn seek_eof_anchors_pattern_at_file_end() {
    let lines = to_vec(&["dup", "x", "dup"]);
    let pattern = to_vec(&["dup"]);
    // eof=true 优先锚定末尾。
    assert_eq!(seek_sequence(&lines, &pattern, 0, true), Some(2));
    assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
}

/// 构造变更块：`context` 为上下文行索引对，`eof` 标记文件末尾锚定。
fn chunk(old: &[&str], new: &[&str], context: &[(usize, usize)], eof: bool) -> UpdateFileChunk {
    UpdateFileChunk {
        change_context: None,
        old_lines: to_vec(old),
        new_lines: to_vec(new),
        context_line_indices: context.to_vec(),
        is_end_of_file: eof,
    }
}

// ── derive_new_contents（移植 codex lib.rs / file_update_tests.rs）────

#[test]
fn update_replaces_line_with_context() {
    let updated = derive_new_contents(
        "foo\nbar\n",
        "update.txt",
        &[chunk(&["foo", "bar"], &["foo", "baz"], &[(0, 0)], false)],
    )
    .unwrap();
    assert_eq!(updated, "foo\nbaz\n");
}

#[test]
fn multiple_chunks_apply_to_disjoint_regions() {
    // codex test_multiple_update_chunks_apply_to_single_file。
    let chunks = vec![
        chunk(&["foo", "bar"], &["foo", "BAR"], &[(0, 0)], false),
        chunk(&["baz", "qux"], &["baz", "QUX"], &[(0, 0)], false),
    ];
    let updated = derive_new_contents("foo\nbar\nbaz\nqux\n", "multi.txt", &chunks).unwrap();
    assert_eq!(updated, "foo\nBAR\nbaz\nQUX\n");
}

#[test]
fn interleaved_changes_with_end_of_file_append() {
    // codex test_update_file_hunk_interleaved_changes：b→B、e→E、末尾追加 g。
    let original = "a\nb\nc\nd\ne\nf\n";
    let chunks = vec![
        chunk(&["a", "b"], &["a", "B"], &[(0, 0)], false),
        chunk(&["c", "d", "e"], &["c", "d", "E"], &[(0, 0), (1, 1)], false),
        chunk(&["f"], &["f", "g"], &[(0, 0)], true),
    ];
    let updated = derive_new_contents(original, "interleaved.txt", &chunks).unwrap();
    assert_eq!(updated, "a\nB\nc\nd\nE\nf\ng\n");
}

#[test]
fn pure_addition_chunk_followed_by_removal() {
    // codex test_pure_addition_chunk_followed_by_removal：无 old_lines 的块
    // 追加到末尾，随后的块再从文件头替换。
    let original = "line1\nline2\nline3\n";
    let chunks = vec![
        chunk(&[], &["after-context", "second-line"], &[], false),
        chunk(
            &["line1", "line2", "line3"],
            &["line1", "line2-replacement"],
            &[(0, 0)],
            false,
        ),
    ];
    let updated = derive_new_contents(original, "panic.txt", &chunks).unwrap();
    assert_eq!(
        updated,
        "line1\nline2-replacement\nafter-context\nsecond-line\n"
    );
}

#[test]
fn missing_old_lines_report_expected_lines() {
    let error = derive_new_contents(
        "foo\nbar\n",
        "f.txt",
        &[chunk(&["nope"], &["yes"], &[], false)],
    )
    .unwrap_err();
    assert!(
        error.contains("Failed to find expected lines in f.txt"),
        "{error}"
    );
    assert!(error.contains("nope"), "{error}");
}

#[test]
fn change_context_narrows_and_must_exist() {
    let original = "class A:\n    pass\nclass B:\n    pass\n";
    let narrowed = UpdateFileChunk {
        change_context: Some("class B:".to_string()),
        ..chunk(&["    pass"], &["    return 1"], &[], false)
    };
    let updated = derive_new_contents(original, "ctx.txt", &[narrowed]).unwrap();
    assert_eq!(updated, "class A:\n    pass\nclass B:\n    return 1\n");

    let missing = UpdateFileChunk {
        change_context: Some("class C:".to_string()),
        ..chunk(&["    pass"], &["    return 1"], &[], false)
    };
    let error = derive_new_contents(original, "ctx.txt", &[missing]).unwrap_err();
    assert!(
        error.contains("Failed to find context 'class C:'"),
        "{error}"
    );
}

#[test]
fn crlf_file_preserves_untouched_endings() {
    // 混合行尾：未触及行保留 CRLF，替换行采用文件首选行尾。
    let original = "one\r\ntwo\r\nthree\r\n";
    let updated = derive_new_contents(
        original,
        "crlf.txt",
        &[chunk(&["two"], &["TWO"], &[], false)],
    )
    .unwrap();
    assert_eq!(updated, "one\r\nTWO\r\nthree\r\n");
}

#[test]
fn trailing_empty_line_sentinel_retry() {
    // 末尾空串哨兵：old_lines 末尾的空行代表终止换行，去掉后仍能定位。
    let original = "keep\nold\n";
    let updated = derive_new_contents(
        original,
        "sentinel.txt",
        &[chunk(&["old", ""], &["new", ""], &[], true)],
    )
    .unwrap();
    assert_eq!(updated, "keep\nnew\n");
}

// ── 端到端文件应用（经 apply_patch_files，验证提交语义）───────────────

#[tokio::test]
async fn end_to_end_add_update_delete_move() {
    let dir = TempDirGuard::new("apply-patch-e2e");
    std::fs::write(dir.path().join("update.txt"), "foo\nbar\n").unwrap();
    std::fs::write(dir.path().join("src.txt"), "line\n").unwrap();
    std::fs::write(dir.path().join("del.txt"), "x\n").unwrap();

    let hunks = parse_patch(&wrap_patch(&format!(
        "*** Add File: added.txt\n+new content\n\
         *** Update File: update.txt\n@@\n foo\n-bar\n+baz\n\
         *** Update File: src.txt\n*** Move to: moved.txt\n@@\n-line\n+line2\n\
         *** Delete File: del.txt"
    )))
    .unwrap();

    let base = dir.path().to_path_buf();
    let summary =
        super::super::apply_patch::apply_hunks_to_workspace(&hunks, dir.path(), move |raw| {
            Ok(base.join(raw))
        })
        .await
        .unwrap();

    assert_eq!(summary.added, vec!["added.txt"]);
    assert_eq!(summary.modified, vec!["update.txt", "moved.txt"]);
    assert_eq!(summary.deleted, vec!["del.txt"]);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("added.txt")).unwrap(),
        "new content\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("update.txt")).unwrap(),
        "foo\nbaz\n"
    );
    assert!(!dir.path().join("src.txt").exists());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("moved.txt")).unwrap(),
        "line2\n"
    );
    assert!(!dir.path().join("del.txt").exists());
}

#[tokio::test]
async fn compute_failure_leaves_filesystem_untouched() {
    let dir = TempDirGuard::new("apply-patch-fail");
    std::fs::write(dir.path().join("good.txt"), "foo\nbar\n").unwrap();
    std::fs::write(dir.path().join("bad.txt"), "totally different\n").unwrap();

    let hunks = parse_patch(&wrap_patch(
        "*** Update File: good.txt\n@@\n-bar\n+baz\n\
         *** Update File: bad.txt\n@@\n-not-there\n+new",
    ))
    .unwrap();

    let base = dir.path().to_path_buf();
    let error =
        super::super::apply_patch::apply_hunks_to_workspace(&hunks, dir.path(), move |raw| {
            Ok(base.join(raw))
        })
        .await
        .unwrap_err();
    assert!(error.contains("Failed to find expected lines"), "{error}");
    // 先计算后提交：good.txt 不应被改动。
    assert_eq!(
        std::fs::read_to_string(dir.path().join("good.txt")).unwrap(),
        "foo\nbar\n"
    );
}

#[tokio::test]
async fn delete_missing_file_reports_error() {
    let dir = TempDirGuard::new("apply-patch-del");
    let hunks = parse_patch(&wrap_patch("*** Delete File: missing.txt")).unwrap();
    let base = dir.path().to_path_buf();
    let error =
        super::super::apply_patch::apply_hunks_to_workspace(&hunks, dir.path(), move |raw| {
            Ok(base.join(raw))
        })
        .await
        .unwrap_err();
    assert!(error.contains("missing.txt"), "{error}");
}
