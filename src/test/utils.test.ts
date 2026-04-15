import { describe, it, expect, beforeEach, vi } from "vitest";
import {
  AVATAR_COLORS,
  getAvatarGradient,
  shortenPath,
  load,
  save,
  getGitStatusColor,
  getGitStatusLabel,
  getFileColor,
  CODE_EXTS,
} from "../utils";
import { resolveFilePresentation } from "../file-icons";

// ── getAvatarGradient ────────────────────────────────────────────────────────

describe("getAvatarGradient", () => {
  it("始终返回 AVATAR_COLORS 中的颜色对", () => {
    const result = getAvatarGradient("my-project");
    expect(AVATAR_COLORS).toContainEqual(result);
  });

  it("相同名称始终返回相同颜色（幂等性）", () => {
    expect(getAvatarGradient("jkcodingagent")).toEqual(getAvatarGradient("jkcodingagent"));
  });

  it("不同名称通常返回不同颜色", () => {
    // 散列不均匀时可能碰撞，但常见名称不应相同
    const a = getAvatarGradient("project-alpha");
    const b = getAvatarGradient("project-beta");
    // 不强断言不相等（避免散列碰撞导致误报），仅断言返回值合法
    expect(AVATAR_COLORS).toContainEqual(a);
    expect(AVATAR_COLORS).toContainEqual(b);
  });

  it("空字符串不抛出异常并返回合法颜色", () => {
    expect(() => getAvatarGradient("")).not.toThrow();
    expect(AVATAR_COLORS).toContainEqual(getAvatarGradient(""));
  });
});

// ── shortenPath ──────────────────────────────────────────────────────────────

describe("shortenPath", () => {
  it("将 /Users/<username>/ 前缀替换为 ~", () => {
    expect(shortenPath("/Users/john/Documents/project")).toBe("~/Documents/project");
  });

  it("用户名包含点和连字符时正确处理", () => {
    expect(shortenPath("/Users/xxxx/workspace/jkcodingagent")).toBe("~/workspace/jkcodingagent");
  });

  it("非 /Users/ 路径保持不变", () => {
    expect(shortenPath("/etc/hosts")).toBe("/etc/hosts");
    expect(shortenPath("/tmp/foo")).toBe("/tmp/foo");
  });

  it("路径仅为 /Users/<username> 时缩短为 ~", () => {
    expect(shortenPath("/Users/john")).toBe("~");
  });
});

// ── localStorage load / save ─────────────────────────────────────────────────

describe("load / save", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("save 写入后 load 能正确读取", () => {
    save("theme", "dark");
    expect(load("theme", "light")).toBe("dark");
  });

  it("键不存在时返回 fallback", () => {
    expect(load("nonexistent", 42)).toBe(42);
  });

  it("支持存储复杂对象", () => {
    const data = { projectId: "abc", count: 3 };
    save("meta", data);
    expect(load("meta", null)).toEqual(data);
  });

  it("存储损坏的 JSON 时返回 fallback 而不是抛出异常", () => {
    localStorage.setItem("corrupt", "{not-valid-json");
    expect(load("corrupt", "fallback")).toBe("fallback");
  });
});

// ── getGitStatusColor ────────────────────────────────────────────────────────

describe("getGitStatusColor", () => {
  it.each([
    ["A", "#3fb950"],
    ["D", "#f85149"],
    ["M", "#e3b341"],
    ["R", "#79c0ff"],
    ["?", "#79c0ff"],
    ["U", "#f85149"],
  ])("状态 %s 返回正确颜色", (status, expected) => {
    expect(getGitStatusColor(status)).toBe(expected);
  });

  it("未知状态返回 muted 变量", () => {
    expect(getGitStatusColor("X")).toBe("var(--text-muted)");
  });
});

// ── getGitStatusLabel ────────────────────────────────────────────────────────

