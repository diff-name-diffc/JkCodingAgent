use super::*;
use crate::test_util::TempDirGuard;

#[test]
fn project_run_paths_keep_the_global_runtime_layout() {
    let root = Path::new("application-data");
    let paths = run_paths(root, "run-one", None);
    assert_eq!(paths.root, root.join("python-runner"));
    assert_eq!(paths.venv, root.join("python-runner/venv"));
    assert_eq!(paths.run_dir, root.join("python-runner/runs/run-one"));
    assert_eq!(paths.main_py, paths.run_dir.join("main.py"));
    assert!(paths.temp_dir.is_none());
}

#[test]
fn chat_run_paths_keep_scripts_and_temp_files_in_the_session_but_share_the_venv() {
    let root = Path::new("application-data");
    let workspace = ChatWorkspace {
        session_root: root.join("plain-chat-browser/session-one"),
        project_roots: vec![PathBuf::from("shared-project")],
    };
    let paths = run_paths(root, "run-one", Some(&workspace));
    assert_eq!(
        paths.run_dir,
        workspace.session_root.join("python-runs/run-one")
    );
    assert_eq!(paths.main_py, paths.run_dir.join("main.py"));
    assert_eq!(paths.temp_dir, Some(paths.run_dir.join("tmp")));
    assert_eq!(paths.venv, root.join("python-runner/venv"));
    let mut command = Command::new("python");
    configure_temp_environment(&mut command, paths.temp_dir.as_deref());
    for name in ["TMPDIR", "TMP", "TEMP"] {
        let actual = command
            .as_std()
            .get_envs()
            .find(|(key, _)| *key == name)
            .unwrap();
        assert_eq!(actual.1, paths.temp_dir.as_deref().map(Path::as_os_str));
    }
    let mut project_command = Command::new("python");
    configure_temp_environment(&mut project_command, None);
    assert_eq!(project_command.as_std().get_envs().count(), 0);
}

#[cfg(unix)]
#[test]
fn chat_python_run_files_cannot_follow_a_symlink_outside_the_session() {
    let dir = TempDirGuard::new("python-session-symlink");
    let session_root = dir.path().join("session");
    let shared = dir.path().join("shared");
    std::fs::create_dir(&session_root).unwrap();
    std::fs::create_dir(&shared).unwrap();
    std::os::unix::fs::symlink(&shared, session_root.join("python-runs")).unwrap();
    let workspace = ChatWorkspace {
        session_root: session_root.canonicalize().unwrap(),
        project_roots: vec![shared.canonicalize().unwrap()],
    };
    assert!(prepare_paths(dir.path(), "run-one", Some(&workspace)).is_err());
    assert!(!shared.join("run-one").exists());
}
