import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { AssistantMessage, type AssistantMessageProps } from "./assistant-message";
import { createRowUiStateStore, RowUiStateProvider } from "./row-ui-state";
import { TooltipProvider } from "../ui/tooltip";

vi.mock("../../hooks/useIsDarkTheme", () => ({ useIsDarkTheme: () => false }));

vi.mock("./markdown-renderer", () => ({
  MarkdownRenderer: ({ content, messageId }: { content: string; messageId?: string }) => (
    <p data-message-id={messageId}>{content}</p>
  ),
}));

const props: AssistantMessageProps = {
  rowId: "turn-1",
  segments: [
    { kind: "assistant-text", messageId: "step-1", text: "先检查配置", superseded: true },
    { kind: "assistant-text", messageId: "answer-1", text: "这是排查结果" },
  ],
  tools: [{ id: "read-1", name: "read_file", status: "success", input: { path: "config.toml" } }],
};

function render(overrides: Partial<AssistantMessageProps> = {}, store = createRowUiStateStore()) {
  return renderToStaticMarkup(
    <TooltipProvider>
      <RowUiStateProvider value={store}>
        <AssistantMessage {...props} {...overrides} />
      </RowUiStateProvider>
    </TooltipProvider>,
  );
}

describe("助手执行过程与回复分区", () => {
  it("首次查看已完成历史时收起过程，回复直接可见", () => {
    const html = render();
    expect(html).toContain("执行过程");
    expect(html).not.toContain("先检查配置");
    expect(html).toContain("这是排查结果");
  });

  it("进度归过程、回复单独展示，并保留各自消息锚点", () => {
    const store = createRowUiStateStore();
    store.set("process:turn-1", true);
    store.set("progress:turn-1", true);
    const html = render({}, store);
    const reply = html.indexOf('aria-label="回复"');
    expect(html.indexOf("先检查配置")).toBeLessThan(reply);
    expect(html.indexOf("这是排查结果")).toBeGreaterThan(reply);
    expect(html).toContain('data-message-id="step-1"');
    expect(html).toContain('data-message-id="answer-1"');
  });

  it("普通聊天不添加空过程框或重复回复标题", () => {
    const html = render({ segments: [props.segments[1]], tools: [] });
    expect(html).not.toContain("执行过程");
    expect(html).not.toContain("ai-reply-heading");
    expect(html).toContain("这是排查结果");
  });

  it("过程收起仍能看到工具错误摘要和待处理工具", () => {
    const store = createRowUiStateStore();
    store.set("process:turn-1", false);
    const html = render(
      {
        tools: [
          { id: "bad", name: "read_file", status: "error", errorText: "配置文件不存在" },
          { id: "pending", name: "local_zsh", status: "running", planned: true },
        ],
      },
      store,
    );
    expect(html).toContain("配置文件不存在");
    expect(html).toContain("1 项失败");
    expect(html).toContain("1 项等待");
    expect(html).toContain("执行命令");
    expect(html).not.toContain("先检查配置");
  });

  it("恢复既有工具详情展开态时不会被新增过程容器隐藏", () => {
    const store = createRowUiStateStore();
    store.set("card:read-1", true);
    const first = render({}, store);
    const remount = render({}, store);
    expect(first).toContain("config.toml");
    expect(remount).toContain("read_file");
  });

  it("用户手动收起优先于流式默认展开，回复仍保持独立", () => {
    const store = createRowUiStateStore();
    store.set("process:turn-1", false);
    store.set("progress:turn-1", true);
    const html = render({ isStreaming: true }, store);
    expect(html).not.toContain("先检查配置");
    expect(html).toContain("正在回复");
    expect(html).toContain("这是排查结果");
  });

  it("只有执行过程时不制造空回复或已完成结论", () => {
    const html = render({ segments: [props.segments[0]] });
    expect(html).toContain("执行过程");
    expect(html).not.toContain('aria-label="回复"');
  });
});
