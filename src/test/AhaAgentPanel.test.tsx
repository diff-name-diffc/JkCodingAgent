import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { AhaAgentPanel } from "../components/app-settings/aha/AhaAgentPanel";
import type { AhaSettingsV2 } from "../types";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

const invokeMock = vi.mocked(invoke);

function defaultV2(): AhaSettingsV2 {
  return {
    shared: {
      visionModelConfigs: [
        { url: "https://vision.example.com/v1", apiKey: "sk-vision", model: "vision-v1", active: true },
      ],
      imageModelConfigs: [],
      imageEditModelConfigs: [],
      asrModelConfigs: [],
      ttsModelConfigs: [],
      embeddingModelConfigs: [],
    },
    project: {
      chatModelConfigs: [
        { url: "https://project.example.com/v1", apiKey: "sk-project", model: "project-chat", active: true },
      ],
      summaryModelConfigs: [],
      allowedTools: [],
    },
    chat: {
      chatModelConfigs: [
        { url: "https://chat.example.com/v1", apiKey: "sk-chat", model: "chat-main", active: true },
      ],
      summaryModelConfigs: [],
      allowedTools: [],
    },
    autoApproveDispatch: false,
    contextDebug: false,
  };
}

describe("AhaAgentPanel (v2)", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    invokeMock.mockImplementation(async (command: string, payload?: unknown) => {
      if (command === "aha_get_settings_v2") return defaultV2();
      if (command === "aha_save_settings_v2") {
        return (payload as { settings: AhaSettingsV2 }).settings;
      }
      if (command === "sub_agent_list_tools") return [];
      if (command === "sub_agent_list") return [];
      throw new Error(`unexpected command: ${command}`);
    });
  });

  it("renders the four top-level tabs", async () => {
    render(<AhaAgentPanel />);
    expect(await screen.findByRole("tab", { name: /通用模型/ })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: /项目智能体/ })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: /聊天智能体/ })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: /子智能体/ })).toBeInTheDocument();
  });

  it("loads settings from aha_get_settings_v2 on mount", async () => {
    render(<AhaAgentPanel />);
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("aha_get_settings_v2");
    });
  });

  it("switches to project agent tab and shows sub-tabs", async () => {
    render(<AhaAgentPanel />);
    fireEvent.click(await screen.findByRole("tab", { name: /项目智能体/ }));
    expect(screen.getByRole("tab", { name: /主模型/ })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: /工具配置/ })).toBeInTheDocument();
    const subAgentTabs = screen.getAllByRole("tab", { name: /子智能体/ });
    expect(subAgentTabs.length).toBeGreaterThanOrEqual(2);
  });

  it("shows shared models section in shared tab", async () => {
    render(<AhaAgentPanel />);
    expect(await screen.findByText("视觉模型")).toBeInTheDocument();
  });

  it("calls aha_save_settings_v2 when save is clicked", async () => {
    render(<AhaAgentPanel />);
    await screen.findByText("视觉模型");
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("aha_save_settings_v2", expect.any(Object));
    });
  });
});