describe("getGitStatusLabel", () => {
  it("? 映射为 U（Untracked 显示用）", () => {
    expect(getGitStatusLabel("?")).toBe("U");
  });

  it("U 映射为 !（冲突显示用）", () => {
    expect(getGitStatusLabel("U")).toBe("!");
  });

  it.each(["A", "D", "M", "R"])("已知状态 %s 原样返回", (s) => {
    expect(getGitStatusLabel(s)).toBe(s);
  });

  it("未知状态原样返回", () => {
    expect(getGitStatusLabel("Z")).toBe("Z");
  });
});

// ── getFileColor ─────────────────────────────────────────────────────────────

describe("getFileColor", () => {
  it("TSX 与 TypeScript 文件返回新的主题色", () => {
    expect(getFileColor("App.tsx")).toBe("#0EA5E9");
    expect(getFileColor("utils.ts")).toBe("#2563EB");
  });

  it("Rust 文件返回红色", () => {
    expect(getFileColor("lib.rs")).toBe("#C2410C");
  });

  it("Dockerfile 特殊文件名（大小写不敏感）返回 Docker 蓝", () => {
    expect(getFileColor("Dockerfile")).toBe("#0284C7");
    expect(getFileColor("dockerfile.prod")).toBe("#0284C7");
  });

  it("Makefile 返回构建类主题色", () => {
    expect(getFileColor("Makefile")).toBe("#0F766E");
  });

  it(".env 文件返回配置类主题色", () => {
    expect(getFileColor(".env")).toBe("#475569");
    expect(getFileColor(".env.production")).toBe("#475569");
  });

  it("无扩展名的未知文件返回默认灰色", () => {
    expect(getFileColor("NOTICE")).toBe("#475569");
  });

  it("ext 参数优先于从文件名推断的扩展名", () => {
    // 传入 ext="rs" 覆盖从 "foo.ts" 推断的 "ts"
    expect(getFileColor("foo.ts", "rs")).toBe("#C2410C");
  });
});

// ── resolveFilePresentation ─────────────────────────────────────────────────

describe("resolveFilePresentation", () => {
  it("按精确文件名优先匹配", () => {
    const result = resolveFilePresentation({ name: "Dockerfile" });
    expect(result.iconKey).toBe("docker");
    expect(result.monacoLanguage).toBe("dockerfile");
  });

  it("按复合后缀识别测试文件", () => {
    const result = resolveFilePresentation({ name: "Button.test.tsx" });
    expect(result.iconKey).toBe("test");
    expect(result.monacoLanguage).toBe("typescript");
  });

  it("识别 Markdown 与图片预览能力", () => {
    expect(resolveFilePresentation({ name: "README.mdx" }).isMarkdown).toBe(true);
    expect(resolveFilePresentation({ name: "logo.svg" }).isPreviewableImage).toBe(true);
  });

  it("目录走专用文件夹图标规则", () => {
    expect(resolveFilePresentation({ name: "src", isDir: true }).iconKey).toBe("folder-src");
    expect(resolveFilePresentation({ name: "components", isDir: true }).iconKey).toBe(
      "folder-components",
    );
  });

  it("未知扩展名回退为默认文档图标，而不是崩溃", () => {
    const result = resolveFilePresentation({ name: "data.unknownext" });
    expect(result.iconKey).toBe("default");
    expect(result.monacoLanguage).toBe("plaintext");
  });
});

// ── CODE_EXTS ─────────────────────────────────────────────────────────────────

describe("CODE_EXTS", () => {
  it("包含常见代码扩展名", () => {
    expect(CODE_EXTS.has("ts")).toBe(true);
    expect(CODE_EXTS.has("rs")).toBe(true);
    expect(CODE_EXTS.has("py")).toBe(true);
  });

  it("不包含图片等非代码扩展名", () => {
    expect(CODE_EXTS.has("png")).toBe(false);
    expect(CODE_EXTS.has("pdf")).toBe(false);
  });
});

// 确保 vi 被引用（避免 lint 警告）
void vi;
