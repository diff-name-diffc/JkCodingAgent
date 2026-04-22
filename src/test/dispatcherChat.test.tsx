import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ComponentProps } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DispatcherMessage, DispatcherSettings } from "../types";

const { invokeMock, openDialogMock, scrollIntoViewMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  openDialogMock: vi.fn(),
  scrollIntoViewMock: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
  Channel: class MockChannel<T> {
    onmessage: ((event: T) => void) | null = null;
  },
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: (...args: unknown[]) => openDialogMock(...args),
}));

import { DispatcherChat } from "../components/DispatcherChat";

function createSettings(): DispatcherSettings {
  return {
    apiBase: "https://example.com",
    apiKey: "token",
    model: "gpt-test",
    autoApproveDispatch: false,
    contextDebug: false,
  };
}

function renderChat(props?: Partial<ComponentProps<typeof DispatcherChat>>) {
  return render(
    <DispatcherChat
      sessionId="ws-1"
      projectPath="/repo"
      mcpStatus={null}
      mcpChecking={false}
      subProcesses={[]}
      onDispatchApproved={() => undefined}
      onDispatchRejected={() => undefined}
      onDispatchContinue={() => undefined}
      onDispatchExit={() => undefined}
      onOpenMcpStatus={() => undefined}
      onOpenSettings={() => undefined}
      {...props}
    />,
  );
}

