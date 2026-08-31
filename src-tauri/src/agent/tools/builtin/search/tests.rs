use super::*;

const BOUNDED_TEST_STDOUT_BYTES: usize = 64 * 1024;

/// 在临时目录中创建相对路径文件，返回目录（测试结束由调用方清理）。
fn make_workspace(name: &str, files: &[&str]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("jk-search-test-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    for file in files {
        let path = dir.join(file);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, "x").unwrap();
    }
    dir
}

#[test]
fn split_grep_line_parses_match_and_context_lines() {
    let workspace = make_workspace("basic", &["src/a.rs"]);
    let mut cache = HashMap::new();

    let parsed = split_grep_line("src/a.rs:12:命中内容", &workspace, &mut cache).unwrap();
    assert_eq!(parsed, ("src/a.rs", 12, true, "命中内容"));

    let parsed = split_grep_line("src/a.rs-11-上下文", &workspace, &mut cache).unwrap();
    assert_eq!(parsed, ("src/a.rs", 11, false, "上下文"));

    // 路径不可证实的行返回 None。
    assert!(split_grep_line("ghost.rs:1:text", &workspace, &mut cache).is_none());

    let _ = fs::remove_dir_all(&workspace);
}

#[test]
fn split_grep_line_prefers_longest_path_when_prefix_is_also_a_file() {
    // 文件名自身含 `:数字:`，且短前缀 `a` 恰好也是文件：
    // 旧实现按首个可证实候选切分会误判为 `a:12`，正文错位成 `b.rs:5:foo`。
    let workspace = make_workspace("colon-path", &["a", "a:12:b.rs"]);
    let mut cache = HashMap::new();

    let parsed = split_grep_line("a:12:b.rs:5:foo", &workspace, &mut cache).unwrap();
    assert_eq!(parsed, ("a:12:b.rs", 5, true, "foo"));

    // 缓存不得固化错误候选：同一行的解析结果保持一致。
    let parsed = split_grep_line("a:12:b.rs:9:bar", &workspace, &mut cache).unwrap();
    assert_eq!(parsed, ("a:12:b.rs", 9, true, "bar"));

    let _ = fs::remove_dir_all(&workspace);
}

#[test]
fn split_grep_line_handles_dashed_dir_names() {
    // 目录名含 `-数字-`：命中行的 `:行:` 候选应先于 `-行-` 候选被证实。
    let workspace = make_workspace("dash-dir", &["logs-2024-1/app.rs"]);
    let mut cache = HashMap::new();

    let parsed = split_grep_line("logs-2024-1/app.rs:7:hit", &workspace, &mut cache).unwrap();
    assert_eq!(parsed, ("logs-2024-1/app.rs", 7, true, "hit"));

    let parsed = split_grep_line("logs-2024-1/app.rs-6-ctx", &workspace, &mut cache).unwrap();
    assert_eq!(parsed, ("logs-2024-1/app.rs", 6, false, "ctx"));

    let _ = fs::remove_dir_all(&workspace);
}

#[test]
fn grep_fallback_exclude_bare_name_covers_files_and_dirs() {
    // rg 的 !target 匹配任意深度的同名文件与目录；grep 只用 --exclude
    // 不跳过同名目录（结果静默偏宽），必须两个参数并用。
    assert_eq!(
        grep_fallback_exclude("target").unwrap(),
        ["--exclude-dir=target", "--exclude=target"]
    );
    assert_eq!(
        grep_fallback_exclude("*.log").unwrap(),
        ["--exclude-dir=*.log", "--exclude=*.log"]
    );
    // gitignore 尾斜杠语义：仅目录
    assert_eq!(
        grep_fallback_exclude("target/").unwrap(),
        ["--exclude-dir=target"]
    );
}

#[test]
fn grep_fallback_exclude_bare_dir_tree_maps_to_exclude_dir() {
    // rg 的 `!name/*` 与 `!name/**` 同义（gitignore 目录剪枝，均排除整棵树），
    // 以 --exclude-dir=name 近似（锚定差异见函数注释）。
    assert_eq!(
        grep_fallback_exclude("dist/**").unwrap(),
        ["--exclude-dir=dist"]
    );
    assert_eq!(
        grep_fallback_exclude("dist/*").unwrap(),
        ["--exclude-dir=dist"]
    );
}

#[test]
fn grep_fallback_exclude_path_patterns_are_relaxed_not_silent() {
    // --exclude-dir 只匹配目录 basename，路径形排除无法表达：
    // 必须注明放宽，而非静默失效（旧实现会产出永不命中的
    // `--exclude-dir=src/generated`）。
    assert!(grep_fallback_exclude("src/generated/**").is_err());
    assert!(grep_fallback_exclude("src/**/*.rs").is_err());
    assert!(grep_fallback_exclude("src/generated").is_err());
    // `*`/`**` 等价于不排除：无参数、无提示
    assert!(grep_fallback_exclude("*").unwrap().is_empty());
    assert!(grep_fallback_exclude("**").unwrap().is_empty());
}

