import { describe, expect, it } from "vitest";
import { serializeData } from "./tool-call-data";

describe("工具输入/输出展示序列化", () => {
  it("JSON 字符串美化重排", () => {
    expect(serializeData('{"a":1,"b":[2,3]}')).toBe(
      '{\n  "a": 1,\n  "b": [\n    2,\n    3\n  ]\n}',
    );
  });

  it("非 JSON 原文保留（流式参数未成形时不吞内容）", () => {
    expect(serializeData("not json {")).toBe("not json {");
    expect(serializeData("普通文本输出")).toBe("普通文本输出");
  });

  it("对象直接序列化为格式化 JSON", () => {
    expect(serializeData({ path: "/tmp/x" })).toBe('{\n  "path": "/tmp/x"\n}');
  });
});
