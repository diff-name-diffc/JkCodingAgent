import { describe, expect, it } from "vitest";
import type { ChatWorkspaceOverview } from "../../types";
import {
  buildChatFileRoots,
  pathBaseName,
  pickDefaultSessionId,
  pickInitialRoot,
} from "./chat-files-model";

function overview(partial: Partial<ChatWorkspaceOverview>): ChatWorkspaceOverview {
  return { sessionDir: null, sessionDirExists: false, projectDirs: [], ...partial };
}

describe("pickDefaultSessionId", () => {
  it("优先当前活跃会话（属于该分类时）", () => {
    const sessions = [{ id: "a" }, { id: "b" }];
    expect(pickDefaultSessionId("b", sessions)).toBe("b");
  });

  it("活跃会话不属于该分类时取首个会话", () => {
    const sessions = [{ id: "a" }, { id: "b" }];
    expect(pickDefaultSessionId("z", sessions)).toBe("a");
    expect(pickDefaultSessionId(null, sessions)).toBe("a");
  });

  it("无会话时返回 null", () => {
    expect(pickDefaultSessionId("a", [])).toBeNull();
  });
});

describe("buildChatFileRoots", () => {
  it("工程目录在前（失效保留原因），会话目录殿后；无会话目录则缺省", () => {
    const roots = buildChatFileRoots(
      overview({
        projectDirs: [
          { raw: "/tmp/raw-a", canonical: "/real/a", error: null },
          { raw: "/tmp/gone", canonical: null, error: "共享工程目录不存在或无法访问" },
        ],
        sessionDir: "/app/plain-chat-browser/s1",
        sessionDirExists: true,
      }),
    );
    expect(roots.map((root) => root.kind)).toEqual(["project", "project", "session"]);
    expect(roots[0].path).toBe("/real/a");
    expect(roots[0].error).toBeNull();
    expect(roots[1].path).toBe("/tmp/gone");
    expect(roots[1].error).toContain("不存在");
    expect(roots[2].path).toBe("/app/plain-chat-browser/s1");

    const withoutSession = buildChatFileRoots(overview({}));
    expect(withoutSession).toHaveLength(0);
  });

  it("总览未加载时返回空列表", () => {
    expect(buildChatFileRoots(undefined)).toEqual([]);
  });
});

describe("pickInitialRoot", () => {
  it("取首个有效工程目录", () => {
    const roots = buildChatFileRoots(
      overview({
        projectDirs: [
          { raw: "/gone", canonical: null, error: "失效" },
          { raw: "/tmp/b", canonical: "/real/b", error: null },
        ],
        sessionDir: "/app/s1",
      }),
    );
    expect(pickInitialRoot(roots)).toBe("/real/b");
  });

  it("工程目录全部失效时回退会话临时目录", () => {
    const roots = buildChatFileRoots(
      overview({
        projectDirs: [{ raw: "/gone", canonical: null, error: "失效" }],
        sessionDir: "/app/s1",
      }),
    );
    expect(pickInitialRoot(roots)).toBe("/app/s1");
  });

  it("无任何根时返回 null", () => {
    expect(pickInitialRoot([])).toBeNull();
  });
});

describe("pathBaseName", () => {
  it("取最后一段并容忍尾部分隔符", () => {
    expect(pathBaseName("/Users/jk/work/space")).toBe("space");
    expect(pathBaseName("/Users/jk/work/space/")).toBe("space");
    expect(pathBaseName("relative")).toBe("relative");
    expect(pathBaseName("/")).toBe("");
  });
});
