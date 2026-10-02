//! 进程内 QuickJS。运行时只执行源码和具名绑定，不认识具体工具。

use std::collections::BTreeMap;
use std::sync::Arc;

use rquickjs::prelude::Async;
use rquickjs::{async_with, AsyncContext, AsyncRuntime, CatchResultExt, Function, Promise};
use serde_json::{json, Value};
use tokio::sync::watch;

use super::bindings::{BindingResult, HostBinding};
use super::dispatch::{AcquireError, DispatchPool, StopKind};
use super::error::{CodeRunFailed, FailureKind};
use super::limits::ProgramLimits;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ProgramSuccess {
    pub logs: String,
    pub value: Value,
}

struct Session {
    bindings: BTreeMap<String, HostBinding>,
    pool: Arc<DispatchPool>,
}

pub(super) async fn run(
    code: &str,
    bindings: BTreeMap<String, HostBinding>,
    mut cancel: watch::Receiver<bool>,
    limits: ProgramLimits,
) -> Result<ProgramSuccess, CodeRunFailed> {
    if *cancel.borrow() {
        return Err(CodeRunFailed::new(
            FailureKind::Abort,
            stop_message(FailureKind::Abort),
            "",
        ));
    }
    let pool = DispatchPool::new(limits.max_concurrency, limits.max_calls);
    let session = Arc::new(Session {
        bindings,
        pool: Arc::clone(&pool),
    });
    let watcher = {
        let pool = Arc::clone(&pool);
        let wall = limits.wall_time;
        tokio::spawn(async move {
            let wall_deadline = tokio::time::sleep(wall);
            tokio::pin!(wall_deadline);
            loop {
                tokio::select! {
                    changed = cancel.changed() => {
                        match changed {
                            Ok(()) if *cancel.borrow() => {
                                pool.stop(StopKind::Abort);
                                break;
                            }
                            Ok(()) => {}
                            Err(_) => {
                                // 取消端已关闭。wall-time 仍然有效。
                                wall_deadline.await;
                                pool.stop(StopKind::Timeout);
                                break;
                            }
                        }
                    }
                    _ = &mut wall_deadline => {
                        pool.stop(StopKind::Timeout);
                        break;
                    }
                }
            }
        })
    };
    let result = drive(
        code,
        session,
        Arc::clone(&pool),
        limits.max_memory_bytes,
        limits.max_captured_chars,
    )
    .await;
    watcher.abort();
    result
}

async fn drive(
    code: &str,
    session: Arc<Session>,
    pool: Arc<DispatchPool>,
    memory_limit: usize,
    captured_limit: usize,
) -> Result<ProgramSuccess, CodeRunFailed> {
    let runtime = AsyncRuntime::new().map_err(|error| {
        CodeRunFailed::new(
            FailureKind::Exception,
            format!("QuickJS 运行时创建失败：{error}"),
            "",
        )
    })?;
    runtime.set_memory_limit(memory_limit).await;
    let interrupt_pool = Arc::clone(&pool);
    runtime
        .set_interrupt_handler(Some(Box::new(move || interrupt_pool.should_interrupt_js())))
        .await;
    let context = AsyncContext::full(&runtime).await.map_err(|error| {
        CodeRunFailed::new(
            FailureKind::Exception,
            format!("QuickJS 上下文创建失败：{error}"),
            "",
        )
    })?;
    let script = assemble_script(code, captured_limit);
    let evaluated = async_with!(context => |ctx| {
        let host_session = Arc::clone(&session);
        let host = Function::new(
            ctx.clone(),
            Async(move |name: String, payload: String| {
                let host_session = Arc::clone(&host_session);
                async move { host_call(host_session, name, payload).await }
            }),
        )
        .map_err(|error| format!("注册绑定入口失败：{error}"))?
        .with_name("__host_call")
        .map_err(|error| format!("命名绑定入口失败：{error}"))?;
        ctx.globals()
            .set("__host_call", host)
            .map_err(|error| format!("安装绑定入口失败：{error}"))?;
        let promise = ctx
            .eval::<Promise, _>(script.as_str())
            .catch(&ctx)
            .map_err(|error| format!("{error}"))?;
        promise
            .into_future::<String>()
            .await
            .catch(&ctx)
            .map_err(|error| format!("{error}"))
    })
    .await;
    match evaluated {
        Ok(payload) => decode_payload(&payload, &pool, captured_limit),
        Err(message) => classify_drive_error(&message, &pool),
    }
}

