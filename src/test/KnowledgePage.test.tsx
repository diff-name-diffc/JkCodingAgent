import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { confirm, open } from "@tauri-apps/plugin-dialog";
import { KnowledgePage } from "../components/knowledge/KnowledgePage";
import { KnowledgeSettingsPanel } from "../components/knowledge/KnowledgeSettingsPanel";

vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: (path: string) => `asset://${path}`,
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  confirm: vi.fn(),
  open: vi.fn(),
}));

vi.mock("@monaco-editor/react", () => ({
  default: () => <div data-testid="monaco-editor" />,
}));

const invokeMock = vi.mocked(invoke);
const confirmMock = vi.mocked(confirm);
const openMock = vi.mocked(open);

const collection = {
  id: "kc-1",
  name: "旧集合",
  rootPath: "/Users/me/.jkcodingagent/knowledge/collections/kc-1",
  createdAt: 1,
  updatedAt: 1,
};

async function baseInvoke(command: string) {
  if (command === "knowledge_get_settings") {
    return {
      textModel: { url: "", apiKey: "", model: "" },
      visionModel: { url: "", apiKey: "", model: "" },
      embeddingModel: { url: "", apiKey: "", model: "" },
    };
  }
  if (command === "knowledge_list_collections") return [];
  if (command === "knowledge_list_pages") return [];
  if (command === "knowledge_get_ingest_jobs") return [];
  if (command === "knowledge_vector_stats") {
    return { collectionId: "kc-1", pageCount: 0, chunkCount: 0, dimension: 0 };
  }
  throw new Error(`unexpected command: ${command}`);
}

function mockInvokeDefaults() {
  invokeMock.mockImplementation((command: string) => baseInvoke(command));
}

describe("KnowledgePage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockInvokeDefaults();
  });

  it("creates a collection through the Tauri command instead of browser prompt", async () => {
    invokeMock.mockImplementation(async (command: string, args?: unknown) => {
      if (command === "knowledge_create_collection") {
        expect(args).toEqual({ name: "资料库" });
        return { ...collection, name: "资料库" };
      }
      return baseInvoke(command);
    });

    render(<KnowledgePage />);

    fireEvent.click(await screen.findByRole("button", { name: /新建/ }));
    const input = screen.getByPlaceholderText("新集合名称");
    fireEvent.change(input, { target: { value: "资料库" } });
    fireEvent.submit(input.closest("form")!);

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("knowledge_create_collection", { name: "资料库" });
    });
  });

  it("wires collection management buttons to update import and delete commands", async () => {
    invokeMock.mockImplementation(async (command: string) => {
      if (command === "knowledge_list_collections") return [collection];
      if (command === "knowledge_update_collection") return { ...collection, name: "新集合" };
      if (command === "knowledge_import_sources") return [];
      if (command === "knowledge_delete_collection") return undefined;
      if (command === "knowledge_get_settings") {
        return {
          textModel: { url: "", apiKey: "", model: "" },
          visionModel: { url: "", apiKey: "", model: "" },
          embeddingModel: { url: "", apiKey: "", model: "" },
        };
      }
      if (command === "knowledge_list_pages") return [];
      if (command === "knowledge_get_ingest_jobs") return [];
      if (command === "knowledge_vector_stats") {
        return { collectionId: "kc-1", pageCount: 0, chunkCount: 0, dimension: 0 };
      }
      throw new Error(`unexpected command: ${command}`);
    });
    openMock.mockResolvedValue(["/tmp/a.md"]);
    confirmMock.mockResolvedValue(true);

    render(<KnowledgePage />);

    await waitFor(() => {
      expect(screen.getAllByText("旧集合").length).toBeGreaterThan(0);
    });

    fireEvent.click(screen.getByRole("button", { name: /重命名集合/ }));
    const renameInput = screen.getByPlaceholderText("集合名称");
    fireEvent.change(renameInput, { target: { value: "新集合" } });
    fireEvent.submit(renameInput.closest("form")!);

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("knowledge_update_collection", {
        collectionId: "kc-1",
        name: "新集合",
      });
    });

    fireEvent.click(screen.getAllByRole("button", { name: /导入源文件/ })[0]);
    await waitFor(() => {
      expect(openMock).toHaveBeenCalled();
      expect(invokeMock).toHaveBeenCalledWith("knowledge_import_sources", {
        collectionId: "kc-1",
        paths: ["/tmp/a.md"],
      });
    });

    fireEvent.click(screen.getByRole("button", { name: "删除集合" }));
    await waitFor(() => {
      expect(confirmMock).toHaveBeenCalled();
      expect(invokeMock).toHaveBeenCalledWith("knowledge_delete_collection", { collectionId: "kc-1" });
    });
  });

  it("wires knowledge model test buttons to the Tauri command", async () => {
    const settings = {
      textModel: { url: "https://api.example.com/v1", apiKey: "sk-text", model: "text-model" },
      visionModel: { url: "https://api.example.com/v1", apiKey: "sk-vision", model: "vision-model" },
      embeddingModel: { url: "https://api.example.com/v1", apiKey: "sk-embedding", model: "embedding-model" },
    };
    invokeMock.mockResolvedValue("text ok：pong");

    render(<KnowledgeSettingsPanel settings={settings} onSettingsSaved={vi.fn()} />);

    fireEvent.click(screen.getAllByRole("button", { name: "测试" })[0]);

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("knowledge_test_model", {
        kind: "text",
        settings,
      });
      expect(screen.getByText("text ok：pong")).toHaveStyle({ color: "var(--success)" });
    });
  });

  it("shows failed model test feedback next to the clicked button", async () => {
    const settings = {
      textModel: { url: "https://api.example.com/v1", apiKey: "sk-text", model: "text-model" },
      visionModel: { url: "https://api.example.com/v1", apiKey: "sk-vision", model: "vision-model" },
      embeddingModel: { url: "https://api.example.com/v1", apiKey: "sk-embedding", model: "embedding-model" },
    };
    invokeMock.mockRejectedValue("文本模型 API Key 未配置");

    render(<KnowledgeSettingsPanel settings={settings} onSettingsSaved={vi.fn()} />);

    const button = screen.getAllByRole("button", { name: "测试" })[0];
    fireEvent.click(button);

    await waitFor(() => {
      expect(screen.getByText("文本模型 API Key 未配置")).toHaveStyle({ color: "var(--danger)" });
      expect(button.parentElement).toHaveTextContent("文本模型 API Key 未配置");
    });
  });
});
