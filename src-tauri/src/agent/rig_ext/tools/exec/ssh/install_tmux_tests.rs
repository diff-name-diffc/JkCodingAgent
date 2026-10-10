use std::sync::Arc;

use parking_lot::Mutex;
use rig::tool::{PortableDynamicTool, ToolErrorKind, ToolExecutionError, ToolOutput};
use serde_json::{json, Value};

use super::{ssh_tmux_install_tool, INSTALL_TMUX_COMMAND};

fn mock_exec(
    result: Result<ToolOutput, ToolExecutionError>,
) -> (PortableDynamicTool, Arc<Mutex<Vec<Value>>>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let recorded = calls.clone();
    let tool = PortableDynamicTool::new("ssh_exec", "测试执行器", json!({}), move |args| {
        recorded.lock().push(args);
        let result = result.clone();
        Box::pin(async move { result })
    });
    (ssh_tmux_install_tool(tool), calls)
}

#[tokio::test]
async fn delegates_fixed_script_and_explicit_options_to_ssh_exec() {
    let output = ToolOutput::json(json!({"exit_code": 0, "stdout": "tmux 3.5"}));
    let (tool, calls) = mock_exec(Ok(output.clone()));
    for (options, sudo, timeout) in [
        (json!({}), false, 300),
        (json!({"sudo": true, "timeout_secs": 17}), true, 17),
    ] {
        let mut args = json!({"server_id": "server", "session_id": "session"});
        args.as_object_mut()
            .unwrap()
            .extend(options.as_object().unwrap().clone());
        assert_eq!(tool.execute(args).await.unwrap(), output);
        assert_eq!(
            calls.lock().last().unwrap(),
            &json!({
                "server_id": "server", "session_id": "session",
                "command": INSTALL_TMUX_COMMAND, "sudo": sudo,
                "timeout_secs": timeout,
            })
        );
    }
    assert_eq!(calls.lock().len(), 2);
}

#[tokio::test]
async fn rejects_invalid_input_and_command_overrides_before_delegating() {
    let (tool, calls) = mock_exec(Ok(ToolOutput::text("不应执行")));
    let base = json!({"server_id": "server", "session_id": "session"});
    for invalid in [
        Value::Null,
        json!([]),
        json!({}),
        json!({"server_id": "server"}),
        json!({"session_id": "session"}),
        json!({"server_id": 1}),
        json!({"server_id": " "}),
        json!({"session_id": false}),
        json!({"session_id": ""}),
        json!({"sudo": "true"}),
        json!({"sudo": null}),
        json!({"timeout_secs": 0}),
        json!({"timeout_secs": 301}),
        json!({"timeout_secs": -1}),
        json!({"timeout_secs": 1.5}),
        json!({"timeout_secs": "300"}),
        json!({"timeout_secs": null}),
        json!({"command": "arbitrary command"}),
        json!({"stdin": "arbitrary script"}),
    ] {
        // 缺少字段与非对象按原样验证，其余逐项覆盖合法参数。
        let args = if invalid.is_object()
            && invalid.as_object().unwrap().len() == 1
            && invalid.get("server_id") != Some(&json!("server"))
            && invalid.get("session_id") != Some(&json!("session"))
        {
            let mut args = base.clone();
            args.as_object_mut()
                .unwrap()
                .extend(invalid.as_object().unwrap().clone());
            args
        } else {
            invalid
        };
        let error = tool.execute(args.clone()).await.expect_err("非法参数");
        assert_eq!(error.kind(), ToolErrorKind::InvalidArgs, "{args}");
    }
    assert!(calls.lock().is_empty(), "非法输入不得送入 SSH 执行链路");
}

#[tokio::test]
async fn preserves_review_refusal_cancellation_and_unknown_state_errors() {
    for expected in [
        ToolExecutionError::refused("审查拒绝"),
        ToolExecutionError::new(ToolErrorKind::Cancelled, "调用已取消"),
        ToolExecutionError::new(ToolErrorKind::Timeout, "执行超时"),
        ToolExecutionError::other("远端结果未知")
            .with_code("external_state_unknown")
            .with_retryable(false),
    ] {
        let (tool, calls) = mock_exec(Err(expected.clone()));
        let error = tool
            .execute(json!({"server_id": "server", "session_id": "session"}))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), expected.kind());
        assert_eq!(error.message(), expected.message());
        assert_eq!(error.model_output(), expected.model_output());
        assert_eq!(error.code(), expected.code());
        assert_eq!(error.retryable(), expected.retryable());
        assert_eq!(calls.lock().len(), 1, "不得自行重试安装命令");
    }
}

