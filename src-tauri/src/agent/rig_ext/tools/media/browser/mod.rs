//! 浏览器智能体工具（内置浏览器自动化）。
//!
//! 入口保留工具注册、导航（open_url）、快照读取（read_text）、视觉分析与
//! sidecar 命令管道；子模块按变化原因划分：
//! - `actions`：点击/输入/按键/等待/关闭等单命令转发工具；
//! - `snapshot`：read_text 快照缓存与行号分页渲染；
//! - `recovery`：错误分类与 LLM 感知的自动恢复；
//! - `file_url`：file:// URL 解析与工作区拘禁（高危安全面）。
//!
//! 迁移自旧 `agent/tools/builtin/browser*`；浏览器管理器经
//! `deps.app_handle` 的 `BrowserManager` state 访问。视觉分析改用
//! `deps.vision_spec`（旧实现回退「聊天 provider + 视觉模型名」的链路
//! 随旧 llm 层退役，视觉槽位解析已内置凭据回退，语义等价）。

mod actions;
mod file_url;
mod recovery;
mod snapshot;

use std::path::PathBuf;

use rig::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use file_url::validate_file_url_within_workspace;
use recovery::{classify_browser_error, handle_browser_error, BrowserErrorKind};
use snapshot::{
    format_snapshot_response, invalidate_cached_snapshot, render_cached_page,
    READ_TEXT_DEFAULT_LINE_LIMIT,
};

use super::super::common::{
    string_arg, u64_arg, usize_arg, with_compression_parameters,
    DEFAULT_FORCE_COMPRESS_AFTER_CHARS,
};
use super::super::deps::RigToolDeps;
use super::super::super::model::PurposeModelSpec;
use super::{data_url_to_image, vision_complete_image};
use crate::browser::{normalize_browser_url, BrowserManager};

const DEFAULT_BROWSER_TIMEOUT_MS: u64 = 60_000;

/// browser_* 工具闭包共享的构造期依赖（全部取自 `RigToolDeps`，
/// 收敛到单一结构便于 `Clone` 进 `'static` 回调）。
#[derive(Clone)]
pub(super) struct BrowserDeps {
    pub(crate) app_handle: Option<AppHandle>,
    pub(crate) workspace_id: String,
    pub(crate) workspace: PathBuf,
    pub(crate) vision_spec: Option<PurposeModelSpec>,
}

impl BrowserDeps {
    fn from(deps: &RigToolDeps) -> Self {
        Self {
            app_handle: deps.app_handle.clone(),
            workspace_id: deps.workspace_id.clone(),
            workspace: deps.workspace.clone(),
            vision_spec: deps.vision_spec.clone(),
        }
    }
}

pub(super) fn browser_tools(deps: &RigToolDeps) -> Vec<PortableDynamicTool> {
    let browser = BrowserDeps::from(deps);
    vec![
        open_url_tool(browser.clone()),
        actions::click_tool(browser.clone()),
        actions::type_tool(browser.clone()),
        actions::press_tool(browser.clone()),
        actions::wait_for_tool(browser.clone()),
        read_text_tool(browser.clone()),
        visual_analyze_tool(browser.clone()),
        actions::close_tool(browser),
    ]
}

