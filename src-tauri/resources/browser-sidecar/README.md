# Browser Use sidecar

固定使用 PyPI `browser-use==0.13.11` 与 Python 3.12：

```sh
uv run --no-project --python 3.12 --with browser-use==0.13.11 python -u driver.py
```

每个进程只管理一个 dispatcher session 的浏览器。启动时优先使用系统或缓存中的
Chrome/Chromium；没有可用浏览器时，Browser Use 调用 `uvx playwright install chromium`
安装。浏览器继承 sidecar 的进程组，Rust 宿主仍负责异常与取消时的最终进程组回收。

stdin/stdout 使用单行 JSON 协议；第三方日志仅进入 stderr。`start`、`open_url`、`back`、
`reload`、坐标 `click`、`screenshot`、`status`、`close` 供 UI 使用。`explore` 执行专用 Agent，
返回明确的 success/result/iterations/tokenUsage/urls。`cancel` 按 runId 取消。
取消和超时返回失败协议响应，并关闭浏览器；普通目标未完成返回 `success:false`。
正常完成保留浏览器及登录状态供下次委派或 UI 使用。探索期间禁止 UI 修改页面。
失败结果的 `result` 保留具体错误与停止原因，区分启动失败、连续错误、步数限制、
程序停止和模型主动报告未完成；`iterations` 记录实际尝试步骤数，包含上下文准备失败。
步骤开始与错误会记录进展，错误诊断保留异常类型/原因链并脱敏、限长，
不读取模型 thinking/memory，不回传完整 history 或 ValidationError 原始输入。

预览默认不截图。`set_preview` 接收
`{revision, observers:[{observerId, toolCallId, leaseMs}]}`，每次替换完整观察者集合；
同进程仅接受递增的 u64 revision，租约按单调时钟计算、最多 8000ms。
`toolCallId:null` 观察会话浏览器，指定值只观察当前探索的 `explore.toolCallId`。
只有存在匹配且未过期的可见观察者才开启约 1fps 的单路 JPEG 采集；多观察者共享帧，
续租不会触发额外截图，隐藏/租约到期后停止采集。
`frame` 带有 `sessionId/runId/toolCallId/sequence/capturedAt/url/data/width/height`；
普通浏览时 runId/toolCallId 为 null，sequence 在进程内递增，capturedAt 为 Unix 毫秒。
截图前后重新验证运行身份、观察租约与取消状态，丢弃跨运行、隐藏和取消的在途帧。
监控不持有动作锁、不进入模型消息；截图与 URL 获取共用 3 秒预算。
预览失败发独立 `preview_error`（携带 sessionId/runId/toolCallId/message）并记日志，
不进入 Agent 执行事件、不中断 Agent；隐藏、取消或跨运行后的迟到错误同样丢弃。

Tauri 的 `browser_preview_set` 只登记观察租约，不启动浏览器或取得动作锁；
前端仅在文档与监控区域同时可见时注册，每 3 秒续租，隐藏/卸载立即释放，
异常断连由 8 秒租约兜底。宿主为帧附加进程级 `streamId`，并校验当前执行与帧序号。
子智能体小窗绑定 dispatcher session + tool call，收到首帧后固定 run 身份；
`browser_cancel_exploration` 也核对同一身份，只停止对应执行。完成/失败/取消/超时后
冻结最后收到的画面，不为终态补拍。前端共享帧监听、每个 Canvas 只解码最新帧，
画面不写入轨迹数据库；内存缓存最多 24 个会话/执行索引、16 MiB 图片字符串，
关闭并重开轨迹可恢复尚未淘汰的画面，应用重启后不保留。

Agent 仅暴露浏览器动作白名单，不暴露任意 JavaScript、Shell、Python、上传或文件读写。
截图、内容抽取和下载产物存入 profile 的 `runs/<随机ID>/`。自动确认弹窗仅在探索期间启用；
prompt、普通 UI confirm 以及取消开始后的弹窗均拒绝。处理结果进入 progress 事件。
遥测、云同步、自动扩展与版本检查均关闭。模型明确使用宿主传来的 OpenAI-compatible
Chat Completions 配置，API key 在协议与 stderr 日志边界脱敏。
请求使用宿主的 `max_tokens` 语义，未配置时省略。结构化请求启用
`response_format: {"type":"json_object"}`，并将上游动作 schema 作为合法 JSON 加入系统消息；
最终回答的格式要求放入 `done.text`，协议外层始终是 JSON 对象。
网关须支持 JSON mode，无需支持 strict `json_schema`；普通文本抽取请求不强制 JSON。
返回结果仍由上游 Pydantic 校验，非法 JSON/动作结构不会执行，也不提取片段、修补或降级重试。
本地结构化输出校验失败明确标示为解析/校验错误，不把 SDK 的默认 502 当成真实 HTTP 状态。
`explore.useVision` 默认为 `false`，DOM 浏览仍可工作；只有用户明确启用才向模型发送截图，
因此开启视觉时配置的模型必须支持图片输入。
自动化只接受 http(s)、about:blank 及宿主 `allowedFileRoot` 内的 file URL；解码并解析
符号链接后校验路径，感知前也检查当前页面，内部协议和工作区外文件均拒绝。

`dialogs.py` / `navigation.py` 扩展固定版本上游 watchdog，`PersistentAgent` 明确将浏览器生命周期
交回宿主；升级依赖必须重跑真实浏览器集成测试，不能直接修改版本号。

无模型凭据测试（需要可运行的本地 Chrome/Chromium）：

```sh
uv run --no-project --python 3.12 --with browser-use==0.13.11 --with pytest --with pytest-asyncio python -m pytest . -q
```

设置 `JKC_TEST_UV_RUNTIME=1` 还会通过生产 `uv run --with` 启动方式验证 JSONL 协议。
