use super::*;
use crate::test_util::TempDirGuard;

fn fixture() -> (TempDirGuard, ChatWorkspace) {
    let dir = TempDirGuard::new("chat-workspace");
    let session = dir.path().join("session-a");
    let project_a = dir.path().join("project-a");
    let project_b = dir.path().join("project-b");
    for path in [&session, &project_a, &project_b] {
        std::fs::create_dir_all(path).unwrap();
    }
    let workspace = ChatWorkspace::resolve(
        &session,
        &[
            project_a.to_string_lossy().into_owned(),
            project_b.to_string_lossy().into_owned(),
        ],
    )
    .unwrap();
    (dir, workspace)
}

#[test]
fn resolving_a_new_session_does_not_create_temporary_files() {
    let dir = TempDirGuard::new("chat-workspace-new-session");
    let session = dir.path().join("sessions/new");
    let workspace = ChatWorkspace::resolve(&session, &[]).unwrap();
    assert_eq!(
        workspace.session_root,
        dir.path().canonicalize().unwrap().join("sessions/new")
    );
    assert!(!session.exists());
}

#[test]
fn sessions_share_project_files_but_not_temporary_files() {
    let (dir, first) = fixture();
    let second_session = dir.path().join("session-b");
    std::fs::create_dir(&second_session).unwrap();
    let second = ChatWorkspace::resolve(
        &second_session,
        &first
            .project_roots
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
    )
    .unwrap();
    std::fs::write(first.project_roots[1].join("result.txt"), "shared result").unwrap();
    assert_eq!(
        std::fs::read_to_string(second.project_roots[1].join("result.txt")).unwrap(),
        "shared result"
    );
    std::fs::write(first.session_root.join("probe.py"), "temporary").unwrap();
    assert!(resolve_path(
        &second.session_root,
        true,
        &second.project_roots,
        first.session_root.join("probe.py").to_str().unwrap()
    )
    .is_err());
    assert_ne!(
        first.validate_working_directory(None).unwrap(),
        second.validate_working_directory(None).unwrap()
    );
}

#[test]
fn explicit_working_directory_accepts_each_project_and_rejects_outside() {
    let (dir, workspace) = fixture();
    for root in &workspace.project_roots {
        assert_eq!(
            workspace.validate_working_directory(root.to_str()).unwrap(),
            *root
        );
    }
    assert!(workspace
        .validate_working_directory(Some("../project-a"))
        .is_err());
    assert!(workspace
        .validate_working_directory(dir.path().to_str())
        .is_err());
    assert!(workspace
        .validate_working_directory(workspace.project_roots[0].join("missing").to_str())
        .is_err());
}

#[cfg(unix)]
#[test]
fn working_directory_rejects_symlink_escape() {
    let (dir, workspace) = fixture();
    let other = dir.path().join("other-session");
    std::fs::create_dir(&other).unwrap();
    let link = workspace.project_roots[0].join("escape");
    std::os::unix::fs::symlink(other, &link).unwrap();
    assert!(workspace.validate_working_directory(link.to_str()).is_err());
}

#[test]
fn missing_bound_directory_is_an_error_not_a_fallback() {
    let (_dir, workspace) = fixture();
    let paths = workspace
        .project_roots
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    std::fs::remove_dir(&workspace.project_roots[0]).unwrap();
    assert!(ChatWorkspace::resolve(&workspace.session_root, &paths).is_err());
}

#[test]
fn fixed_prompt_distinguishes_outputs_and_temporary_files_for_all_roots() {
    let (_dir, workspace) = fixture();
    let prompt = workspace.render_prompt();
    assert!(prompt.contains("普通聊天目录规则（固定）"));
    assert!(prompt.contains("一次性测试文件"));
    assert!(prompt.contains("正式产物目录、成果目录"));
    assert!(prompt.contains("以本节为准"));
    assert!(prompt.contains("没有隐含的默认工程"));
    for root in &workspace.project_roots {
        assert!(prompt.contains(&quoted_path(root)));
    }
    let unbound = ChatWorkspace {
        project_roots: vec![],
        ..workspace
    };
    assert!(unbound.render_prompt().contains("当前分类未绑定工程目录"));
}