fn classify_drive_error(
    message: &str,
    pool: &DispatchPool,
) -> Result<ProgramSuccess, CodeRunFailed> {
    if let Some(kind) = pool.stop_kind() {
        return Err(CodeRunFailed::new(kind, stop_message(kind), ""));
    }
    Err(CodeRunFailed::new(FailureKind::Exception, message, ""))
}

async fn host_call(
    session: Arc<Session>,
    name: String,
    payload: String,
) -> rquickjs::Result<String> {
    if let Some(kind) = session.pool.stop_kind() {
        return Ok(fatal_json(kind));
    }
    let Some(binding) = session.bindings.get(&name) else {
        return Ok(tool_error_json(format!("工具 '{name}' 不在本轮数据面")));
    };
    let parallel = binding.parallel;
    let call = Arc::clone(&binding.call);
    let arguments = match serde_json::from_str::<Value>(&payload) {
        Ok(value) if value.is_object() => value,
        _ => return Ok(tool_error_json("参数必须是对象")),
    };
    let permit = match session.pool.acquire(parallel).await {
        Ok(permit) => permit,
        Err(AcquireError::CallLimit) => {
            return Ok(tool_error_json(session.pool.call_limit_message()))
        }
        Err(AcquireError::Stopped(kind)) => return Ok(fatal_json(kind)),
    };
    let sequence = permit.sequence();
    let result = call(sequence, arguments).await;
    drop(permit);
    if let Some(kind) = session.pool.stop_kind() {
        return Ok(fatal_json(kind));
    }
    Ok(match result {
        BindingResult::Value(value) => ok_json(value),
        BindingResult::ToolError(message) => tool_error_json(message),
        BindingResult::Cancelled => {
            session.pool.stop(StopKind::Abort);
            fatal_json(session.pool.stop_kind().unwrap_or(FailureKind::Abort))
        }
    })
}

fn decode_payload(
    payload: &str,
    pool: &DispatchPool,
    captured_limit: usize,
) -> Result<ProgramSuccess, CodeRunFailed> {
    if payload.chars().count() > captured_limit {
        return Err(CodeRunFailed::new(
            FailureKind::OutputLimit,
            stop_message(FailureKind::OutputLimit),
            prefix_chars(payload, captured_limit.min(8_000)),
        ));
    }
    let value: Value = serde_json::from_str(payload).map_err(|error| {
        CodeRunFailed::new(
            FailureKind::Exception,
            format!("程序结果不是 JSON：{error}"),
            "",
        )
    })?;
    let logs = value
        .get("logs")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if let Some(failed) = value.get("failed").and_then(Value::as_str) {
        let kind = match failed {
            "timeout" => FailureKind::Timeout,
            "abort" => FailureKind::Abort,
            "output-limit" => FailureKind::OutputLimit,
            _ => FailureKind::Exception,
        };
        let message = if kind == FailureKind::Exception {
            failed.to_string()
        } else {
            stop_message(kind).to_string()
        };
        return Err(CodeRunFailed::new(kind, message, logs));
    }
    if let Some(kind) = pool.stop_kind() {
        return Err(CodeRunFailed::new(kind, stop_message(kind), logs));
    }
    let returned = value.get("value").cloned().unwrap_or(Value::Null);
    Ok(ProgramSuccess {
        logs,
        value: returned,
    })
}

fn stop_message(kind: FailureKind) -> &'static str {
    match kind {
        FailureKind::Timeout => "程序超过时间上限",
        FailureKind::Abort => "程序已取消",
        FailureKind::OutputLimit => "程序输出超过内部捕获上限",
        FailureKind::Exception => "程序执行失败",
    }
}

