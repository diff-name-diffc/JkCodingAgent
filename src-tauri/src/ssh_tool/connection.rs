use std::future::Future;
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

/// 建连失败 + 瞬态分类。`transient = true` 表示网络层瞬态错误
/// （路由抖动、连接被重置等），值得短退避后重试一次；认证被拒、
/// 主机密钥不一致、私钥加载失败等是确定性失败，重试只会掩盖真实原因。
#[derive(Clone, Debug)]
pub(super) struct ConnectError {
    pub(super) message: String,
    pub(super) transient: bool,
}

impl ConnectError {
    fn permanent(message: String) -> Self {
        Self {
            message,
            transient: false,
        }
    }
}

/// 全新建连瞬态失败的退避间隔：路由抖动窗口（Wi-Fi 切换、VPN 重建路由、
/// 唤醒后网络栈就绪）通常几秒内自愈，1.5s 足以跨过大多数窗口。
const TRANSIENT_CONNECT_RETRY_DELAY: Duration = Duration::from_millis(1_500);

/// 判定为瞬态、值得重试一次的 errno 集合。
/// 刻意不含 ECONNREFUSED（端口未开/防火墙 REJECT 是当刻确定性拒绝）；
/// DNS 解析失败（无 raw errno）同样不重试——与主机名拼错无法区分。
fn is_transient_connect_errno(errno: i32) -> bool {
    matches!(
        errno,
        libc::EHOSTUNREACH    // 本机无去往目标的路由（最常见：路由抖动）
            | libc::ENETUNREACH   // 网络不可达
            | libc::ENETDOWN      // 网络接口下线（切换 Wi-Fi 瞬间）
            | libc::ETIMEDOUT     // 内核级连接超时（含握手阶段路由消失）
            | libc::ECONNRESET    // 握手期间被重置（NAT 表项失效常见）
            | libc::ECONNABORTED  // 连接被中止
    )
}

/// TCP 建连/握手阶段的 russh 错误是否属于瞬态网络错误。
/// `client::connect` 把 TcpStream 的 io 错误包成 `Error::IO`（raw errno 保留）。
fn is_transient_connect_error(error: &russh::Error) -> bool {
    match error {
        russh::Error::IO(io_error) => io_error
            .raw_os_error()
            .is_some_and(is_transient_connect_errno),
        _ => false,
    }
}

/// 瞬态失败短退避重试一次的通用骨架：
/// - 第一次成功 → 直接返回；
/// - 第一次确定性失败 → 立即返回，不重试；
/// - 第一次瞬态失败 → 退避 `retry_delay` 后再试一次；二次仍瞬态失败时
///   在消息中标注「已重试过」，提示调用方（模型/用户）不要盲目再重试。
///
/// 独立成泛型函数便于单测（无需真实 Handle 与网络）。
async fn with_one_transient_retry<T, F, Fut>(
    mut attempt: F,
    retry_delay: Duration,
) -> Result<T, ConnectError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, ConnectError>>,
{
    match attempt().await {
        Ok(value) => Ok(value),
        Err(first) if !first.transient => Err(first),
        Err(_first) => {
            tokio::time::sleep(retry_delay).await;
            match attempt().await {
                Ok(value) => Ok(value),
                Err(second) => {
                    let message = if second.transient {
                        format!(
                            "{}（已自动重试一次仍失败，多为瞬时网络中断；若持续失败请检查本机网络/VPN 与服务器可达性）",
                            second.message
                        )
                    } else {
                        second.message
                    };
                    Err(ConnectError {
                        message,
                        transient: second.transient,
                    })
                }
            }
        }
    }
}

/// 建立并认证一条 SSH 连接：握手（含 TOFU 主机密钥校验）→ 密码/私钥认证。
/// 网络层瞬态错误（No route to host / 连接超时 / 连接被重置等）自动
/// 短退避重试一次；确定性失败（认证、密钥、超时配置）不重试。
pub(super) async fn connect(
    server: &SshServerConfig,
    ssh_db: &SshDb,
) -> Result<Handle<SshClientHandler>, String> {
    with_one_transient_retry(
        || connect_once(server, ssh_db),
        TRANSIENT_CONNECT_RETRY_DELAY,
    )
    .await
    .map_err(|error| error.message)
}

