@AGENTS.md

## 新增 Agent 工具流程

新增工具涉及 3 个文件（必选）+ 1 个文件（可选），前端无需修改。

### 1. 实现工具 — `src-tauri/src/agent/tools/builtin/<tool_name>.rs`

创建新文件，实现 `AgentTool` trait（定义在 `registry.rs`）：

```rust
pub(super) fn my_tool() -> Box<dyn AgentTool> { Box::new(MyTool) }
struct MyTool;
#[async_trait]
impl AgentTool for MyTool {
    fn name(&self) -> &'static str { "my_tool" }
    fn description(&self) -> &'static str { "工具用途描述" }
    fn parameters(&self) -> Value { json!({ "type": "object", "properties": { ... } }) }
    async fn execute(&self, args: &Value, ctx: &ToolContext) -> String { ... }
}
```

- 参数提取用 `common.rs` 中的 `string_arg` / `boolish_arg` 等辅助函数
- 路径参数必须通过 `resolve_path(ctx, raw)` 确保不越界
- 错误消息以 `"错误："` 开头
- 如需 `result_mode` 参数，调用 `with_result_mode_parameter(schema, default, guidance)`

### 2. 注册工具 — `src-tauri/src/agent/tools/builtin/mod.rs`

两处：顶部 `mod my_tool;` + `builtin_tools()` 函数中添加 `my_tool::my_tool()`。

### 3. 分类工具输出 — `src-tauri/src/agent/summary.rs`

在 `tool_output_kind()` 中添加工具名到对应的 `ToolOutputKind`：
- `Exact`：输出本身就是精确答案（read_file、grep 等）
- `Command`：命令执行结果，大输出需摘要（exec）
- `Mutation`：写操作结果，保持原文（write_file、generate_image）
- `Message`：简单消息，保持原文
- `Other`：默认，大输出时自动摘要

可选：在 `tool_summary_focus()` 中添加该工具的摘要侧重点说明。

### 4. 添加配置（可选）— `src-tauri/src/agent/config.rs`

如工具需要 API Key / URL 等配置：在 `DispatcherAgentConfig` 加字段 → `load()` 中从环境变量读取 → `runtime.rs` 中传入 `ToolContext`。
