use super::transport::{quote, supported_version};
use super::*;

fn request() -> SyncDirectory {
    serde_json::from_value(serde_json::json!({
        "ssh_profile":"customer-beijing-01", "source":"/releases/v1.5.0",
        "destination":"/opt/product/releases/v1.5.0"
    }))
    .unwrap()
}

#[test]
fn validates_remote_paths_and_strict_types() {
    let mut req = request();
    assert!(!req.delete && !req.dry_run);
    for path in ["/", "///", "relative", "/opt/../etc", "/opt/./app", "/a\nb"] {
        req.destination = path.into();
        assert!(req.validate().is_err(), "{path:?}");
    }
    req.destination = "/opt/a b/中文/$(touch nope);'".into();
    assert!(req.validate().is_ok());
    let mut value = serde_json::to_value(&req).unwrap();
    value["delete"] = "false".into();
    assert!(serde_json::from_value::<SyncDirectory>(value).is_err());
}

#[test]
fn protects_paths_and_dry_run_delete_semantics() {
    let mut req = request();
    let args = process::arguments(&req, std::path::Path::new("/tmp/a b"), "wrapper");
    assert!(args.contains(&"--protect-args".into()));
    assert!(args.contains(&"--safe-links".into()));
    assert!(!args
        .iter()
        .any(|s| s.starts_with("--delete") || s == "--dry-run"));
    assert_eq!(args[args.len() - 2], "/tmp/a b/");
    assert_eq!(
        args.last().unwrap(),
        "aha-sync:/opt/product/releases/v1.5.0/"
    );
    req.delete = true;
    req.dry_run = true;
    let args = process::arguments(&req, std::path::Path::new("/tmp/a"), "wrapper");
    assert!(args.contains(&"--dry-run".into()) && args.contains(&"--delete-delay".into()));
    assert!(!args
        .iter()
        .any(|s| s == "--copy-links" || s == "--delete-excluded"));
}

#[test]
fn parses_progress_and_rejects_old_rsync() {
    let progress = process::parse_progress(" 1,234,567  42%  10.2MB/s 0:00:03 (xfr#1)").unwrap();
    assert_eq!(progress.transferred_bytes, 1234567);
    assert_eq!(progress.percent, 42);
    for line in ["file 42%", "10 101%", "Total file size: 4", ""] {
        assert!(process::parse_progress(line).is_none());
    }
    assert!(supported_version(
        "rsync  version 3.1.0  protocol version 31"
    ));
    assert!(supported_version("rsync  version 3.4.1"));
    assert!(!supported_version(
        "openrsync: protocol version 29\nrsync version 2.6.9 compatible"
    ));
    assert!(!supported_version("rsync version 3.0.9"));
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::AtomicBool;
    use transport::{Binaries, Transport};

    fn server() -> SshServerConfig {
        serde_json::from_value(serde_json::json!({
            "id":"customer-beijing-01","host":"localhost","username":"tester",
            "password":"test-secret","defaultTimeoutSecs":10,"maxOutputBytes":1024
        }))
        .unwrap()
    }
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("sync-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn script(&self, text: &str) -> PathBuf {
            let path = self.0.join("fake-rsync");
            std::fs::write(&path, format!("#!/bin/sh\n{text}")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            path
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn transport(server: &SshServerConfig, executable: PathBuf) -> Transport {
        Transport::new(
            server,
            "ssh-ed25519 AAAA",
            Binaries {
                rsync: executable,
                ssh: "/usr/bin/ssh".into(),
            },
        )
        .unwrap()
    }

    #[test]
    fn shell_quote_preserves_metacharacters() {
        let value = "a b'$(printf BAD);\"";
        let out = std::process::Command::new("/bin/sh")
            .args(["-c", &format!("printf %s {}", quote(value))])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8(out.stdout).unwrap(), value);
    }

    #[test]
    fn captures_nonzero_partial_failure_and_truncates_output() {
        let tmp = Temp::new();
        let script = tmp.script("printf '\\r 1,234 42%% 1MB/s\\r'; i=0; while [ $i -lt 1500 ]; do printf x; i=$((i+1)); done; printf problem >&2; exit 23\n");
        let server = server();
        let result = process::run(
            &server,
            request(),
            tmp.0.clone(),
            transport(&server, script),
            None,
            Arc::new(AtomicBool::new(false)),
            Arc::new(|_| {}),
        )
        .unwrap();
        assert_eq!(result.exit_code, Some(23));
        assert_eq!(result.stderr, "problem");
        assert!(result.truncated);
        assert_eq!(result.stdout.len(), 1024);
        assert_eq!(result.progress.unwrap().percent, 42);
    }

    #[test]
    fn timeout_kills_descendants_before_they_can_write() {
        let tmp = Temp::new();
        let marker = tmp.0.join("should-not-exist");
        let script = tmp.script(&format!(
            "(sleep 2; touch {}) & wait\n",
            quote(marker.to_str().unwrap())
        ));
        let mut server = server();
        server.default_timeout_secs = 1;
        let result = process::run(
            &server,
            request(),
            tmp.0.clone(),
            transport(&server, script),
            None,
            Arc::new(AtomicBool::new(false)),
            Arc::new(|_| {}),
        )
        .unwrap();
        assert!(result.timed_out);
        std::thread::sleep(Duration::from_millis(1300));
        assert!(!marker.exists());
    }

    #[test]
    fn cancellation_stops_process_and_cleans_credentials() {
        let tmp = Temp::new();
        let script = tmp.script("sleep 20\n");
        let server = server();
        let transport = transport(&server, script);
        let shell = transport.shell.trim_matches('"').to_string();
        let secret = PathBuf::from(&shell).parent().unwrap().join("secret");
        assert_eq!(
            std::fs::metadata(&secret).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let (tx, rx) = watch::channel(false);
        let trigger = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            tx.send(true).unwrap();
        });
        let result = process::run(
            &server,
            request(),
            tmp.0.clone(),
            transport,
            Some(rx),
            Arc::new(AtomicBool::new(false)),
            Arc::new(|_| {}),
        )
        .unwrap();
        trigger.join().unwrap();
        assert!(result.cancelled && !result.timed_out);
        assert!(!secret.exists());
    }

    #[test]
    fn openssh_uses_only_pinned_host_key_and_configured_identity() {
        let server = server();
        let transport = transport(&server, "/usr/bin/true".into());
        let wrapper = transport.shell.trim_matches('"');
        let output = std::process::Command::new(wrapper)
            .args(["-G", "aha-sync"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let config = String::from_utf8(output.stdout).unwrap();
        assert!(config.contains("hostname localhost\n"));
        assert!(config.contains("user tester\n"));
        assert!(config.contains("hostkeyalias aha-sync\n"));
        assert!(config.contains("stricthostkeychecking true\n"));
        assert!(config.contains("identityagent none\n"));
        assert!(config.contains("preferredauthentications password\n"));
        let path = std::path::Path::new(wrapper)
            .parent()
            .unwrap()
            .join("known_hosts");
        assert!(config.contains(&format!("userknownhostsfile {}\n", path.display())));
        assert!(!config.contains("test-secret"));
    }

    #[test]
    fn rejects_endpoint_options_and_cleans_failed_key_setup() {
        let mut server = server();
        for host in [
            "-oProxyCommand=bad",
            "localhost\nProxyCommand=bad",
            "host$(bad)",
            "",
        ] {
            server.host = host.into();
            assert!(transport::validate_endpoint(&server).is_err());
        }
        server.host = "::1".into();
        assert!(transport::validate_endpoint(&server).is_ok());
    }
}