/// 单次建连尝试：TCP + SSH 握手 + 认证，失败按确定性/瞬态分类。
async fn connect_once(
    server: &SshServerConfig,
    ssh_db: &SshDb,
) -> Result<Handle<SshClientHandler>, ConnectError> {
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
            // 应用层超时（SYN 已发出但握手未完成）不重试：每次重试都要
            // 再等一个完整超时窗口，且慢/丢包链路下大概率同样超时。
            return Err(ConnectError::permanent(format!(
                "连接 SSH server {} 超时（{}s 内未完成握手）",
                server.id, server.default_timeout_secs
            )));
        }
        Ok(Err(error)) => {
            // 主机密钥校验拒绝 / 固定记录故障：用 handler 记录的详细原因。
            let transient = is_transient_connect_error(&error);
            if let Some(reason) = reject_reason.lock().clone() {
                return Err(ConnectError::permanent(reason));
            }
            return Err(ConnectError {
                message: sanitize_ssh_error(
                    &format!("连接 SSH server {} 失败", server.id),
                    error,
                ),
                transient,
            });
        }
        Ok(Ok(handle)) => handle,
    };

    match server.auth_method {
        SshAuthMethod::Password => {
            let result = handle
                .authenticate_password(server.username.clone(), server.password.clone())
                .await
                .map_err(|error| {
                    ConnectError::permanent(sanitize_ssh_error("SSH 密码认证失败", error))
                })?;
            if !result.success() {
                return Err(ConnectError::permanent(format!(
                    "SSH server {} 密码认证被拒绝",
                    server.id
                )));
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
            .map_err(|error| ConnectError::permanent(error.to_string()))?
            .map_err(|error| {
                ConnectError::permanent(format!(
                    "加载私钥文件 {display_path} 失败：{}",
                    sanitize_error_text(&error.to_string())
                ))
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
                .map_err(|error| {
                    ConnectError::permanent(sanitize_ssh_error("SSH 密钥认证失败", error))
                })?;
            if !result.success() {
                return Err(ConnectError::permanent(format!(
                    "SSH server {} 密钥认证被拒绝",
                    server.id
                )));
            }
        }
    }
    Ok(handle)
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::sync::Arc as StdArc;

    use parking_lot::Mutex as PlMutex;

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

    #[test]
    fn transient_errno_set_excludes_deterministic_refused() {
        // 瞬态网络状态错误：值得重试
        assert!(is_transient_connect_errno(libc::EHOSTUNREACH));
        assert!(is_transient_connect_errno(libc::ENETUNREACH));
        assert!(is_transient_connect_errno(libc::ETIMEDOUT));
        assert!(is_transient_connect_errno(libc::ECONNRESET));
        // ECONNREFUSED 是当刻确定性拒绝（端口未开/防火墙 REJECT），不重试
        assert!(!is_transient_connect_errno(libc::ECONNREFUSED));
    }

    #[test]
    fn russh_io_error_transient_classification() {
        assert!(is_transient_connect_error(&russh::Error::IO(
            std::io::Error::from_raw_os_error(libc::EHOSTUNREACH)
        )));
        // 无 raw errno 的 io 错误（如 DNS 解析失败）不判瞬态：
        // 与主机名拼错无法区分，重试会掩盖配置问题。
        assert!(!is_transient_connect_error(&russh::Error::IO(
            std::io::Error::new(std::io::ErrorKind::Other, "failed to lookup address")
        )));
        // 非 io 的 russh 错误（协议/密钥类）为确定性失败
        assert!(!is_transient_connect_error(&russh::Error::UnknownKey));
    }

    fn transient_error(message: &str) -> ConnectError {
        ConnectError {
            message: message.to_string(),
            transient: true,
        }
    }

    /// 测试用尝试序列的失败策略。
    #[derive(Clone)]
    enum FailMode {
        Succeed,
        /// 仅第 1 次尝试失败（验证瞬态重试成功路径）
        First(ConnectError),
        /// 每次尝试都失败（验证二次失败不再重试、消息标注）
        Always(ConnectError),
    }

    /// 计数器闭包：每次调用自增计数并按 `FailMode` 决定成败。
    fn counting_attempt(
        counter: StdArc<PlMutex<u32>>,
        mode: FailMode,
    ) -> impl FnMut() -> Pin<Box<dyn Future<Output = Result<u32, ConnectError>> + Send>> {
        move || {
            let counter = counter.clone();
            let mode = mode.clone();
            Box::pin(async move {
                let mut attempt_no = counter.lock();
                *attempt_no += 1;
                let failure = match mode {
                    FailMode::Succeed => None,
                    FailMode::First(error) if *attempt_no == 1 => Some(error),
                    FailMode::Always(error) => Some(error),
                    FailMode::First(_) => None,
                };
                failure.map_or(Ok(7), Err)
            })
        }
    }

    #[tokio::test]
    async fn transient_failure_is_retried_once_then_succeeds() {
        let counter = StdArc::new(PlMutex::new(0));
        let result = with_one_transient_retry(
            counting_attempt(counter.clone(), FailMode::First(transient_error("No route to host"))),
            Duration::ZERO,
        )
        .await;

        assert_eq!(result.unwrap(), 7);
        assert_eq!(*counter.lock(), 2);
    }

    #[tokio::test]
    async fn permanent_failure_is_not_retried() {
        let counter = StdArc::new(PlMutex::new(0));
        let result = with_one_transient_retry(
            counting_attempt(
                counter.clone(),
                FailMode::First(ConnectError::permanent("密码认证被拒绝".into())),
            ),
            Duration::ZERO,
        )
        .await;

        let error = result.unwrap_err();
        assert_eq!(error.message, "密码认证被拒绝");
        assert!(!error.transient);
        assert_eq!(*counter.lock(), 1);
    }

    #[tokio::test]
    async fn transient_failure_twice_reports_retried_hint() {
        let counter = StdArc::new(PlMutex::new(0));
        let result = with_one_transient_retry(
            counting_attempt(
                counter.clone(),
                FailMode::Always(transient_error("No route to host")),
            ),
            Duration::ZERO,
        )
        .await;

        let error = result.unwrap_err();
        assert!(error.transient);
        assert!(error.message.starts_with("No route to host"));
        assert!(error.message.contains("已自动重试一次仍失败"));
        assert_eq!(*counter.lock(), 2);
    }

    #[tokio::test]
    async fn success_on_first_attempt_skips_retry() {
        let counter = StdArc::new(PlMutex::new(0));
        let result =
            with_one_transient_retry(counting_attempt(counter.clone(), FailMode::Succeed), Duration::ZERO).await;

        assert_eq!(result.unwrap(), 7);
        assert_eq!(*counter.lock(), 1);
    }
}
