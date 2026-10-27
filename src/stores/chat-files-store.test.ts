import { beforeEach, describe, expect, it } from "vitest";
import { useChatFilesStore } from "./chat-files-store";

describe("chat-files-store", () => {
  beforeEach(() => {
    useChatFilesStore.getState().close();
  });

  it("记录文件入口的分类和会话，新入口替换旧目标", () => {
    const { open } = useChatFilesStore.getState();

    open("category-a", "session-a");
    expect(useChatFilesStore.getState().target).toEqual({
      categoryId: "category-a",
      sessionId: "session-a",
    });

    open("category-b", "session-b");
    expect(useChatFilesStore.getState().target).toEqual({
      categoryId: "category-b",
      sessionId: "session-b",
    });
  });

  it("关闭时清空文件入口目标", () => {
    useChatFilesStore.getState().open("category-a", "session-a");
    useChatFilesStore.getState().close();

    expect(useChatFilesStore.getState().target).toBeNull();
  });
});
