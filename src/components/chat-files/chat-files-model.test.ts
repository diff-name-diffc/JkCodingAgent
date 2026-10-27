import { describe, expect, it } from "vitest";
import type { ChatWorkspaceOverview } from "../../types";
import {
  abbreviateHomePath,
  buildChatFileRoots,
  formatDisplayPath,
  inferHomeDir,
  middleEllipsis,
  pathBaseName,
  pickDefaultSessionId,
  pickInitialRoot,
  splitPathDisplay,
} from "./chat-files-model";

function overview(partial: Partial<ChatWorkspaceOverview>): ChatWorkspaceOverview {
  return { sessionDir: null, sessionDirExists: false, projectDirs: [], ...partial };
}

describe("pickDefaultSessionId", () => {
  it("优先文件入口选中的会话（属于该分类时）", () => {
    const sessions = [{ id: "a" }, { id: "b" }];
    expect(pickDefaultSessionId("b", sessions)).toBe("b");
  });

  it("入口会话不属于该分类时取首个会话", () => {
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

describe("inferHomeDir", () => {
  it("以会话临时目录的 .jkcodingagent 段反推 home", () => {
    expect(
      inferHomeDir(["/Users/jk/.jkcodingagent/plain-chat-browser/s1", "/Users/jk/work/a"]),
    ).toBe("/Users/jk");
  });

  it("兼容 Windows 分隔符", () => {
    expect(inferHomeDir([null, "C:\\Users\\jk\\.jkcodingagent\\plain-chat-browser\\s1"])).toBe(
      "C:\\Users\\jk",
    );
  });

  it("无锚点或路径为空时返回 null", () => {
    expect(inferHomeDir([null, undefined, "/opt/shared/a"])).toBeNull();
    expect(inferHomeDir(["/.jkcodingagent/plain-chat-browser/s1"])).toBeNull();
    expect(inferHomeDir([])).toBeNull();
  });
});

describe("abbreviateHomePath", () => {
  it("home 前缀缩成 ~，相等时只剩 ~", () => {
    expect(abbreviateHomePath("/Users/jk/work/a", "/Users/jk")).toBe("~/work/a");
    expect(abbreviateHomePath("/Users/jk", "/Users/jk")).toBe("~");
    expect(abbreviateHomePath("/Users/jk/", "/Users/jk")).toBe("~");
  });

  it("非同名前缀不误缩；无 home 时原样返回", () => {
    expect(abbreviateHomePath("/Users/jk2/work/a", "/Users/jk")).toBe("/Users/jk2/work/a");
    expect(abbreviateHomePath("/opt/a", null)).toBe("/opt/a");
    expect(abbreviateHomePath("", "/Users/jk")).toBe("");
  });
});

describe("middleEllipsis", () => {
  it("不超预算时原样返回（含边界）", () => {
    expect(middleEllipsis("abcdef", 6)).toBe("abcdef");
    expect(middleEllipsis("abc", 6)).toBe("abc");
  });

  it("超预算时保留头尾，长度恰为 maxChars 且尾部优先可信", () => {
    const result = middleEllipsis("~/work/python/ai/aha-coding", 12);
    expect(result).toHaveLength(12);
    expect(result).toContain("…");
    expect(result.startsWith("~/work")).toBe(true);
    expect(result.endsWith("oding")).toBe(true);
  });

  it("极小预算安全退化", () => {
    expect(middleEllipsis("abcdef", 1)).toBe("…");
    expect(middleEllipsis("a", 1)).toBe("a");
    expect(middleEllipsis("abcdef", 0)).toBe("");
    expect(middleEllipsis("abcdef", 2)).toBe("a…");
  });
});

describe("formatDisplayPath", () => {
  it("先缩 home 再做中间省略", () => {
    const path = "/Users/jk/work/java/any_file_server";
    expect(formatDisplayPath(path, { homeDir: "/Users/jk" })).toBe("~/work/java/any_file_server");
    const long = "/Users/jk/.jkcodingagent/plain-chat-browser/455ea6fb-2831-4a1b";
    const display = formatDisplayPath(long, { homeDir: "/Users/jk", maxChars: 20 });
    expect(display).toHaveLength(20);
    expect(display.startsWith("~/.")).toBe(true);
    expect(display.endsWith("a1b")).toBe(true);
  });

  it("缺省预算 30 字符、无 home 时保留绝对路径开头", () => {
    const display = formatDisplayPath("/opt/shared/very-long-directory-name/another-deep-one");
    expect(display).toHaveLength(30);
    expect(display.startsWith("/opt/shared")).toBe(true);
  });
});

describe("splitPathDisplay", () => {
  it("末段作名称、父目录作缩写路径", () => {
    expect(
      splitPathDisplay("/Users/jk/work/java/deploy-console", { homeDir: "/Users/jk" }),
    ).toEqual({ name: "deploy-console", parent: "~/work/java" });
  });

  it("home 下的直接子目录父路径为 ~；根路径无父目录", () => {
    expect(splitPathDisplay("/Users/jk/notes", { homeDir: "/Users/jk" })).toEqual({
      name: "notes",
      parent: "~",
    });
    expect(splitPathDisplay("/")).toEqual({ name: "/", parent: "" });
  });

  it("会话临时目录的长父路径按预算省略", () => {
    const result = splitPathDisplay(
      "/Users/jk/.jkcodingagent/plain-chat-browser/455ea6fb-2831-4a1b99",
      { homeDir: "/Users/jk", maxChars: 18 },
    );
    expect(result.name).toBe("455ea6fb-2831-4a1b99");
    expect(result.parent).toHaveLength(18);
    expect(result.parent.startsWith("~/")).toBe(true);
  });
});