fn assemble_script(code: &str, log_limit: usize) -> String {
    let mut script = String::from("(async () => {\n");
    script.push_str(&format!(
        r#"const __logs = [];
const __logLimit = {log_limit};
function __pushLog(args) {{
  const line = args.map((item) => {{
    if (typeof item === "string") return item;
    try {{ return JSON.stringify(item); }} catch (error) {{ return String(item); }}
  }}).join(" ");
  const used = __logs.join("\n").length + (__logs.length ? 1 : 0);
  if (used + line.length > __logLimit) throw new Error("output-limit");
  __logs.push(line);
}}
const console = {{
  log: (...args) => __pushLog(args),
  error: (...args) => __pushLog(args),
}};
class ToolCallError extends Error {{
  constructor(tool, message) {{
    super(String(message));
    this.name = "ToolCallError";
    this.tool = String(tool);
  }}
}}
async function __call(name, args) {{
  if (args === undefined) args = {{}};
  if (args === null || typeof args !== "object" || Array.isArray(args)) {{
    throw new ToolCallError(name, "参数必须是对象");
  }}
  let payload;
  try {{ payload = JSON.stringify(args); }}
  catch (error) {{ throw new ToolCallError(name, "参数必须是无损 JSON"); }}
  const raw = await __host_call(String(name), payload);
  const decoded = JSON.parse(raw);
  if (decoded.fatal) throw new Error(decoded.fatal);
  if (!decoded.ok) throw new ToolCallError(decoded.tool || name, decoded.message);
  return decoded.value;
}}
const tools = new Proxy({{}}, {{
  get(_target, prop) {{
    if (typeof prop !== "string") return undefined;
    return (args) => __call(prop, args);
  }},
  set() {{ return false; }},
}});
let __returned;
let __failed = null;
try {{
  __returned = await (async () => {{
"#
    ));
    script.push_str(code);
    script.push_str(
        r#"
  })();
} catch (error) {
  __failed = error instanceof Error ? error.message : String(error);
}
return JSON.stringify({
  logs: __logs.join("\n"),
  failed: __failed,
  value: __returned === undefined ? null : __returned,
});
})()"#,
    );
    script
}

fn ok_json(value: Value) -> String {
    json!({ "ok": true, "value": value }).to_string()
}

fn tool_error_json(message: impl Into<String>) -> String {
    json!({ "ok": false, "message": message.into() }).to_string()
}

fn fatal_json(kind: FailureKind) -> String {
    json!({ "fatal": kind.as_str() }).to_string()
}