#[cfg(unix)]
mod script {
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::path::Path;
    use std::process::{Command, Output};

    use super::INSTALL_TMUX_COMMAND;
    use crate::test_util::TempDirGuard;

    struct Sandbox {
        dir: TempDirGuard,
    }

    impl Sandbox {
        fn new() -> Self {
            let dir = TempDirGuard::new("tmux-install-script");
            // PATH 只含此目录；所有包管理器都是 stub，永远不触达宿主机安装程序。
            symlink("/bin/sh", dir.path().join("sh")).unwrap();
            Self { dir }
        }

        fn stub(&self, name: &str, body: &str) {
            let path = self.dir.path().join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        fn package_stub(&self, name: &str, failure: Option<i32>) {
            let suffix = match failure {
                Some(code) => format!("printf 'package failed\\n' >&2\nexit {code}"),
                None => "printf 'installed\\n' > \"$TMUX_INSTALL_TEST_ROOT/installed\"".into(),
            };
            self.stub(
                name,
                &format!(
                    "printf '{name}|%s|%s\\n' \"$*\" \"${{DEBIAN_FRONTEND-unset}}\" >> \"$TMUX_INSTALL_TEST_ROOT/calls\"\n{suffix}"
                ),
            );
        }

        fn run(&self) -> Output {
            Command::new("/bin/sh")
                .args(["-c", INSTALL_TMUX_COMMAND])
                .env_clear()
                .env("PATH", self.dir.path())
                .env("TMUX_INSTALL_TEST_ROOT", self.dir.path())
                .output()
                .unwrap()
        }

        fn path(&self) -> &Path {
            self.dir.path()
        }
    }

    #[test]
    fn installed_tmux_only_reports_version() {
        let sandbox = Sandbox::new();
        sandbox.stub("tmux", "printf 'tmux-test 1.0\\n'");
        sandbox.package_stub("apt-get", Some(99));
        let output = sandbox.run();
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout), "tmux-test 1.0\n");
        assert!(!sandbox.path().join("calls").exists());
    }

    #[test]
    fn supported_installers_are_noninteractive_and_verify_installed_tmux() {
        for (manager, expected) in [
            (
                "apt-get",
                "apt-get|install -y --no-install-recommends tmux|noninteractive\n",
            ),
            ("dnf", "dnf|-y install tmux|unset\n"),
        ] {
            let sandbox = Sandbox::new();
            // 尚未安装时 PATH 无 tmux。包管理 stub 只生成本地测试可执行文件。
            sandbox.package_stub(manager, None);
            let installer = sandbox.path().join(manager);
            let body = std::fs::read_to_string(&installer).unwrap();
            std::fs::write(
                &installer,
                format!("{body}\nprintf '#!/bin/sh\\nprintf \"tmux-test installed\\\\n\"\\n' > \"$TMUX_INSTALL_TEST_ROOT/tmux\"\n/bin/chmod +x \"$TMUX_INSTALL_TEST_ROOT/tmux\"\n"),
            )
            .unwrap();
            let output = sandbox.run();
            assert!(output.status.success(), "{:?}", output);
            assert_eq!(
                String::from_utf8_lossy(&output.stdout),
                "tmux-test installed\n"
            );
            assert_eq!(
                std::fs::read_to_string(sandbox.path().join("calls")).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn unsupported_manager_and_failed_install_return_errors_without_fallback() {
        let unsupported = Sandbox::new();
        let output = unsupported.run();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("未找到支持的包管理器"));
        assert!(!unsupported.path().join("calls").exists());

        let failed = Sandbox::new();
        failed.package_stub("apt-get", Some(42));
        failed.package_stub("dnf", None);
        let output = failed.run();
        assert_eq!(output.status.code(), Some(42));
        assert!(String::from_utf8_lossy(&output.stderr).contains("package failed"));
        let calls = std::fs::read_to_string(failed.path().join("calls")).unwrap();
        assert!(calls.starts_with("apt-get|"));
        assert!(!calls.contains("dnf"));
    }

    #[test]
    fn package_success_without_tmux_is_not_reported_as_success() {
        let sandbox = Sandbox::new();
        sandbox.package_stub("apt-get", None);
        let output = sandbox.run();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("tmux"));
    }
}
