import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import {
  AhaAgentPanel,
  draftToSavePayload,
  settingsToDraft,
} from "../components/app-settings/aha/AhaAgentPanel";
import type { DispatcherSettings } from "../types";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

const invokeMock = vi.mocked(invoke);

function settings(): DispatcherSettings {
  return {
    apiBase: "https://legacy.example.com/v1",
    apiKey: "sk-legacy",
    model: "legacy-chat",
    summaryModel: "legacy-summary",
    visionModel: "legacy-vision",
    asrApiKey: "sk-asr",
    asrWebsocketUrl: "wss://asr.example.com/ws",
    autoApproveDispatch: false,
    contextDebug: false,
    imageModelUrl: "https://image.example.com/api/v1",
    imageModelApiKey: "sk-image",
    imageModel: "image-gen",
    imageEditModel: "",
    chatModelConfig: { url: "", apiKey: "", model: "", active: true },
    summaryModelConfig: { url: "", apiKey: "", model: "", active: true },
    visionModelConfig: { url: "", apiKey: "", model: "", active: true },
    imageModelConfig: { url: "", apiKey: "", model: "", active: true },
    imageEditModelConfig: { url: "", apiKey: "", model: "", active: true },
    asrModelConfig: { url: "", apiKey: "", model: "", active: true },
    ttsModelConfig: { url: "", apiKey: "", model: "", active: true },
    embeddingModelConfig: { url: "", apiKey: "", model: "", active: true },
    chatModelConfigs: [],
    summaryModelConfigs: [],
    visionModelConfigs: [],
    imageModelConfigs: [],
    imageEditModelConfigs: [],
    asrModelConfigs: [],
    ttsModelConfigs: [],
    embeddingModelConfigs: [],
  };
}

