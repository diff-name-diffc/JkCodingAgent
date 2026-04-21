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
});
