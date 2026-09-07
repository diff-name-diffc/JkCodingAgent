import { describe, expect, it } from "vitest";
import {
  createShikiHighlightCache,
  shikiCacheKey,
} from "./shiki-cache";

describe("shikiCacheKey — 键构成", () => {
  it("语言与主题参与键：同代码不同主题/语言不撞键", () => {
    const code = '{"a":1}';
    expect(shikiCacheKey(code, "json", false)).not.toBe(shikiCacheKey(code, "json", true));
    expect(shikiCacheKey(code, "json", false)).not.toBe(shikiCacheKey(code, "js", false));
  });

  it("同参数键稳定", () => {
    expect(shikiCacheKey("x", "json", true)).toBe(shikiCacheKey("x", "json", true));
  });
});

describe("createShikiHighlightCache — LRU 语义", () => {
  it("未写入 miss，写入后 hit", () => {
    const cache = createShikiHighlightCache(8);
    expect(cache.get("k1")).toBeUndefined();
    cache.set("k1", "<pre>1</pre>", 3);
    expect(cache.get("k1")).toBe("<pre>1</pre>");
    expect(cache.size).toBe(1);
  });

  it("超容量淘汰最久未用（插入序最旧）", () => {
    const cache = createShikiHighlightCache(2);
    cache.set("k1", "a", 1);
    cache.set("k2", "b", 1);
    cache.set("k3", "c", 1);
    expect(cache.get("k1")).toBeUndefined();
    expect(cache.get("k2")).toBe("b");
    expect(cache.get("k3")).toBe("c");
    expect(cache.size).toBe(2);
  });

  it("get 命中提升新鲜度：随后淘汰绕过刚读过的条目", () => {
    const cache = createShikiHighlightCache(2);
    cache.set("k1", "a", 1);
    cache.set("k2", "b", 1);
    expect(cache.get("k1")).toBe("a"); // k1 提升为最新
    cache.set("k3", "c", 1); // 淘汰 k2
    expect(cache.get("k1")).toBe("a");
    expect(cache.get("k2")).toBeUndefined();
    expect(cache.get("k3")).toBe("c");
  });

  it("同键覆盖写不增容量", () => {
    const cache = createShikiHighlightCache(4);
    cache.set("k1", "a", 1);
    cache.set("k1", "b", 1);
    expect(cache.size).toBe(1);
    expect(cache.get("k1")).toBe("b");
  });

  it("超长代码不入缓存（防单条目内存过大）", () => {
    const cache = createShikiHighlightCache(4);
    cache.set("k1", "<pre/>", 64 * 1024 + 1);
    expect(cache.get("k1")).toBeUndefined();
    expect(cache.size).toBe(0);
    cache.set("k2", "<pre/>", 64 * 1024);
    expect(cache.get("k2")).toBe("<pre/>");
  });

  it("clear 清空全部", () => {
    const cache = createShikiHighlightCache(4);
    cache.set("k1", "a", 1);
    cache.set("k2", "b", 1);
    cache.clear();
    expect(cache.size).toBe(0);
    expect(cache.get("k1")).toBeUndefined();
  });
});