describe("AhaAgentPanel", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("maps legacy settings into isolated model configs", () => {
    const draft = settingsToDraft(settings());

    expect(draft.chatModelConfigs[0]).toEqual({
      url: "https://legacy.example.com/v1",
      apiKey: "sk-legacy",
      model: "legacy-chat",
      active: true,
    });
    expect(draft.visionModelConfigs[0]).toEqual({
      url: "https://legacy.example.com/v1",
      apiKey: "sk-legacy",
      model: "legacy-vision",
      active: true,
    });
    expect(draft.imageEditModelConfigs[0].model).toBe("image-gen");
    expect(draft.asrModelConfigs[0]).toEqual({
      url: "wss://asr.example.com/ws",
      apiKey: "sk-asr",
      model: "fun-asr-realtime",
      active: true,
    });
  });

  it("saves structured configs while preserving legacy compatibility fields", () => {
    const draft = settingsToDraft(settings());
    draft.chatModelConfigs = [
      { url: "https://chat.example.com/v1", apiKey: "sk-chat", model: "chat-main", active: false },
      {
        url: "https://chat-2.example.com/v1",
        apiKey: "sk-chat-2",
        model: "chat-active",
        active: true,
      },
    ];
    draft.imageModelConfigs = [
      {
        url: "https://image.example.com/api/v1",
        apiKey: "sk-image",
        model: "image-gen",
        active: true,
      },
    ];
    draft.imageEditModelConfigs = [{ url: "", apiKey: "", model: "", active: true }];

    const payload = draftToSavePayload(draft);

    expect(payload.apiBase).toBe("https://chat-2.example.com/v1");
    expect(payload.apiKey).toBe("sk-chat-2");
    expect(payload.model).toBe("chat-active");
    expect(payload.chatModelConfig).toEqual(draft.chatModelConfigs[1]);
    expect(payload.chatModelConfigs).toEqual(draft.chatModelConfigs);
    expect(payload.imageEditModel).toBe("image-gen");
    expect(payload.imageEditModelConfig).toEqual({
      url: "https://image.example.com/api/v1",
      apiKey: "sk-image",
      model: "image-gen",
      active: true,
    });
  });

  it("allows no active provider when the active badge is clicked again", async () => {
    invokeMock.mockImplementation(async (command: string, payload?: unknown) => {
      if (command === "dispatcher_get_settings") return settings();
      if (command === "dispatcher_save_settings") {
        return (payload as { settings: DispatcherSettings }).settings;
      }
      throw new Error(`unexpected command: ${command}`);
    });

    render(<AhaAgentPanel />);

    await screen.findByText("主聊天模型");
    fireEvent.click(screen.getAllByRole("button", { name: "已激活" })[0]);
    fireEvent.click(screen.getByRole("button", { name: "保存" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "dispatcher_save_settings",
        expect.objectContaining({
          settings: expect.objectContaining({
            apiBase: "",
            apiKey: "",
            model: "",
            chatModelConfigs: [expect.objectContaining({ model: "legacy-chat", active: false })],
          }),
        }),
      );
    });
  });

  it("keeps only one active provider per model kind", async () => {
    invokeMock.mockImplementation(async (command: string, payload?: unknown) => {
      if (command === "dispatcher_get_settings") return settings();
      if (command === "dispatcher_save_settings") {
        return (payload as { settings: DispatcherSettings }).settings;
      }
      throw new Error(`unexpected command: ${command}`);
    });

    render(<AhaAgentPanel />);

    await screen.findByText("主聊天模型");
    fireEvent.click(screen.getAllByRole("button", { name: "添加 Provider" })[0]);
    fireEvent.click(screen.getByRole("button", { name: "激活" }));
    fireEvent.click(screen.getByRole("button", { name: "保存" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "dispatcher_save_settings",
        expect.objectContaining({
          settings: expect.objectContaining({
            chatModelConfigs: [
              expect.objectContaining({ model: "legacy-chat", active: false }),
              expect.objectContaining({ active: true }),
            ],
          }),
        }),
      );
    });
  });

  it("deletes the last provider instead of disabling the delete button", async () => {
    invokeMock.mockImplementation(async (command: string, payload?: unknown) => {
      if (command === "dispatcher_get_settings") return settings();
      if (command === "dispatcher_save_settings") {
        return (payload as { settings: DispatcherSettings }).settings;
      }
      throw new Error(`unexpected command: ${command}`);
    });

    render(<AhaAgentPanel />);

    await screen.findByText("主聊天模型");
    fireEvent.click(screen.getAllByRole("button", { name: "删除" })[0]);
    fireEvent.click(screen.getByRole("button", { name: "保存" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "dispatcher_save_settings",
        expect.objectContaining({
          settings: expect.objectContaining({
            apiBase: "",
            apiKey: "",
            model: "",
            chatModelConfigs: [],
          }),
        }),
      );
    });
  });

  it("tests only the model config for the active clicked section", async () => {
    invokeMock.mockImplementation(async (command: string) => {
      if (command === "dispatcher_get_settings") return settings();
      if (command === "dispatcher_test_model") return "视觉模型 ok：pong";
      throw new Error(`unexpected command: ${command}`);
    });

    render(<AhaAgentPanel />);

    fireEvent.click(await screen.findByRole("tab", { name: /视觉模型/ }));
    fireEvent.click(screen.getByRole("button", { name: "测试当前 Provider" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("dispatcher_test_model", {
        kind: "vision",
        config: {
          url: "https://legacy.example.com/v1",
          apiKey: "sk-legacy",
          model: "legacy-vision",
          active: true,
        },
      });
      expect(screen.getByText("视觉模型 ok：pong")).toBeInTheDocument();
    });
  });

  it("fetches model list with the active model url and key", async () => {
    invokeMock.mockImplementation(async (command: string) => {
      if (command === "dispatcher_get_settings") return settings();
      if (command === "dispatcher_fetch_models") return ["legacy-chat", "chat-next"];
      throw new Error(`unexpected command: ${command}`);
    });

    render(<AhaAgentPanel />);

    const fetchButtons = await screen.findAllByRole("button", { name: "获取模型" });
    fireEvent.click(fetchButtons[0]);

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("dispatcher_fetch_models", {
        apiBase: "https://legacy.example.com/v1",
        apiKey: "sk-legacy",
      });
      expect(screen.getByRole("button", { name: "legacy-chat" })).toBeInTheDocument();
    });
  });

  it("adds another provider and saves the activated provider for legacy runtime fields", async () => {
    invokeMock.mockImplementation(async (command: string, payload?: unknown) => {
      if (command === "dispatcher_get_settings") return settings();
      if (command === "dispatcher_save_settings") {
        return (payload as { settings: DispatcherSettings }).settings;
      }
      throw new Error(`unexpected command: ${command}`);
    });

    render(<AhaAgentPanel />);

    await screen.findByText("主聊天模型");
    fireEvent.click(screen.getAllByRole("button", { name: "添加 Provider" })[0]);
    fireEvent.click(screen.getByRole("button", { name: /Provider 2/ }));

    const urlInputs = screen.getAllByPlaceholderText("https://api.example.com/v1");
    fireEvent.change(urlInputs[0], { target: { value: "https://second.example.com/v1" } });
    const keyInputs = screen.getAllByPlaceholderText("sk-...");
    fireEvent.change(keyInputs[0], { target: { value: "sk-second" } });
    const modelInputs = screen.getAllByPlaceholderText("model-name");
    fireEvent.change(modelInputs[0], { target: { value: "second-chat" } });
    fireEvent.click(screen.getAllByRole("button", { name: "激活" })[0]);
    fireEvent.click(screen.getByRole("button", { name: "保存" }));

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "dispatcher_save_settings",
        expect.objectContaining({
          settings: expect.objectContaining({
            apiBase: "https://second.example.com/v1",
            apiKey: "sk-second",
            model: "second-chat",
            chatModelConfigs: [
              expect.objectContaining({ model: "legacy-chat", active: false }),
              expect.objectContaining({ model: "second-chat", active: true }),
            ],
          }),
        }),
      );
    });
  });
});