fn open_url_tool(deps: BrowserDeps) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "browser_open_url",
        "使用项目级无头浏览器打开 URL。支持浏览器引擎可导航的 URL（包括 http、https、file、data、about 等），会自动启动嵌入式浏览器会话；页面执行画面经屏幕帧流回放到前端（用户可在工具执行轨迹中查看，不会弹出窗口）。注意：file:// URL 仅允许打开当前工作区内的本地文件，工作区之外的路径会被拒绝。",
        with_compression_parameters(
            json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "要打开的完整 URL；支持浏览器引擎可导航的协议，例如 http://、https://、file://、data:、about:。file:// 仅允许当前工作区内的本地文件。" },
                    "timeout": { "type": "integer", "description": "超时时间，单位毫秒，默认 60000", "minimum": 1 }
                },
                "required": ["url"]
            }),
            false,
            DEFAULT_FORCE_COMPRESS_AFTER_CHARS,
            "浏览器操作结果通常较短，默认关闭压缩。",
        ),
        move |args: Value| {
            let deps = deps.clone();
            Box::pin(async move {
                let Some(url) = string_arg(&args, "url") else {
                    return Err(ToolExecutionError::invalid_args("错误：缺少必填参数 url"));
                };
                let url = match normalize_browser_url(url) {
                    Ok(url) => url,
                    Err(error) => {
                        return Err(ToolExecutionError::invalid_args(format!("错误：{error}")))
                    }
                };
                // file: URL 可被用来读取任意本地文件（随后 browser_read_text 会把内容读入
                // 上下文），属于高危面。这里强制解析出本地路径并校验其必须位于当前工作区内，
                // 越界直接拒绝，不受 restrict_to_workspace 全局开关影响（fail-closed）。
                // 按 scheme 判定（而非 `file://` 前缀）：`file:/path` 单斜杠形式同样是合法
                // file URL，浏览器引擎会将其归一化为 file:///path。
                if url
                    .split_once(':')
                    .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("file"))
                {
                    let workspace = deps.workspace.clone();
                    let url_owned = url.clone();
                    let validation = tokio::task::spawn_blocking(move || {
                        validate_file_url_within_workspace(&url_owned, &workspace)
                    })
                    .await;
                    match validation {
                        Ok(Ok(())) => {}
                        Ok(Err(message)) => return Err(ToolExecutionError::refused(message)),
                        Err(error) => {
                            return Err(ToolExecutionError::other(format!(
                                "错误：file:// URL 校验任务失败：{error}"
                            )))
                        }
                    }
                }
                // 即将导航：sidecar 的 ref 映射与缓存快照都会随之失效。提前丢弃缓存，
                // 避免导航后的分页读取返回旧页面内容（即使 open_url 失败，代价也只是
                // 下次分页读取多一次全量抓取）。
                invalidate_cached_snapshot(&deps.workspace_id);
                run_browser_command(
                    &deps,
                    "open_url",
                    json!({ "url": url, "timeout": timeout_arg(&args) }),
                )
                .await
            })
        },
    )
}