#[test]
fn path_within_allowed_roots_accepts_inside_and_rejects_escape() {
    let workspace_raw = make_workspace("glob-contain", &["src/a.rs"]);
    // canonicalize：macOS 临时目录 /var/... 是 /private/var/... 的符号链接，
    // 与生产路径一致（restrict 开启时 resolve_path 返回 canonical 目录）。
    let workspace = workspace_raw.canonicalize().unwrap();
    let inside = workspace.join("src/**/*.rs");
    assert!(path_within_allowed_roots(
        &inside,
        std::slice::from_ref(&workspace)
    ));

    // `..` 归一化后越出工作区
    let escaped = lexical_normalize(&workspace.join("../../etc/*"));
    assert!(!path_within_allowed_roots(
        &escaped,
        std::slice::from_ref(&workspace)
    ));

    // 绝对路径模式不位于工作区内
    assert!(!path_within_allowed_roots(
        Path::new("/etc/*"),
        std::slice::from_ref(&workspace)
    ));

    let _ = fs::remove_dir_all(&workspace_raw);
}

#[cfg(unix)]
#[test]
fn safe_glob_entries_never_follows_or_returns_symlinks() {
    use std::os::unix::fs::symlink;

    let workspace = make_workspace("glob-symlink", &["local.rs"]);
    let outside = make_workspace("glob-outside", &["secret.rs"]);
    symlink(&outside, workspace.join("outside-link")).unwrap();

    let entries = safe_glob_entries(&workspace, None)
        .map(|entry| {
            entry
                .unwrap()
                .path()
                .strip_prefix(&workspace)
                .unwrap()
                .to_path_buf()
        })
        .collect::<Vec<_>>();
    assert_eq!(entries, [std::path::PathBuf::from("local.rs")]);

    let _ = fs::remove_dir_all(&workspace);
    let _ = fs::remove_dir_all(&outside);
}

#[test]
fn render_grep_fallback_output_marks_file_limit_truncation() {
    let workspace = make_workspace("fallback-trunc", &["a.rs", "b.rs", "c.rs"]);
    let stdout = "a.rs:1:match\nb.rs:1:match\nc.rs:1:match\n";

    let rendered = render_grep_fallback_output(stdout, &workspace, 1, false);
    assert!(rendered.display.contains("a.rs"), "应保留首个命中文件");
    assert!(
        rendered.display.contains("只展示前 1 个命中文件"),
        "超过 max_files 时必须给出截断提示：{}",
        rendered.display
    );
    assert!(
        rendered.display.contains("部分统计"),
        "截断时必须注明匹配数为部分统计：{}",
        rendered.display
    );
    assert!(rendered.truncated);
    assert_eq!(rendered.files.len(), 1);
    assert!(
        !rendered.display.contains("b.rs:1"),
        "超限文件的匹配行应被丢弃"
    );

    let _ = fs::remove_dir_all(&workspace);
}

#[test]
fn append_capped_never_grows_past_limit() {
    let mut output = vec![1, 2];
    assert!(append_capped(&mut output, &[3, 4, 5], 4));
    assert_eq!(output, [1, 2, 3, 4]);
    assert!(append_capped(&mut output, &[6], 4));
    assert_eq!(output.len(), 4);
}

#[test]
fn grep_queries_share_the_call_stdout_budget() {
    assert_eq!(grep_stdout_budget_per_query(1), MAX_GREP_STDOUT_BYTES);
    assert_eq!(grep_stdout_budget_per_query(2), MAX_GREP_STDOUT_BYTES);

    for query_count in 1..=16 {
        let per_query = grep_stdout_budget_per_query(query_count);
        assert!(per_query <= MAX_GREP_STDOUT_BYTES);
        assert!(per_query * query_count <= MAX_GREP_CALL_STDOUT_BYTES);
    }
    assert_eq!(grep_stdout_budget_per_query(16), 1024 * 1024);
}

/// 由下一个测试作为独立子进程精确调用；普通测试运行时不产生大输出。
#[test]
fn bounded_command_child_writes_past_limit() {
    if std::env::var_os("JK_SEARCH_BOUNDED_OUTPUT_CHILD").is_none() {
        return;
    }
    use std::io::Write as _;
    let chunk = vec![b'x'; BOUNDED_TEST_STDOUT_BYTES];
    let mut stdout = std::io::stdout().lock();
    for _ in 0..=4 {
        stdout.write_all(&chunk).unwrap();
    }
    stdout.flush().unwrap();
}

#[tokio::test]
async fn bounded_command_kills_child_at_stdout_limit() {
    let executable = std::env::current_exe().unwrap();
    let mut command = Command::new(executable);
    command
        .arg("--exact")
        .arg("agent::tools::builtin::search::tests::bounded_command_child_writes_past_limit")
        .arg("--nocapture")
        .env("JK_SEARCH_BOUNDED_OUTPUT_CHILD", "1")
        .kill_on_drop(true);

    let output = run_bounded_search_command(command, BOUNDED_TEST_STDOUT_BYTES)
        .await
        .unwrap();
    assert!(output.truncated);
    assert_eq!(output.stdout.len(), BOUNDED_TEST_STDOUT_BYTES);
    assert!(search_status_is_success(&output));
}
