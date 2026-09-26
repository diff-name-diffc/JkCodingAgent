//! 唯一保存入口：超阈值低损压缩、严格 mime 映射，写入
//! `chat-images/{workspace_id}/{image_id}.{ext}`，并登记 chat_images 索引行
//! （message_id 为 NULL，消息落库时由 insert_chat_images 绑定）。
//!
//! 阻塞工作持有运行租约，取消在本地写入边界检查。
//!
//! 失败分工（文件是事实源，索引可由消息路径补齐——发送前校验与消息落库都按
//! 文件系统解析 image_id，并在落库时重新登记索引）：
//! - Cancelled：删除已写盘文件并返回 Cancelled（不会被任何消息引用，避免垃圾）；
//! - 登记失败：**保留文件**、留痕后返回 Ok（DB 瞬时 busy/locked 不得丢弃已生成
//!   或已压缩的图片字节）；
//! - 回收删除失败：仅留痕，始终返回 Cancelled，不让回收的 io 错误覆盖根因。
use super::*;

/// `cancel_rx` 由调用方（工具边界 / 命令边界，即唯一处于 agent 循环
/// task-local 作用域的一层）显式传入：本层只消费取消信号，不反向依赖
/// agent 循环。None 表示无取消源，行为与未取消一致。
pub(crate) async fn save_image(
    db: &crate::agent::db::DispatcherDb,
    params: SaveChatImageParams<'_>,
    cancel_rx: Option<tokio::sync::watch::Receiver<bool>>,
) -> ChatImageResult<SavedChatImage> {
    let db = db.clone();
    let workspace = params.workspace_id.to_string();
    let mime = params.mime_type.to_string();
    let source = params.source.to_string();
    let prompt = params.generation_prompt.map(str::to_string);
    let (width, height, bytes) = (params.width, params.height, params.bytes);
    let lease = crate::agent::ActiveRunHandle::current();
    tokio::task::spawn_blocking(move || {
        let _lease = lease;
        let cancelled = || {
            cancel_rx
                .as_ref()
                .is_some_and(|rx| *rx.borrow() || rx.has_changed().is_err())
        };
        if cancelled() {
            return Err(ChatImageError::Cancelled);
        }
        let image_id = uuid::Uuid::new_v4().to_string();
        let (ext, image_bytes) = if bytes.len() >= COMPRESS_THRESHOLD {
            let compressed = compress_image_bytes(&bytes, MAX_COMPRESS_DIM);
            if compressed.len() < bytes.len() {
                ("jpg", compressed)
            } else {
                (ext_for_mime(&mime)?, bytes)
            }
        } else {
            (ext_for_mime(&mime)?, bytes)
        };
        let saved_mime = mime_for_ext(ext).expect("MIME 映射完整").to_string();
        let dir = workspace_image_dir(&workspace)?;
        let path = dir.join(format!("{image_id}.{ext}"));
        if cancelled() {
            return Err(ChatImageError::Cancelled);
        }
        std::fs::create_dir_all(&dir).map_err(io_error("创建会话图片目录", &dir))?;
        std::fs::write(&path, &image_bytes).map_err(io_error("写入聊天图片", &path))?;
        let registration = ChatImageRegistration {
            image_id: image_id.clone(),
            workspace_id: workspace,
            width,
            height,
            mime_type: saved_mime.clone(),
            source,
            generation_prompt: prompt,
        };
        if cancelled() {
            // 取消：不会被任何消息引用，删除半成品避免垃圾文件。
            if let Err(error) = std::fs::remove_file(&path) {
                eprintln!("删除已取消的聊天图片失败（{}）：{error}", path.display());
            }
            return Err(ChatImageError::Cancelled);
        }
        if let Err(error) = db.register_chat_image(&registration, &path) {
            // 登记失败：保留文件只留痕——索引可由消息路径补齐，
            // 而丢弃已生成/已压缩的图片字节不可逆。
            eprintln!("登记聊天图片失败（{image_id}）：{error:#}");
        }
        Ok(SavedChatImage {
            image_id,
            mime_type: saved_mime,
        })
    })
    .await?
}
