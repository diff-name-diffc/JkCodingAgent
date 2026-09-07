import { describe, expect, it } from "vitest";
import type { McpConfig } from "../../../types";
import {
  nextServerName,
  parseConfigText,
  parseKeyValueLines,
  parseLines,
  serializeConfig,
  toConfig,
  toEntries,
  type McpEntry,
} from "./mcp-config";

function entry(name: string): McpEntry {
  return {
    name,
    server: { enabled: true, transport: "stdio", command: "npx", args: [], env: {}, headers: {} },
  };
}

describe("toEntries / toConfig 往返", () => {
  it("保持插入顺序并补齐缺省数组/对象", () => {
    // 模拟后端 wire 可能省略 args/env/headers，验证 toEntries 的防御性补齐。
    const config = {
      mcpServers: {
        a: { enabled: true, transport: "stdio", command: "x" },
        b: { enabled: false, transport: "streamable_http", url: "http://h" },
      },
    } as unknown as McpConfig;
    const entries = toEntries(config);
    expect(entries.map((e) => e.name)).toEqual(["a", "b"]);
    expect(entries[0].server.args).toEqual([]);
    expect(entries[0].server.env).toEqual({});
    expect(entries[0].server.headers).toEqual({});
    // 往返回到等价配置
    expect(toConfig(entries)).toEqual({
      mcpServers: {
        a: { enabled: true, transport: "stdio", command: "x", args: [], env: {}, headers: {} },
        b: {
          enabled: false,
          transport: "streamable_http",
          url: "http://h",
          args: [],
          env: {},
          headers: {},
        },
      },
    });
  });

  it("toConfig 跳过空白名称", () => {
    const config = toConfig([entry("ok"), entry("   "), entry("")]);
    expect(Object.keys(config.mcpServers ?? {})).toEqual(["ok"]);
  });

  it("名称两端空白被裁剪为键", () => {
    const config = toConfig([entry("  spaced  ")]);
    expect(Object.keys(config.mcpServers ?? {})).toEqual(["spaced"]);
  });

  it("空 mcpServers / undefined 得到空条目列表", () => {
    expect(toEntries({ mcpServers: {} })).toEqual([]);
    expect(toEntries({} as McpConfig)).toEqual([]);
  });
});

describe("parseLines", () => {
  it("按行分割、去空白、过滤空行", () => {
    expect(parseLines("a\n  b  \n\n   \nc")).toEqual(["a", "b", "c"]);
    expect(parseLines("")).toEqual([]);
  });
});

describe("parseKeyValueLines", () => {
  it("解析 KEY=VALUE（env）", () => {
    expect(parseKeyValueLines("A=1\nB = two \n=novalue\nC=", /^([^=]+)=/)).toEqual({
      A: "1",
      B: "two",
      C: "",
    });
  });

  it("解析 Header: Value（含冒号后空白）", () => {
    expect(parseKeyValueLines("Authorization: Bearer x\nPlain:v", /^([^:]+):\s*/)).toEqual({
      Authorization: "Bearer x",
      Plain: "v",
    });
  });

  it("无匹配分隔符的行被忽略", () => {
    expect(parseKeyValueLines("garbage\nA=1", /^([^=]+)=/)).toEqual({ A: "1" });
  });
});

describe("nextServerName", () => {
  it("空列表从 mcp-1 起", () => {
    expect(nextServerName([])).toBe("mcp-1");
  });

  it("按长度递增", () => {
    expect(nextServerName([entry("mcp-1")])).toBe("mcp-2");
  });

  it("跳过已占用名称", () => {
    // 长度 2 → 起始 index 3 → "mcp-3"，但 mcp-3 已占用 → 跳到 mcp-4
    expect(nextServerName([entry("mcp-1"), entry("mcp-3")])).toBe("mcp-4");
    // 长度 1 → 起始 index 2 → "mcp-2" 已占用 → 跳到 mcp-3
    expect(nextServerName([entry("mcp-2")])).toBe("mcp-3");
    expect(nextServerName([entry("mcp-1"), entry("mcp-2"), entry("mcp-3")])).toBe("mcp-4");
  });
});

describe("parseConfigText", () => {
  it("合法 mcpServers", () => {
    const result = parseConfigText('{"mcpServers":{"a":{"transport":"stdio"}}}');
    expect(result.error).toBeNull();
    expect(result.config?.mcpServers?.a).toBeDefined();
  });

  it("兼容别名键 servers", () => {
    const result = parseConfigText('{"servers":{"a":{"transport":"stdio"}}}');
    expect(result.error).toBeNull();
    expect(result.config?.mcpServers?.a).toBeDefined();
  });

  it("缺失 mcpServers 视为空注册表", () => {
    const result = parseConfigText("{}");
    expect(result.error).toBeNull();
    expect(result.config).toEqual({ mcpServers: {} });
  });

  it("非法 JSON 返回解析错误", () => {
    const result = parseConfigText("{not json");
    expect(result.config).toBeNull();
    expect(result.error).toContain("JSON 解析失败");
  });

  it("顶层非对象被拒绝", () => {
    expect(parseConfigText("[1,2]").error).toContain("顶层必须是一个 JSON 对象");
    expect(parseConfigText('"str"').error).toContain("顶层必须是一个 JSON 对象");
    expect(parseConfigText("null").error).toContain("顶层必须是一个 JSON 对象");
  });

  it("mcpServers 非对象被拒绝", () => {
    expect(parseConfigText('{"mcpServers":[]}').error).toContain("mcpServers 必须是一个 JSON 对象");
  });

  it("单个服务器配置非对象被拒绝并带名称", () => {
    const result = parseConfigText('{"mcpServers":{"bad":"nope"}}');
    expect(result.config).toBeNull();
    expect(result.error).toContain("服务器「bad」");
  });
});

describe("serializeConfig", () => {
  it("两空格缩进并以换行结尾", () => {
    const text = serializeConfig({ mcpServers: {} });
    expect(text.endsWith("\n")).toBe(true);
    expect(text).toContain('  "mcpServers": {}');
  });

  it("serialize → parse 往返一致", () => {
    const config: McpConfig = {
      mcpServers: { a: { transport: "stdio", command: "x", args: [], env: {}, headers: {} } },
    };
    const parsed = parseConfigText(serializeConfig(config));
    expect(parsed.error).toBeNull();
    expect(parsed.config?.mcpServers?.a?.command).toBe("x");
  });
});
