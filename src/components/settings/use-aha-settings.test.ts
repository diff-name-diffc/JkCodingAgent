import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AhaSettingsV2, ChatCategoryAgentConfig } from "../../types";
import type { AhaSettingsStore } from "./use-aha-settings";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  toastError: vi.fn(),
  subscribe: null as null | ((subscriber: () => void) => () => void),
  read: null as null | (() => AhaSettingsStore),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("../Toast", () => ({ toast: { error: mocks.toastError } }));
vi.mock("../../lib/query-client", () => ({
  queryClient: { invalidateQueries: vi.fn().mockResolvedValue(undefined) },
}));
vi.mock("../../hooks/use-chat-queries", () => ({ CHAT_MODELS_QUERY_KEY: ["chat-models"] }));
vi.mock("react", async (importOriginal) => ({
  ...(await importOriginal<typeof import("react")>()),
  useSyncExternalStore: (
    subscribe: (subscriber: () => void) => () => void,
    getSnapshot: () => AhaSettingsStore,
  ) => {
    mocks.subscribe = subscribe;
    mocks.read = getSnapshot;
    return getSnapshot();
  },
}));

const initialSettings = { theme: "system" } as AhaSettingsV2;
const initialCategory: ChatCategoryAgentConfig = {
  categoryId: "category-a",
  categoryName: "工程",
  allowedTools: [],
  systemPrompt: "原提示词",
  subAgentIds: [],
  projectDirectories: [],
  createdAt: "2026-10-16T00:00:00Z",
  updatedAt: "2026-10-16T00:00:00Z",
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

let unsubscribe: (() => void) | undefined;

beforeEach(() => {
  vi.resetModules();
  vi.useFakeTimers();
  mocks.invoke.mockReset();
  mocks.toastError.mockReset();
  mocks.subscribe = null;
  mocks.read = null;
});

afterEach(() => {
  unsubscribe?.();
  vi.clearAllTimers();
  vi.useRealTimers();
});

async function loadStore(
  saveCategories: (configs: ChatCategoryAgentConfig[]) => Promise<ChatCategoryAgentConfig[]>,
) {
  mocks.invoke.mockImplementation((command, args) => {
    switch (command) {
      case "aha_get_settings_v2":
        return Promise.resolve(initialSettings);
      case "aha_get_chat_category_agent_configs":
        return Promise.resolve([initialCategory]);
      case "sub_agent_list":
        return Promise.resolve([]);
      case "aha_save_settings_v2":
        return Promise.resolve(args.settings);
      case "aha_save_chat_category_agent_configs":
        return saveCategories(args.configs);
      case "sub_agent_set_global_enabled":
        return Promise.resolve(undefined);
      default:
        throw new Error(`未模拟的命令：${command}`);
    }
  });
  // useSyncExternalStore 已替换为测试订阅器，直接连接模块级保存管线。
  const { useAhaSettingsStore: connectStore } = await import("./use-aha-settings");
  connectStore();
  unsubscribe = mocks.subscribe!(() => {});
  await vi.advanceTimersByTimeAsync(0);
  return mocks.read!;
}

describe("分类工程目录自动保存", () => {
  it("回填服务端 canonical 路径，异步目录选择继续基于最新目录追加", async () => {
    const read = await loadStore(async (configs) =>
      configs.map((config) => ({
        ...config,
        projectDirectories: config.projectDirectories.map((path) =>
          path.replace("/linked/", "/real/"),
        ),
      })),
    );
    const originalStore = read();
    originalStore.updateChatCategoryConfigs((configs) =>
      configs.map((config) => ({ ...config, projectDirectories: ["/linked/project-a"] })),
    );
    await originalStore.flush();
    expect(read().chatCategoryConfigs[0].projectDirectories).toEqual(["/real/project-a"]);

    // 模拟选择器打开期间完成自动保存，再使用打开前捕获的 action 添加另一目录。
    originalStore.updateChatCategoryConfigs((configs) =>
      configs.map((config) => ({
        ...config,
        projectDirectories: [...config.projectDirectories, "/real/project-b"],
      })),
    );
    expect(read().chatCategoryConfigs[0].projectDirectories).toEqual([
      "/real/project-a",
      "/real/project-b",
    ]);
  });

  it("保存期间的新目录和提示词不被旧响应覆盖，补存后再回填 canonical 路径", async () => {
    const first = deferred<ChatCategoryAgentConfig[]>();
    const second = deferred<ChatCategoryAgentConfig[]>();
    const requests: ChatCategoryAgentConfig[][] = [];
    const read = await loadStore((configs) => {
      requests.push(configs);
      return requests.length === 1 ? first.promise : second.promise;
    });
    read().updateChatCategoryConfigs((configs) =>
      configs.map((config) => ({ ...config, projectDirectories: ["/linked/project-a"] })),
    );
    const saving = read().flush();
    read().updateChatCategoryConfigs((configs) =>
      configs.map((config) => ({
        ...config,
        systemPrompt: "新提示词",
        projectDirectories: [...config.projectDirectories, "/real/project-b"],
      })),
    );
    first.resolve([{ ...requests[0][0], projectDirectories: ["/real/project-a"] }]);
    await saving;
    expect(requests).toHaveLength(2);
    expect(requests[1][0]).toMatchObject({
      systemPrompt: "新提示词",
      projectDirectories: ["/linked/project-a", "/real/project-b"],
    });
    expect(read().chatCategoryConfigs[0]).toEqual(requests[1][0]);

    second.resolve([
      { ...requests[1][0], projectDirectories: ["/real/project-a", "/real/project-b"] },
    ]);
    await vi.advanceTimersByTimeAsync(0);
    expect(read().chatCategoryConfigs[0]).toMatchObject({
      systemPrompt: "新提示词",
      projectDirectories: ["/real/project-a", "/real/project-b"],
    });
    expect(mocks.toastError).not.toHaveBeenCalled();
  });
});
