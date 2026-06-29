use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine;
use futures::{SinkExt, StreamExt};
use parking_lot::Mutex;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

const DEFAULT_DASHSCOPE_WS_URL: &str = "wss://dashscope.aliyuncs.com/api-ws/v1/inference";
const DEFAULT_DASHSCOPE_INTL_WS_URL: &str = "wss://dashscope-intl.aliyuncs.com/api-ws/v1/inference";
const MAX_PENDING_AUDIO_CHUNKS: usize = 256;

#[derive(Clone, Default)]
pub struct VoiceAsrManager {
    sessions: Arc<Mutex<HashMap<String, VoiceAsrSessionHandle>>>,
}

struct VoiceAsrSessionHandle {
    tx: mpsc::UnboundedSender<VoiceAsrCommand>,
}

enum VoiceAsrCommand {
    AppendAudio { audio_base64: String },
    Finish,
    Cancel,
}

#[derive(Clone)]
pub struct VoiceAsrConfig {
    pub api_key: String,
    pub websocket_url: String,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct VoiceAsrEventPayload {
    workspace_id: String,
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

struct VoiceAsrSessionState {
    workspace_id: String,
    task_id: String,
    started: bool,
    finish_requested: bool,
    finish_sent: bool,
    pending_audio: Vec<String>,
}

impl VoiceAsrSessionState {
    fn new(workspace_id: String) -> Self {
        Self {
            workspace_id,
            task_id: Uuid::new_v4().to_string(),
            started: false,
            finish_requested: false,
            finish_sent: false,
            pending_audio: Vec::new(),
        }
    }
}

impl VoiceAsrManager {
    pub fn start_session(
        &self,
        app: AppHandle,
        workspace_id: String,
        config: VoiceAsrConfig,
    ) -> Result<(), String> {
        self.cancel_session(&workspace_id);

        let (tx, rx) = mpsc::unbounded_channel();
        self.sessions
            .lock()
            .insert(workspace_id.clone(), VoiceAsrSessionHandle { tx });

        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            run_voice_asr_session(manager, app, workspace_id, config, rx).await;
        });

        Ok(())
    }

    pub fn append_audio(&self, workspace_id: &str, audio_base64: String) -> Result<(), String> {
        self.with_session_sender(workspace_id, |tx| {
            tx.send(VoiceAsrCommand::AppendAudio { audio_base64 })
                .map_err(|_| "语音识别会话已关闭".to_string())
        })
    }

    pub fn finish_session(&self, workspace_id: &str) -> Result<(), String> {
        self.with_session_sender(workspace_id, |tx| {
            tx.send(VoiceAsrCommand::Finish)
                .map_err(|_| "语音识别会话已关闭".to_string())
        })
    }

    pub fn cancel_session(&self, workspace_id: &str) {
        let sender = self
            .sessions
            .lock()
            .remove(workspace_id)
            .map(|session| session.tx);
        if let Some(tx) = sender {
            let _ = tx.send(VoiceAsrCommand::Cancel);
        }
    }

    fn remove_session(&self, workspace_id: &str) {
        self.sessions.lock().remove(workspace_id);
    }

    fn with_session_sender(
        &self,
        workspace_id: &str,
        f: impl FnOnce(&mpsc::UnboundedSender<VoiceAsrCommand>) -> Result<(), String>,
    ) -> Result<(), String> {
        let sender = self
            .sessions
            .lock()
            .get(workspace_id)
            .map(|session| session.tx.clone());
        match sender {
            Some(tx) => f(&tx),
            None => Err("当前没有可用的语音识别会话".to_string()),
        }
    }
}

pub fn resolve_dashscope_websocket_url(api_base: &str) -> String {
    if let Ok(value) = std::env::var("DASHSCOPE_REALTIME_API_URL") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    if api_base.contains("dashscope-intl.aliyuncs.com") {
        DEFAULT_DASHSCOPE_INTL_WS_URL.to_string()
    } else {
        DEFAULT_DASHSCOPE_WS_URL.to_string()
    }
}

async fn run_voice_asr_session(
    manager: VoiceAsrManager,
    app: AppHandle,
    workspace_id: String,
    config: VoiceAsrConfig,
    mut rx: mpsc::UnboundedReceiver<VoiceAsrCommand>,
) {
    let result = run_voice_asr_session_inner(&app, &workspace_id, config, &mut rx).await;

    if let Err(message) = result {
        emit_voice_asr_event(&app, &workspace_id, "error", None, Some(message));
    }

    manager.remove_session(&workspace_id);
}