fn read_text_tool(deps: BrowserDeps) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "browser_read_text",
        "读取浏览器当前页面或指定 ref 元素的可访问性树文本快照，输出为「行号|内容」格式；快照会为可交互/可定位节点生成 ref，后续浏览器自动化统一使用这些 ref。主文档与 iframe 内元素（以「iframe [frame=N]」小节嵌入快照）都会生成可点击/可输入的 ref。快照较长时超过内联上限（默认 10000 字符）会被截断并注明行位置，此时用 offset/limit 按行号接续读取剩余部分（分页读取的内联上限提高到 20000 字符，一次可读约一两百行）；带行范围的调用读取的是最近一次全量快照（不重新请求页面、ref 保持有效），需要刷新页面状态时省略行范围重新读取。",
        with_compression_parameters(
            json!({
                "type": "object",
                "properties": {
                    "ref": { "type": "string", "description": "可选。读取某个已知 ref 对应元素的局部 Accessibility Tree；不传则读取整个页面并刷新 ref 映射。局部树的行号独立编号。" },
                    "offset": { "type": "integer", "description": "起始行号，从 1 开始。传了 offset/limit 时读取最近一次全量快照的对应行范围，不会重新请求页面，ref 保持有效；需要最新页面状态时省略行范围重新读取。", "minimum": 1 },
                    "limit": { "type": "integer", "description": "最多读取多少行，默认 2000（即读到快照末尾）。内联字符上限默认 10000，显式指定 offset/limit 分页读取时提高到 20000；输出为 行号|内容，超过上限仍会截断并注明行位置，用 offset 从截断行的下一行接续读取即可。", "minimum": 1 },
                    "max_nodes": { "type": "integer", "description": "最多返回的可访问性节点数，默认 600。仅在不带行范围（重新抓取快照）时生效。", "minimum": 1 },
                    "timeout": { "type": "integer", "description": "超时时间，单位毫秒，默认 60000", "minimum": 1 }
                }
            }),
            false,
            DEFAULT_FORCE_COMPRESS_AFTER_CHARS,
            "可访问性树快照经常是后续定位和判断依据，默认关闭压缩；只看页面概览时可开启并写明 compress_intent。",
        ),
        move |args: Value| {
            let deps = deps.clone();
            Box::pin(async move {
                // 与 sidecar 的空 ref 语义对齐：空字符串按未传处理（读取整页）。
                let ref_arg = string_arg(&args, "ref").filter(|value| !value.trim().is_empty());
                let offset = usize_arg(&args, "offset");
                let limit_arg = usize_arg(&args, "limit");
                let has_range = offset.is_some() || limit_arg.is_some();
                let limit = limit_arg.unwrap_or(READ_TEXT_DEFAULT_LINE_LIMIT);

                // 分页读取：全量快照直接命中缓存切片，不重复请求 CDP，也不会刷新
                // sidecar 的 ref 映射（此前下发的 ref 保持有效）。
                if ref_arg.is_none() && has_range {
                    if let Some(page) =
                        render_cached_page(&deps.workspace_id, offset.unwrap_or(1), limit)
                    {
                        return Ok(ToolOutput::text(page));
                    }
                    // 无缓存（sidecar 重启 / 冷启动）：继续走全量读取后按行范围切片。
                }

                match run_browser_command_value(
                    &deps,
                    "read_text",
                    json!({
                        "ref": ref_arg,
                        "maxNodes": u64_arg(&args, "max_nodes").unwrap_or(600).max(1),
                        "timeout": timeout_arg(&args)
                    }),
                )
                .await
                {
                    Ok(value) => Ok(ToolOutput::text(format_snapshot_response(
                        value,
                        &deps.workspace_id,
                        ref_arg.as_deref(),
                        offset.unwrap_or(1),
                        limit,
                    ))),
                    Err(error) => Err(ToolExecutionError::other(
                        handle_browser_error(&deps, error).await,
                    )),
                }
            })
        },
    )
}

fn visual_analyze_tool(deps: BrowserDeps) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "browser_visual_analyze",
        "对浏览器当前可视页面进行轻量视觉理解。工具会在内部截图，并调用已配置的视觉模型按指令分析页面；不会把原始截图 data URL 暴露给聊天上下文。",
        with_compression_parameters(
            json!({
                "type": "object",
                "properties": {
                    "instruction": { "type": "string", "description": "视觉分析指令：说明当前任务需要关注的页面内容、控件、布局、状态、异常或截图区域线索。" },
                    "timeout": { "type": "integer", "description": "截图超时时间，单位毫秒，默认 60000", "minimum": 1 }
                },
                "required": ["instruction"]
            }),
            false,
            DEFAULT_FORCE_COMPRESS_AFTER_CHARS,
            "视觉分析结果已由轻量模型压缩为文本，默认关闭压缩保留完整结果。",
        ),
        move |args: Value| {
            let deps = deps.clone();
            Box::pin(async move { execute_visual_analyze(&args, &deps).await })
        },
    )
}

