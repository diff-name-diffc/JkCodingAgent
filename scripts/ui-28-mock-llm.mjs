// UI-28 运行态验收辅助：本地 mock LLM（OpenAI 兼容 /v1/chat/completions，SSE 流式）。
// 用途见 docs/ui-redesign-2026-09-06/03-tasks.md 第 22 节。零依赖，Node ≥ 18。
//
// 行为分支（按请求特征依次判定）：
//   - 最后一条消息 role=tool                  → 短确认文本（工具调用回合收尾）
//   - 请求带 tools 且用户消息含「创建文件」     → 发起 write_file 工具调用
//   - 用户消息含「长回复」                     → 80 行慢速流式（测试停止按钮/滚动锚点）
//   - 其余                                    → 常规分块流式回复（含 usage chunk，驱动 token 用量展示）
//
// 启动：node scripts/ui-28-mock-llm.mjs   （监听 http://127.0.0.1:8802/v1）
// 应用侧配置：设置 → 模型服务 → 对话模型条目 URL 填 http://127.0.0.1:8802/v1，
// API Key 任意非空（不校验），模型名称任意（不参与响应内容）。
import http from "node:http";

const PORT = Number(process.env.MOCK_LLM_PORT ?? 8802);

function sseChunk(res, delta, finish = null, usage = undefined) {
  const obj = {
    id: "chatcmpl-mock",
    object: "chat.completion.chunk",
    created: Math.floor(Date.now() / 1000),
    model: "mock-chat",
    choices: [{ index: 0, delta, finish_reason: finish }],
  };
  if (usage) obj.usage = usage;
  res.write(`data: ${JSON.stringify(obj)}\n\n`);
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const server = http.createServer(async (req, res) => {
  if (req.method !== "POST" || !req.url.endsWith("/chat/completions")) {
    res.writeHead(404).end();
    return;
  }
  let body = "";
  for await (const chunk of req) body += chunk;
  let payload;
  try {
    payload = JSON.parse(body);
  } catch {
    res.writeHead(400).end();
    return;
  }
  const messages = Array.isArray(payload.messages) ? payload.messages : [];
  const last = messages[messages.length - 1] ?? {};
  const lastUser = [...messages].reverse().find((m) => m.role === "user");
  const userText =
    typeof lastUser?.content === "string"
      ? lastUser.content
      : JSON.stringify(lastUser?.content ?? "");
  const hasTools = Array.isArray(payload.tools) && payload.tools.length > 0;

  res.writeHead(200, {
    "Content-Type": "text/event-stream",
    "Cache-Control": "no-cache",
    Connection: "keep-alive",
  });

  const usage = { prompt_tokens: 128, completion_tokens: 64, total_tokens: 192 };
  const startedAt = Date.now();
  console.log(`[mock] 请求进入：userText=${JSON.stringify(userText.slice(0, 20))} tools=${hasTools}`);

  try {
    if (last.role === "tool") {
      for (const c of ["文件", "已", "创建", "完成", "。"]) {
        sseChunk(res, { content: c });
        await sleep(40);
      }
      sseChunk(res, {}, "stop");
      sseChunk(res, {}, null, usage);
    } else if (hasTools && /创建文件/.test(userText)) {
      const args = JSON.stringify({
        path: "mock-notes.md",
        content: "# Mock 写入\n\n由 mock 模型通过 write_file 创建。\n",
      });
      sseChunk(res, {
        tool_calls: [
          { index: 0, id: "call_mock_1", type: "function", function: { name: "write_file", arguments: "" } },
        ],
      });
      sseChunk(res, { tool_calls: [{ index: 0, function: { arguments: args } }] });
      sseChunk(res, {}, "tool_calls");
      sseChunk(res, {}, null, usage);
    } else if (/长回复/.test(userText)) {
      for (let i = 0; i < 80; i++) {
        sseChunk(res, { content: `这是长回复的第 ${i + 1} 行，用于测试停止与滚动锚点。\n` });
        await sleep(120);
      }
      sseChunk(res, {}, "stop");
      sseChunk(res, {}, null, usage);
    } else {
      const reply = `你好！这是 mock 模型的流式回复。你发送的是：「${userText.slice(0, 60)}」。需要测试工具调用可以发送：创建文件；需要测试停止可以发送：长回复。`;
      const tokens =
        reply.match(/[\u4e00-\u9fa5]|[a-zA-Z]+|[^\s\u4e00-\u9fa5a-zA-Z]|\s+/g) ?? [reply];
      for (const t of tokens) {
        sseChunk(res, { content: t });
        await sleep(25);
      }
      sseChunk(res, {}, "stop");
      sseChunk(res, {}, null, usage);
    }
  } catch {
    // 客户端提前断开（停止按钮）属预期
    console.log(`[mock] 请求提前断开（客户端 abort）：userText=${JSON.stringify(userText.slice(0, 20))}`);
  } finally {
    res.write("data: [DONE]\n\n");
    res.end();
    console.log(
      `[mock] 请求结束：userText=${JSON.stringify(userText.slice(0, 20))} tools=${hasTools} 耗时=${Date.now() - startedAt}ms`,
    );
  }
});

server.listen(PORT, "127.0.0.1", () => {
  console.log(`mock LLM listening on http://127.0.0.1:${PORT}/v1`);
});