fn prefix_chars(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use futures::future::BoxFuture;
    use serde_json::{json, Value};
    use tokio::sync::watch;

    use super::super::bindings::install;
    use super::super::error::FailureKind;
    use super::super::limits::ProgramLimits;
    use super::super::DataPlane;
    use super::run;
    use rig::tool::{PortableDynamicTool, ToolOutput};

    fn idle_cancel() -> watch::Receiver<bool> {
        watch::channel(false).1
    }

    fn object_schema() -> Value {
        json!({ "type": "object", "additionalProperties": true })
    }

    fn text_tool(
        name: &'static str,
        body: impl Fn(Value) -> BoxFuture<'static, String> + Send + Sync + 'static,
    ) -> PortableDynamicTool {
        let body = Arc::new(body);
        PortableDynamicTool::new(name, name, object_schema(), move |args| {
            let body = Arc::clone(&body);
            Box::pin(async move { Ok(ToolOutput::text(body(args).await)) })
        })
    }

    fn read_schema() -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["paths"],
            "properties": { "paths": { "type": "array", "items": { "type": "string" } } }
        })
    }

    fn limits_for_test(wall: Duration) -> ProgramLimits {
        ProgramLimits {
            wall_time: wall,
            ..ProgramLimits::default()
        }
    }

    fn inflight_tool(
        name: &'static str,
        inflight: Arc<AtomicUsize>,
        max_seen: Arc<AtomicUsize>,
    ) -> PortableDynamicTool {
        text_tool(name, move |_| {
            let inflight = Arc::clone(&inflight);
            let max_seen = Arc::clone(&max_seen);
            Box::pin(async move {
                let now = inflight.fetch_add(1, Ordering::SeqCst) + 1;
                max_seen.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(80)).await;
                inflight.fetch_sub(1, Ordering::SeqCst);
                "ok".to_string()
            })
        })
    }

    #[tokio::test]
    async fn program_filters_grep_text_then_reads_that_path() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen_read = Arc::clone(&seen);
        let grep = text_tool("grep", |_| {
            Box::pin(async { "src/lib.rs:10:fn main".to_string() })
        });
        let read = PortableDynamicTool::new("read_file", "read", read_schema(), move |args| {
            seen_read.lock().expect("seen").push(args);
            Box::pin(async { Ok(ToolOutput::text("file body")) })
        });
        let plane = DataPlane::new(vec![grep, read]);
        let bindings = install(&plane, None, None);
        let success = run(
            r#"
                const text = await tools.grep({ pattern: "fn main", paths: ["src"] });
                const path = text.split(":")[0];
                return await tools.read_file({ paths: [path] });
            "#,
            bindings,
            idle_cancel(),
            ProgramLimits::default(),
        )
        .await
        .expect("program succeeds");
        assert_eq!(success.value, json!("file body"));
        let calls = seen.lock().expect("seen");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["paths"], json!(["src/lib.rs"]));
    }

    #[tokio::test]
    async fn wrong_field_is_a_catchable_tool_error_and_does_not_run_the_tool() {
        let ran = Arc::new(AtomicUsize::new(0));
        let ran_tool = Arc::clone(&ran);
        let read = PortableDynamicTool::new("read_file", "read", read_schema(), move |_args| {
            ran_tool.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(ToolOutput::text("nope")) })
        });
        let bindings = install(&DataPlane::new(vec![read]), None, None);
        let success = run(
            r#"
                try {
                    await tools.read_file({ path: "a.rs" });
                    return "ran";
                } catch (error) {
                    if (error instanceof ToolCallError) return error.message;
                    throw error;
                }
            "#,
            bindings,
            idle_cancel(),
            ProgramLimits::default(),
        )
        .await
        .expect("caught tool error");
        assert_eq!(ran.load(Ordering::SeqCst), 0);
        let message = success.value.as_str().unwrap_or("");
        assert!(
            message.contains("paths") || message.contains("path"),
            "{message}"
        );
    }

    #[tokio::test]
    async fn uncaught_tool_error_keeps_console_output() {
        let read = PortableDynamicTool::new("read_file", "read", read_schema(), |_args| {
            Box::pin(async { Ok(ToolOutput::text("unused")) })
        });
        let bindings = install(&DataPlane::new(vec![read]), None, None);
        let error = run(
            r#"
                console.log("seen");
                await tools.read_file({ path: "a.rs" });
                return "ok";
            "#,
            bindings,
            idle_cancel(),
            ProgramLimits::default(),
        )
        .await
        .expect_err("uncaught");
        let text = error.model_text();
        assert!(text.contains("code run failed (exception)"), "{text}");
        assert!(text.contains("seen"), "{text}");
    }

    #[tokio::test]
    async fn unknown_tool_throws_without_a_binding() {
        let bindings = install(&DataPlane::new(vec![]), None, None);
        let success = run(
            r#"
                try {
                    await tools.message({ content: "hi" });
                    return "ran";
                } catch (error) {
                    return error.message;
                }
            "#,
            bindings,
            idle_cancel(),
            ProgramLimits::default(),
        )
        .await
        .expect("unknown tool is catchable");
        assert!(success.value.as_str().unwrap_or("").contains("message"));
    }

    #[tokio::test]
    async fn thirty_third_call_is_catchable_and_does_not_stop_the_program() {
        let ran = Arc::new(AtomicUsize::new(0));
        let ran_tool = Arc::clone(&ran);
        let grep = text_tool("grep", move |_| {
            let ran_tool = Arc::clone(&ran_tool);
            Box::pin(async move {
                ran_tool.fetch_add(1, Ordering::SeqCst);
                "ok".to_string()
            })
        });
        let bindings = install(&DataPlane::new(vec![grep]), None, None);
        let success = run(
            r#"
                let last = "";
                for (let i = 0; i < 33; i++) {
                    try {
                        last = await tools.grep({ pattern: "a", paths: ["."] });
                    } catch (error) {
                        return error.message;
                    }
                }
                return last;
            "#,
            bindings,
            idle_cancel(),
            ProgramLimits::default(),
        )
        .await
        .expect("call limit is catchable");
        assert_eq!(ran.load(Ordering::SeqCst), 32);
        assert!(success.value.as_str().unwrap_or("").contains("32"));
    }

    #[tokio::test]
    async fn promise_all_overlaps_readonly_tools_and_serializes_unregistered_tools() {
        let inflight = Arc::new(AtomicUsize::new(0));
        let max_parallel = Arc::new(AtomicUsize::new(0));
        let max_exclusive = Arc::new(AtomicUsize::new(0));
        let grep = inflight_tool("grep", Arc::clone(&inflight), Arc::clone(&max_parallel));
        let read = inflight_tool(
            "read_file",
            Arc::clone(&inflight),
            Arc::clone(&max_parallel),
        );
        let echo = inflight_tool("echo", Arc::clone(&inflight), Arc::clone(&max_exclusive));
        let other = inflight_tool("echo_other", inflight, Arc::clone(&max_exclusive));
        let bindings = install(&DataPlane::new(vec![grep, read, echo, other]), None, None);
        run(
            r#"
                await Promise.all([
                    tools.grep({ pattern: "a", paths: ["."] }),
                    tools.read_file({ paths: ["a.rs"] }),
                ]);
                await Promise.all([
                    tools.echo({}),
                    tools.echo_other({}),
                ]);
                return "ok";
            "#,
            bindings,
            idle_cancel(),
            ProgramLimits::default(),
        )
        .await
        .expect("composed calls finish");
        assert!(
            max_parallel.load(Ordering::SeqCst) >= 2,
            "readonly calls should overlap"
        );
        assert_eq!(max_exclusive.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn deadline_waits_for_the_in_flight_call_and_does_not_start_the_next() {
        let phase = Arc::new(AtomicUsize::new(0));
        let phase_tool = Arc::clone(&phase);
        let slow = text_tool("grep", move |_| {
            let phase_tool = Arc::clone(&phase_tool);
            Box::pin(async move {
                phase_tool.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(150)).await;
                phase_tool.fetch_add(1, Ordering::SeqCst);
                "done".to_string()
            })
        });
        let after = text_tool("read_file", {
            let phase = Arc::clone(&phase);
            move |_| {
                let phase = Arc::clone(&phase);
                Box::pin(async move {
                    phase.fetch_add(10, Ordering::SeqCst);
                    "after".to_string()
                })
            }
        });
        let bindings = install(&DataPlane::new(vec![slow, after]), None, None);
        let error = run(
            r#"
                await tools.grep({ pattern: "a", paths: ["."] });
                await tools.read_file({ paths: ["a.rs"] });
                return "ok";
            "#,
            bindings,
            idle_cancel(),
            limits_for_test(Duration::from_millis(40)),
        )
        .await
        .expect_err("deadline");
        assert_eq!(error.kind, FailureKind::Timeout, "{error:?}");
        let phase = phase.load(Ordering::SeqCst);
        assert_eq!(
            phase, 2,
            "in-flight tool finished and the next call did not start"
        );
    }

    #[tokio::test]
    async fn swallowed_deadline_still_fails_the_program() {
        let slow = text_tool("grep", |_| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(80)).await;
                "done".to_string()
            })
        });
        let bindings = install(&DataPlane::new(vec![slow]), None, None);
        let error = run(
            r#"
                try {
                    await tools.grep({ pattern: "a", paths: ["."] });
                    return "swallowed";
                } catch (error) {
                    return "swallowed";
                }
            "#,
            bindings,
            idle_cancel(),
            limits_for_test(Duration::from_millis(20)),
        )
        .await
        .expect_err("a caught timeout still fails the program");
        assert_eq!(error.kind, FailureKind::Timeout, "{error:?}");
    }

    #[tokio::test]
    async fn cancel_waits_for_the_in_flight_call_and_does_not_start_the_next() {
        let phase = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let phase_tool = Arc::clone(&phase);
        let started_tool = Arc::clone(&started);
        let release_tool = Arc::clone(&release);
        let slow = text_tool("grep", move |_| {
            let phase_tool = Arc::clone(&phase_tool);
            let started_tool = Arc::clone(&started_tool);
            let release_tool = Arc::clone(&release_tool);
            Box::pin(async move {
                phase_tool.fetch_add(1, Ordering::SeqCst);
                started_tool.notify_one();
                release_tool.notified().await;
                phase_tool.fetch_add(1, Ordering::SeqCst);
                "done".to_string()
            })
        });
        let after = text_tool("read_file", {
            let phase = Arc::clone(&phase);
            move |_| {
                let phase = Arc::clone(&phase);
                Box::pin(async move {
                    phase.fetch_add(10, Ordering::SeqCst);
                    "after".to_string()
                })
            }
        });
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let bindings = install(&DataPlane::new(vec![slow, after]), None, None);
        let task = tokio::spawn(async move {
            run(
                r#"
                    await tools.grep({ pattern: "a", paths: ["."] });
                    await tools.read_file({ paths: ["a.rs"] });
                    return "ok";
                "#,
                bindings,
                cancel_rx,
                ProgramLimits::default(),
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(2), started.notified())
            .await
            .expect("in-flight call started");
        cancel_tx.send(true).expect("cancel");
        release.notify_one();
        let error = task.await.expect("join").expect_err("cancelled program");
        assert_eq!(error.kind, FailureKind::Abort, "{error:?}");
        assert_eq!(phase.load(Ordering::SeqCst), 2);
    }
}
