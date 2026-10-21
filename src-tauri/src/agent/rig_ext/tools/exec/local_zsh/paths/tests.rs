use super::*;

fn workspace() -> (crate::test_util::TempDirGuard, ChatWorkspace) {
    let guard = crate::test_util::TempDirGuard::new("local-zsh-directories");
    let base = guard.path().canonicalize().unwrap();
    let session_root = base.join("session");
    let project_root = base.join("project");
    for directory in [&session_root, &project_root] {
        std::fs::create_dir_all(directory).unwrap();
    }
    (
        guard,
        ChatWorkspace {
            session_root,
            project_roots: vec![project_root],
        },
    )
}

#[test]
fn default_chat_directory_preserves_previous_layout() {
    let (_guard, chat) = workspace();
    let paths = prepare(&chat.session_root, Some(&chat), None).unwrap();
    assert_eq!(paths.run_dir, local_zsh_dir(&chat.session_root).unwrap());
    assert_eq!(paths.audit_dir, paths.run_dir);
    assert_eq!(paths.temp_dir, Some(chat.session_root.join(".tmp")));
    assert!(paths.run_dir.is_dir());
    assert!(paths.temp_dir.unwrap().is_dir());
}

#[test]
fn shared_cwd_keeps_runtime_files_in_session() {
    let (_guard, chat) = workspace();
    let cwd = chat.project_roots[0].join("src");
    std::fs::create_dir_all(&cwd).unwrap();
    let paths = prepare(&chat.session_root, Some(&chat), cwd.to_str()).unwrap();
    assert_eq!(paths.run_dir, cwd);
    assert_eq!(paths.audit_dir, local_zsh_dir(&chat.session_root).unwrap());
    assert_eq!(paths.temp_dir, Some(chat.session_root.join(".tmp")));
    assert!(!chat.project_roots[0].join("audit.json").exists());
    assert!(!chat.project_roots[0].join(".tmp").exists());
}

#[test]
fn explicit_cwd_rejects_outside_missing_and_file_paths() {
    let (guard, chat) = workspace();
    let file = chat.project_roots[0].join("file.txt");
    std::fs::write(&file, "file").unwrap();
    for path in [
        guard.path().to_path_buf(),
        chat.project_roots[0].join("missing"),
        file,
    ] {
        assert!(prepare(&chat.session_root, Some(&chat), path.to_str()).is_err());
    }
}

#[test]
fn legacy_project_mode_keeps_fixed_cwd_and_inherited_temp_environment() {
    let (_guard, chat) = workspace();
    let root = &chat.project_roots[0];
    let paths = prepare(root, None, None).unwrap();
    assert_eq!(paths.run_dir, local_zsh_dir(root).unwrap());
    assert_eq!(paths.audit_dir, paths.run_dir);
    assert!(paths.temp_dir.is_none());
    assert!(prepare(root, None, root.to_str()).is_err());
}

#[cfg(unix)]
#[test]
fn session_temp_symlink_cannot_redirect_runtime_files_to_shared_project() {
    let (_guard, chat) = workspace();
    std::os::unix::fs::symlink(&chat.project_roots[0], chat.session_root.join(".tmp")).unwrap();
    assert!(prepare(&chat.session_root, Some(&chat), None).is_err());
}