describe("DispatcherChat", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    openDialogMock.mockReset();
    scrollIntoViewMock.mockReset();
    Element.prototype.scrollIntoView = scrollIntoViewMock;
  });

  it("支持附件上传并在发送消息时携带 attachment ids", async () => {
    const attachment = {
      id: "att-1",
      workspaceId: "ws-1",
      messageId: null,
      originalName: "rules.md",
      storedPath: "/repo/.nezha/dispatcher-attachments/ws-1/rules.md",
      mimeType: "text/markdown",
      sizeBytes: 128,
      createdAt: "2026-04-21T00:00:00Z",
    };

    invokeMock.mockImplementation(async (command: string) => {
      switch (command) {
        case "dispatcher_get_settings":
          return createSettings();
        case "dispatcher_list_messages":
          return [];
        case "dispatcher_get_session_token_usage":
          return [];
        case "dispatcher_list_pending_attachments":
          return [];
        case "dispatcher_upload_attachment":
          return attachment;
        case "dispatcher_send_message":
          return { reply: null, messages: [] };
        default:
          return null;
      }
    });
    openDialogMock.mockResolvedValue("/tmp/rules.md");

    renderChat();

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("dispatcher_list_messages", {
        workspaceId: "ws-1",
      });
    });

    await userEvent.click(screen.getByRole("button", { name: "附件" }));
    expect(await screen.findByText("rules.md")).toBeInTheDocument();

    await userEvent.type(
      screen.getByPlaceholderText("例如：先审查这个仓库的前端架构，再给出重构方案并开始实现。"),
      "请按附件规则审查 sample.dwg",
    );
    await userEvent.click(screen.getByRole("button", { name: "开始对话" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "dispatcher_send_message",
        expect.objectContaining({
          workspaceId: "ws-1",
          projectPath: "/repo",
          content: "请按附件规则审查 sample.dwg",
          attachments: ["att-1"],
        }),
      );
    });
  });

  it("收到 CAD 结果消息定位时会滚动并高亮对应轮次", async () => {
    const messages: DispatcherMessage[] = [
      {
        id: "user-1",
        workspaceId: "ws-1",
        role: "user",
        content: "审查 DWG",
        attachments: [],
        createdAt: "2026-04-21T00:00:00Z",
      },
      {
        id: "tool-1",
        workspaceId: "ws-1",
        role: "tool",
        content: "CAD 审查结果已保存",
        toolCallId: "call-1",
        toolName: "cad_save_review_result",
        toolResultMode: "raw",
        attachments: [],
        toolArtifacts: [],
        createdAt: "2026-04-21T00:00:01Z",
      },
    ];

    invokeMock.mockImplementation(async (command: string) => {
      switch (command) {
        case "dispatcher_get_settings":
          return createSettings();
        case "dispatcher_list_messages":
          return messages;
        case "dispatcher_get_session_token_usage":
          return [];
        case "dispatcher_list_pending_attachments":
          return [];
        default:
          return null;
      }
    });

    const { container } = renderChat({ activeCadResultMessageId: "tool-1" });

    await waitFor(() => {
      const bubble = container.querySelector('[data-dispatch-message-id="tool-1"]');
      expect(bubble).not.toBeNull();
      expect(scrollIntoViewMock).toHaveBeenCalled();
      expect(bubble).toHaveAttribute("data-highlighted", "true");
    });
  });

  it("会在发送区旁展示当前会话的模型 token 占用并支持悬浮查看详情", async () => {
    invokeMock.mockImplementation(async (command: string) => {
      switch (command) {
        case "dispatcher_get_settings":
          return createSettings();
        case "dispatcher_list_messages":
          return [];
        case "dispatcher_get_session_token_usage":
          return [
            {
              workspaceId: "ws-1",
              model: "qwen3.6-plus",
              sourceKind: "primary",
              promptTokens: 8192,
              completionTokens: 512,
              totalTokens: 8704,
              cachedTokens: 2048,
              contextWindowTokens: 8192,
              contextWindowCapacity: 1000000,
              updatedAt: "2026-04-22T00:00:00Z",
            },
            {
              workspaceId: "ws-1",
              model: "qwen3.6-flash",
              sourceKind: "summary",
              promptTokens: 2048,
              completionTokens: 128,
              totalTokens: 2176,
              cachedTokens: 0,
              contextWindowTokens: 2048,
              contextWindowCapacity: 1000000,
              updatedAt: "2026-04-22T00:00:01Z",
            },
          ];
        case "dispatcher_list_pending_attachments":
          return [];
        default:
          return null;
      }
    });

    renderChat();

    const usageButton = await screen.findByRole("button", {
      name: /qwen3\.6-flash 窗口 token 占用/i,
    });
    await userEvent.hover(usageButton);

    expect(await screen.findByText("qwen3.6-flash")).toBeInTheDocument();
    expect(screen.getByText("摘要模型")).toBeInTheDocument();
    expect(screen.getByText("2,048 / 1,000,000")).toBeInTheDocument();
  });

  it("新会话在尚无 usage 时也会显示空的模型占用圈", async () => {
    invokeMock.mockImplementation(async (command: string) => {
      switch (command) {
        case "dispatcher_get_settings":
          return createSettings();
        case "dispatcher_list_messages":
          return [];
        case "dispatcher_get_session_token_usage":
          return [];
        case "dispatcher_list_pending_attachments":
          return [];
        default:
          return null;
      }
    });

    renderChat();

    expect(
      await screen.findByRole("button", {
        name: /gpt-test 窗口 token 占用 0%/i,
      }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", {
        name: /qwen3\.6-flash 窗口 token 占用 0%/i,
      }),
    ).toBeInTheDocument();
  });

  it("收到不完整的 finished 快照时不会把已有对话覆盖掉", async () => {
    const initialMessages: DispatcherMessage[] = [
      {
        id: "user-1",
        workspaceId: "ws-1",
        role: "user",
        content: "已有问题",
        attachments: [],
        createdAt: "2026-04-21T00:00:00Z",
      },
      {
        id: "assistant-1",
        workspaceId: "ws-1",
        role: "assistant",
        content: "已有回复",
        attachments: [],
        createdAt: "2026-04-21T00:00:01Z",
      },
    ];

    invokeMock.mockImplementation(async (command: string, payload?: Record<string, unknown>) => {
      switch (command) {
        case "dispatcher_get_settings":
          return createSettings();
        case "dispatcher_list_messages":
          return initialMessages;
        case "dispatcher_get_session_token_usage":
          return [];
        case "dispatcher_list_pending_attachments":
          return [];
        case "dispatcher_send_message": {
          const onEvent = payload?.onEvent as {
            onmessage: ((event: { event: string; data: Record<string, unknown> }) => void) | null;
          };
          onEvent.onmessage?.({
            event: "userMessage",
            data: {
              message: {
                id: "user-2",
                workspaceId: "ws-1",
                role: "user",
                content: "新的追问",
                attachments: [],
                createdAt: "2026-04-21T00:00:02Z",
              },
            },
          });
          onEvent.onmessage?.({
            event: "assistantMessage",
            data: {
              message: {
                id: "assistant-2",
                workspaceId: "ws-1",
                role: "assistant",
                content: "新的回复",
                attachments: [],
                createdAt: "2026-04-21T00:00:03Z",
              },
            },
          });
          onEvent.onmessage?.({
            event: "finished",
            data: {
              messages: [
                {
                  id: "assistant-2",
                  workspaceId: "ws-1",
                  role: "assistant",
                  content: "新的回复",
                  attachments: [],
                  createdAt: "2026-04-21T00:00:03Z",
                },
              ],
            },
          });
          return { reply: null, messages: [] };
        }
        default:
          return null;
      }
    });

    renderChat();

    expect(await screen.findByText("已有问题")).toBeInTheDocument();
    expect(screen.getByText("已有回复")).toBeInTheDocument();

    await userEvent.type(screen.getByPlaceholderText("给调度智能体发送消息..."), "新的追问");
    await userEvent.click(screen.getByRole("button", { name: "开始对话" }));

    await waitFor(() => {
      expect(screen.getByText("已有问题")).toBeInTheDocument();
      expect(screen.getByText("已有回复")).toBeInTheDocument();
      expect(screen.getByText("新的追问")).toBeInTheDocument();
      expect(screen.getByText("新的回复")).toBeInTheDocument();
    });
  });
});