async fn run_voice_asr_session_inner(
    app: &AppHandle,
    workspace_id: &str,
    config: VoiceAsrConfig,
    rx: &mut mpsc::UnboundedReceiver<VoiceAsrCommand>,
) -> Result<(), String> {
    let mut request = config
        .websocket_url
        .into_client_request()
        .map_err(|error| format!("构造实时识别请求失败：{error}"))?;
    let auth_header = HeaderValue::from_str(&format!("bearer {}", config.api_key.trim()))
        .map_err(|error| format!("构造实时识别鉴权头失败：{error}"))?;
    request.headers_mut().insert("Authorization", auth_header);

    let (websocket, _response) = connect_async(request)
        .await
        .map_err(|error| format!("连接实时识别服务失败：{error}"))?;
    let (mut writer, mut reader) = websocket.split();

    let mut state = VoiceAsrSessionState::new(workspace_id.to_string());
    send_run_task(&mut writer, &state.task_id).await?;

    loop {
        tokio::select! {
            command = rx.recv() => {
                match command {
                    Some(VoiceAsrCommand::AppendAudio { audio_base64 }) => {
                        if state.finish_requested {
                            continue;
                        }
                        if state.started {
                            send_audio_chunk(&mut writer, &audio_base64).await?;
                        } else {
                            if state.pending_audio.len() >= MAX_PENDING_AUDIO_CHUNKS {
                                return Err(format!(
                                    "实时识别启动过慢，待发送音频已达到 {MAX_PENDING_AUDIO_CHUNKS} 块上限"
                                ));
                            }
                            state.pending_audio.push(audio_base64);
                        }
                    }
                    Some(VoiceAsrCommand::Finish) => {
                        state.finish_requested = true;
                        maybe_send_finish(&mut writer, &mut state).await?;
                    }
                    Some(VoiceAsrCommand::Cancel) | None => {
                        let _ = writer.close().await;
                        emit_voice_asr_event(app, workspace_id, "cancelled", None, None);
                        return Ok(());
                    }
                }
            }
            message = reader.next() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        let should_finish =
                            handle_server_message(app, &mut writer, &mut state, text.as_ref()).await?;
                        if should_finish {
                            return Ok(());
                        }
                    }
                    Some(Ok(Message::Binary(_))) => {}
                    Some(Ok(Message::Ping(payload))) => {
                        writer.send(Message::Pong(payload)).await.map_err(|error| {
                            format!("回复实时识别心跳失败：{error}")
                        })?;
                    }
                    Some(Ok(Message::Close(_))) => {
                        return Ok(());
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Frame(_))) => {}
                    Some(Err(error)) => {
                        return Err(format!("实时识别连接异常：{error}"));
                    }
                    None => {
                        return Ok(());
                    }
                }
            }
        }
    }
}

async fn handle_server_message(
    app: &AppHandle,
    writer: &mut futures::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    state: &mut VoiceAsrSessionState,
    text: &str,
) -> Result<bool, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|error| format!("解析实时识别响应失败：{error}"))?;
    let event = value
        .pointer("/header/event")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();

    match event {
        "task-started" => {
            state.started = true;
            emit_voice_asr_event(app, &state.workspace_id, "started", None, None);
            flush_pending_audio(writer, state).await?;
            maybe_send_finish(writer, state).await?;
            Ok(false)
        }
        "result-generated" => {
            if let Some(sentence) = value.pointer("/payload/output/sentence") {
                let text = sentence
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let sentence_end = sentence
                    .get("sentence_end")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);

                if !text.is_empty() {
                    emit_voice_asr_event(
                        app,
                        &state.workspace_id,
                        if sentence_end { "final" } else { "partial" },
                        Some(text),
                        None,
                    );
                }
            }
            Ok(false)
        }
        "task-finished" => {
            emit_voice_asr_event(app, &state.workspace_id, "finished", None, None);
            Ok(true)
        }
        "task-failed" => {
            let message = value
                .pointer("/header/error_message")
                .and_then(serde_json::Value::as_str)
                .or_else(|| {
                    value
                        .pointer("/payload/output/message")
                        .and_then(serde_json::Value::as_str)
                })
                .unwrap_or("实时识别任务失败")
                .to_string();
            emit_voice_asr_event(app, &state.workspace_id, "error", None, Some(message));
            Ok(true)
        }
        _ => Ok(false),
    }
}

async fn flush_pending_audio(
    writer: &mut futures::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    state: &mut VoiceAsrSessionState,
) -> Result<(), String> {
    if state.pending_audio.is_empty() {
        return Ok(());
    }

    for audio_base64 in std::mem::take(&mut state.pending_audio) {
        send_audio_chunk(writer, &audio_base64).await?;
    }

    Ok(())
}

async fn maybe_send_finish(
    writer: &mut futures::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    state: &mut VoiceAsrSessionState,
) -> Result<(), String> {
    if !state.started || !state.finish_requested || state.finish_sent {
        return Ok(());
    }

    let finish_task = serde_json::json!({
        "header": {
            "action": "finish-task",
            "task_id": state.task_id,
            "streaming": "duplex"
        },
        "payload": {
            "input": {}
        }
    });
    writer
        .send(Message::Text(finish_task.to_string().into()))
        .await
        .map_err(|error| format!("发送实时识别结束命令失败：{error}"))?;
    state.finish_sent = true;
    Ok(())
}

async fn send_run_task(
    writer: &mut futures::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    task_id: &str,
) -> Result<(), String> {
    let run_task = serde_json::json!({
        "header": {
            "action": "run-task",
            "task_id": task_id,
            "streaming": "duplex"
        },
        "payload": {
            "task_group": "audio",
            "task": "asr",
            "function": "recognition",
            "model": "fun-asr-realtime",
            "parameters": {
                "format": "pcm",
                "sample_rate": 16000,
                "semantic_punctuation_enabled": false
            },
            "input": {}
        }
    });
    writer
        .send(Message::Text(run_task.to_string().into()))
        .await
        .map_err(|error| format!("发送实时识别启动命令失败：{error}"))
}

async fn send_audio_chunk(
    writer: &mut futures::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    audio_base64: &str,
) -> Result<(), String> {
    let audio = base64::engine::general_purpose::STANDARD
        .decode(audio_base64)
        .map_err(|error| format!("解码语音数据失败：{error}"))?;
    writer
        .send(Message::Binary(audio.into()))
        .await
        .map_err(|error| format!("发送语音数据失败：{error}"))
}

fn emit_voice_asr_event(
    app: &AppHandle,
    workspace_id: &str,
    kind: &str,
    text: Option<String>,
    message: Option<String>,
) {
    let _ = app.emit(
        "dispatcher-asr",
        VoiceAsrEventPayload {
            workspace_id: workspace_id.to_string(),
            kind: kind.to_string(),
            text,
            message,
        },
    );
}
