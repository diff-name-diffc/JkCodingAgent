import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SubAgentSession } from "./subAgentEventStore";
import { SubAgentExecutionCard } from "./SubAgentExecutionView";
import { ArtifactPanel } from "./artifact/artifact-panel";

const monitor = vi.hoisted(() => vi.fn());
vi.mock("./browser/BrowserAgentMonitor", () => ({
  BrowserAgentMonitor: (props: unknown) => {
    monitor(props);
    return <div>当前执行浏览器画面</div>;
  },
}));
vi.mock("./markdown/MarkdownRenderer", () => ({
  MarkdownRenderer: ({ content }: { content: string }) => <p>{content}</p>,
}));
vi.mock("./artifact/artifact-renderers", () => ({ renderArtifactContent: () => null }));

const session: SubAgentSession = {
  agentId: "browser-agent",
  name: "浏览器助手",
  task: "检查搜索结果",
  model: "test-model",
  responseText: "",
  progressMessages: [],
  events: [],
  elapsed: 0,
  status: "running",
  phase: "tool_calling",
  toolCalls: [],
};

beforeEach(() => monitor.mockClear());

describe("子智能体轨迹中的浏览器身份", () => {
  it("详情面板将所选工具调用和 dispatcher 会话透传到监控", () => {
    renderToStaticMarkup(
      <ArtifactPanel
        workspaceId="dispatcher-a"
        subAgentToolCallId="call-first"
        subAgentSession={session}
      />,
    );
    expect(monitor).toHaveBeenLastCalledWith({
      sessionId: "dispatcher-a",
      toolCallId: "call-first",
      status: "running",
    });
    renderToStaticMarkup(
      <ArtifactPanel
        workspaceId="dispatcher-a"
        subAgentToolCallId="call-next"
        subAgentSession={{ ...session, status: "completed" }}
      />,
    );
    expect(monitor).toHaveBeenLastCalledWith({
      sessionId: "dispatcher-a",
      toolCallId: "call-next",
      status: "completed",
    });
  });

  it("卡片收起时卸载监控以暂停观察", () => {
    const html = renderToStaticMarkup(
      <SubAgentExecutionCard
        session={session}
        dispatcherSessionId="dispatcher-a"
        toolCallId="call-a"
        autoExpand={false}
      />,
    );
    expect(monitor).not.toHaveBeenCalled();
    expect(html).not.toContain("当前执行浏览器画面");
  });

  it("监控位于展开内容顶部，长任务概览不会遮住初始画面", () => {
    const html = renderToStaticMarkup(
      <SubAgentExecutionCard
        session={{ ...session, task: "较长任务描述\n".repeat(100) }}
        dispatcherSessionId="dispatcher-a"
        toolCallId="call-a"
      />,
    );
    expect(html.indexOf("当前执行浏览器画面")).toBeLessThan(html.indexOf("来源任务"));
  });

  it("缺少执行身份时不订阅会话级画面，避免历史轨迹串帧", () => {
    renderToStaticMarkup(
      <SubAgentExecutionCard session={session} dispatcherSessionId="dispatcher-a" />,
    );
    renderToStaticMarkup(<SubAgentExecutionCard session={session} toolCallId="call-a" />);
    expect(monitor).not.toHaveBeenCalled();
  });

  it("普通子智能体不添加浏览器监控", () => {
    renderToStaticMarkup(
      <SubAgentExecutionCard
        session={{ ...session, agentId: "coding-agent" }}
        dispatcherSessionId="dispatcher-a"
        toolCallId="call-a"
      />,
    );
    expect(monitor).not.toHaveBeenCalled();
  });
});
