use super::super::{SshAuthMethod, SshServerConfig};
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(test)]
pub(super) use super::binaries::supported_version;
pub(super) use super::binaries::{find_binaries, Binaries};

pub(super) fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(super) struct Transport {
    dir: PathBuf,
    pub binaries: Binaries,
    pub shell: String,
}
impl Drop for Transport {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_dir_all(&self.dir) {
            eprintln!("[sync_directory] 清理临时认证文件失败：{e}");
        }
    }
}

impl Transport {
    pub(super) fn new(
        server: &SshServerConfig,
        key: &str,
        binaries: Binaries,
    ) -> Result<Self, String> {
        validate_endpoint(server)?;
        let dir = std::env::temp_dir().join(format!("aha-rsync-{}", uuid::Uuid::new_v4()));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&dir)
            .map_err(|e| format!("创建 rsync 私有目录失败：{e}"))?;
        let mut transport = Self {
            dir,
            binaries,
            shell: String::new(),
        };
        let known_hosts = transport.dir.join("known_hosts");
        write_private(&known_hosts, &format!("aha-sync {key}\n"), false)?;
        let secret = match server.auth_method {
            SshAuthMethod::Password => &server.password,
            SshAuthMethod::Key => &server.private_key_passphrase,
        };
        write_private(&transport.dir.join("secret"), secret, false)?;
        // 口令不进入命令和环境变量，仅写入短命、0700 目录内的 0600 文件。
        write_private(
            &transport.dir.join("askpass"),
            "#!/bin/sh\nexec /bin/cat \"$AHA_SYNC_SECRET_FILE\"\n",
            true,
        )?;
        let mut ssh = vec![
            transport.binaries.ssh.to_string_lossy().into_owned(),
            "-F".into(),
            "/dev/null".into(),
            "-T".into(),
            "-p".into(),
            server.port.to_string(),
            "-l".into(),
            server.username.clone(),
        ];
        let known_hosts = known_hosts.to_string_lossy();
        if known_hosts.contains(['"', '\n', '\r']) {
            return Err("临时目录含 OpenSSH 配置不支持的字符".into());
        }
        for option in [
            format!("Hostname={}", server.host),
            "HostKeyAlias=aha-sync".into(),
            format!("UserKnownHostsFile=\"{known_hosts}\""),
            "GlobalKnownHostsFile=/dev/null".into(),
            "StrictHostKeyChecking=yes".into(),
            "UpdateHostKeys=no".into(),
            "CheckHostIP=no".into(),
            "IdentityAgent=none".into(),
            "IdentitiesOnly=yes".into(),
            "ForwardAgent=no".into(),
            "ClearAllForwardings=yes".into(),
            "NumberOfPasswordPrompts=1".into(),
            "ConnectTimeout=30".into(),
            "ServerAliveInterval=15".into(),
            "ServerAliveCountMax=2".into(),
        ] {
            ssh.extend(["-o".into(), option]);
        }
        match server.auth_method {
            SshAuthMethod::Password => ssh.extend([
                "-o".into(),
                "PreferredAuthentications=password".into(),
                "-o".into(),
                "PubkeyAuthentication=no".into(),
            ]),
            SshAuthMethod::Key => {
                let key_path = super::super::validation::expand_key_path(&server.private_key_path)
                    .canonicalize()
                    .map_err(|e| format!("解析 SSH 私钥路径失败：{e}"))?;
                ssh.extend([
                    "-o".into(),
                    "PreferredAuthentications=publickey".into(),
                    "-i".into(),
                    key_path.to_string_lossy().into_owned(),
                ]);
            }
        }
        let wrapper = transport.dir.join("ssh");
        write_private(
            &wrapper,
            &format!(
                "#!/bin/sh\nexec {} \"$@\"\n",
                ssh.iter().map(|s| quote(s)).collect::<Vec<_>>().join(" ")
            ),
            true,
        )?;
        // rsync -e 使用自己的分词器，双引号内的双引号以加倍转义。
        transport.shell = format!("\"{}\"", wrapper.to_string_lossy().replace('"', "\"\""));
        Ok(transport)
    }

    pub(super) fn configure(&self, command: &mut Command) {
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("LC_ALL", "C")
            .env("SSH_ASKPASS_REQUIRE", "force")
            .env("SSH_ASKPASS", self.dir.join("askpass"))
            .env("DISPLAY", "aha-sync:0")
            .env("AHA_SYNC_SECRET_FILE", self.dir.join("secret"));
    }
}

fn write_private(path: &Path, data: &str, executable: bool) -> Result<(), String> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(if executable { 0o700 } else { 0o600 });
    }
    #[cfg(not(unix))]
    let _ = executable;
    options
        .open(path)
        .and_then(|mut file| file.write_all(data.as_bytes()))
        .map_err(|e| format!("写入 rsync 私有文件失败：{e}"))
}

pub(super) fn validate_endpoint(server: &SshServerConfig) -> Result<(), String> {
    if server.port == 0
        || server.host.is_empty()
        || server.host.starts_with('-')
        || !server
            .host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-_:".contains(c))
        || server.username.is_empty()
        || server.username.starts_with('-')
        || !server
            .username
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-@".contains(c))
    {
        return Err("SSH host/port/username 不符合 OpenSSH 同步参数要求".into());
    }
    Ok(())
}