async fn execute_visual_analyze(
    args: &Value,
    deps: &BrowserDeps,
) -> Result<ToolOutput, ToolExecutionError> {
    let Some(instruction) = string_arg(args, "instruction") else {
        return Err(ToolExecutionError::invalid_args(
            "错误：缺少必填参数 instruction",
        ));
    };
    // 视觉槽位解析（`resolve_purpose_specs`）已内置凭据回退聊天槽位，
    // 等价于旧实现的 vision_provider → llm_provider+vision_model 回退链。
    let Some(spec) = deps.vision_spec.clone() else {
        return Err(ToolExecutionError::other(
            "错误：浏览器视觉分析需要先在 Dispatcher 设置中配置视觉模型",
        ));
    };
    if !spec.is_configured() {
        return Err(ToolExecutionError::other(
            "错误：LLM API Key 未配置，无法调用视觉模型",
        ));
    }

    let screenshot = match run_browser_command_value(
        deps,
        "screenshot",
        json!({ "fullPage": false, "timeout": timeout_arg(args) }),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return Err(ToolExecutionError::other(format!("错误：{error}"))),
    };
    let Some(data_url) = screenshot.get("data").and_then(Value::as_str) else {
        return Err(ToolExecutionError::other(
            "错误：浏览器截图结果缺少 data URL，无法进行视觉分析",
        ));
    };
    let image = data_url_to_image(data_url)
        .map_err(|error| ToolExecutionError::other(format!("错误：浏览器截图 data URL 非法：{error}")))?;

    let prompt = build_visual_analysis_prompt(&instruction);
    let content = vision_complete_image(
        &spec,
        "你是浏览器网页截图的视觉辅助分析器。只基于截图回答，聚焦用户给定指令；不要编造截图中不可见的信息。输出简洁、可执行的中文观察结果。",
        prompt,
        image,
        None,
        "视觉模型分析网页截图",
    )
    .await
    .map_err(ToolExecutionError::other)?;

    if content.is_empty() {
        return Err(ToolExecutionError::other("错误：视觉模型返回了空分析结果"));
    }
    Ok(ToolOutput::text(content))
}

/// 单命令转发 + 错误分类投影（click 之外的 press/wait_for/open_url 共用；
/// ref 类工具的 RefExpired 自动恢复也经此落入 handle_browser_error）。
pub(super) async fn run_browser_command(
    deps: &BrowserDeps,
    method: &str,
    params: Value,
) -> Result<ToolOutput, ToolExecutionError> {
    match run_browser_command_value(deps, method, params).await {
        Ok(value) => browser_value_output(value),
        Err(error) => {
            // For non-ref tools, classify errors but skip auto-snapshot
            let kind = classify_browser_error(&error);
            Err(ToolExecutionError::other(match kind {
                BrowserErrorKind::Behavioral => {
                    format!(
                        "错误：{error}\n\n提示：这是一个可恢复的行为错误。请检查当前页面状态，\
                        必要时重新调用 browser_read_text 获取最新快照后重试操作。"
                    )
                }
                BrowserErrorKind::System => format!("错误：浏览器系统错误：{error}"),
                BrowserErrorKind::RefExpired => {
                    // Should not happen for non-ref tools, but handle gracefully
                    handle_browser_error(deps, error).await
                }
            }))
        }
    }
}

pub(super) fn browser_value_output(value: Value) -> Result<ToolOutput, ToolExecutionError> {
    match serde_json::to_string_pretty(&value) {
        Ok(text) => Ok(ToolOutput::text(text)),
        Err(error) => Err(ToolExecutionError::other(format!(
            "错误：浏览器结果序列化失败：{error}"
        ))),
    }
}

pub(super) async fn run_browser_command_value(
    deps: &BrowserDeps,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let Some(app) = deps.app_handle.clone() else {
        return Err("浏览器工具缺少 Tauri AppHandle，无法访问浏览器管理器".to_string());
    };
    let manager = app.state::<BrowserManager>();
    manager
        .command(
            app.clone(),
            deps.workspace_id.clone(),
            deps.workspace.to_string_lossy().to_string(),
            method,
            params,
        )
        .await
}

pub(super) fn timeout_arg(args: &Value) -> u64 {
    u64_arg(args, "timeout")
        .unwrap_or(DEFAULT_BROWSER_TIMEOUT_MS)
        .max(1)
}

fn build_visual_analysis_prompt(instruction: &str) -> String {
    format!(
        "请分析当前浏览器可视区域截图。\n\n关注内容：\n{}\n\n输出要求：\n- 只描述截图中能确认的事实。\n- 优先指出与任务相关的控件、文字、状态、错误、布局位置和下一步可操作线索。\n- 如果截图不足以判断，请直接说明缺失信息。",
        instruction.trim()
    )
}

pub(super) fn format_browser_result(value: &Value) -> String {
    match serde_json::to_string_pretty(value) {
        Ok(text) => text,
        Err(error) => format!("错误：浏览器结果序列化失败：{error}"),
    }
}
