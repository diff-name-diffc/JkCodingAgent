use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use russh::client::{self, Handle};
use russh::keys::{PrivateKeyWithHashAlg, PublicKey};
use sha2::{Digest, Sha256};

use super::audit::{sanitize_error_text, sanitize_ssh_error};
use super::validation::{expand_key_path, MAX_TIMEOUT_SECS};
use super::{SshAuthMethod, SshDb, SshServerConfig};

/// russh 客户端回调：承担主机密钥 TOFU 校验（首次学习 / 命中放行 / 变更拒绝）。
///
/// `check_server_key` 只能返回 bool，详细拒绝原因通过 `reject_reason` 共享给
/// `connect()`：连接失败时用它替换 russh 的通用报错，保持「指纹不一致…」中文诊断。
pub(super) struct SshClientHandler {
    server_id: String,
    db: SshDb,
    reject_reason: Arc<Mutex<Option<String>>>,
}

impl client::Handler for SshClientHandler {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKey) -> Result<bool, Self::Error> {
        let fingerprint = match host_key_fingerprint(key) {
            Ok(fingerprint) => fingerprint,
            Err(error) => {
                *self.reject_reason.lock() = Some(format!(
                    "SSH server {} 的主机公钥编码失败，已拒绝连接：{error}",
                    self.server_id
                ));
                return Ok(false);
            }
        };
        let db = self.db.clone();
        let server_id = self.server_id.clone();
        // 固定记录的读写在本地 SQLite 上，放入 spawn_blocking 遵守「不阻塞执行器」约定。
        let verdict = tokio::task::spawn_blocking({
            let fingerprint = fingerprint.clone();
            move || -> Result<Option<String>, String> {
                match db.host_key_pin(&server_id)? {
                    Some(pinned) if pinned == fingerprint => Ok(None),
                    Some(pinned) => Ok(Some(format!(
                        "SSH server {server_id} 的主机密钥与已固定指纹不一致（疑似中间人攻击，或服务器已重建）：\n固定指纹：{pinned}\n实际指纹：{fingerprint}\n如确认是合法变更，请在应用的 SSH 设置中重新测试连接并更新指纹。"
                    ))),
                    None => {
                        db.set_host_key_pin(&server_id, &fingerprint)?;
                        Ok(None)
                    }
                }
            }
        })
        .await
        .unwrap_or_else(|error| Err(error.to_string()));
        match verdict {
            Ok(None) => Ok(true),
            Ok(Some(reason)) => {
                *self.reject_reason.lock() = Some(reason);
                Ok(false)
            }
            // fail-closed：固定记录读写失败时拒绝连接，与命令审查门禁同一原则。
            Err(error) => {
                *self.reject_reason.lock() =
                    Some(format!("主机密钥固定记录读写失败，已拒绝连接：{error}"));
                Ok(false)
            }
        }
    }
}

/// 主机公钥指纹：对 SSH wire 编码的公钥 blob 做 SHA-256 并 hex 编码——
/// 与迁移前 libssh2 `host_key_hash(Sha256)` 的 hex 格式一致，
/// 已有 ssh_host_keys 固定记录无需迁移。
fn host_key_fingerprint(key: &PublicKey) -> Result<String, String> {
    let blob = key
        .to_bytes()
        .map_err(|error| format!("公钥编码失败：{error}"))?;
    let digest = Sha256::digest(&blob);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// 建立并认证一条 SSH 连接：握手（含 TOFU 主机密钥校验）→ 密码/私钥认证。
pub(super) async fn connect(
    server: &SshServerConfig,
    ssh_db: &SshDb,
) -> Result<Handle<SshClientHandler>, String> {
    let address = format!("{}:{}", server.host, server.port);
    let reject_reason = Arc::new(Mutex::new(None));
    let handler = SshClientHandler {
        server_id: server.id.clone(),
        db: ssh_db.clone(),
        reject_reason: reject_reason.clone(),
    };
    let config = Arc::new(client::Config {
        // 空闲回收由连接池自己管理，禁用 russh 的非活动断开。
        inactivity_timeout: None,
        nodelay: true,
        ..Default::default()
    });
    let connect_timeout =
        Duration::from_secs(server.default_timeout_secs.clamp(1, MAX_TIMEOUT_SECS));
    let mut handle = match tokio::time::timeout(
        connect_timeout,
        client::connect(config, address, handler),
    )
    .await
    {
        Err(_elapsed) => {
            return Err(format!(
                "连接 SSH server {} 超时（{}s 内未完成握手）",
                server.id, server.default_timeout_secs
            ));
        }
        Ok(Err(error)) => {
            // 主机密钥校验拒绝 / 固定记录故障：用 handler 记录的详细原因。
            if let Some(reason) = reject_reason.lock().clone() {
                return Err(reason);
            }
            return Err(sanitize_ssh_error(
                &format!("连接 SSH server {} 失败", server.id),
                error,
            ));
        }
        Ok(Ok(handle)) => handle,
    };

    match server.auth_method {
        SshAuthMethod::Password => {
            let result = handle
                .authenticate_password(server.username.clone(), server.password.clone())
                .await
                .map_err(|error| sanitize_ssh_error("SSH 密码认证失败", error))?;
            if !result.success() {
                return Err(format!("SSH server {} 密码认证被拒绝", server.id));
            }
        }
        SshAuthMethod::Key => {
            let key_path = expand_key_path(&server.private_key_path);
            let passphrase = if server.private_key_passphrase.is_empty() {
                None
            } else {
                Some(server.private_key_passphrase.clone())
            };
            let display_path = server.private_key_path.clone();
            let key = tokio::task::spawn_blocking(move || {
                russh::keys::load_secret_key(key_path, passphrase.as_deref())
            })
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| {
                format!(
                    "加载私钥文件 {display_path} 失败：{}",
                    sanitize_error_text(&error.to_string())
                )
            })?;
            // RSA 密钥需要按服务端能力选择 hash 算法；其它算法忽略该参数。
            let hash_alg = handle
                .best_supported_rsa_hash()
                .await
                .ok()
                .flatten()
                .flatten();
            let result = handle
                .authenticate_publickey(
                    server.username.clone(),
                    PrivateKeyWithHashAlg::new(Arc::new(key), hash_alg),
                )
                .await
                .map_err(|error| sanitize_ssh_error("SSH 密钥认证失败", error))?;
            if !result.success() {
                return Err(format!("SSH server {} 密钥认证被拒绝", server.id));
            }
        }
    }
    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_key_fingerprint_is_hex_sha256_of_wire_blob() {
        // 锚定真实服务器（103.194.106.119）的 ed25519 公钥：
        // 与 ssh-keyscan 的 key blob 经 sha256 的 hex 结果一致——即迁移前
        // libssh2 host_key_hash(Sha256) 的格式，已有固定记录保持有效。
        let key = PublicKey::from_openssh(
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIORk9fRyJPh4Mnku0U5uWmIvgKvDv5PKdo/MgwX5WdTZ",
        )
        .unwrap();
        assert_eq!(
            host_key_fingerprint(&key).unwrap(),
            "e2720a9982b17cc99ac153b54cecff11944a30aceb5f16bba40a00a81d3498ca"
        );
    }
}
