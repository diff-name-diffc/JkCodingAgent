import { describe, expect, it } from "vitest";
import {
  isSupportedHighlightLanguage,
  SUPPORTED_HIGHLIGHT_LANGUAGES,
  tokenizeCodeDualTheme,
} from "./shiki";

describe("isSupportedHighlightLanguage（streamdown 高亮插件能力面）", () => {
  it("支持按需加载集合内的语言", () => {
    expect(isSupportedHighlightLanguage("python")).toBe(true);
    expect(isSupportedHighlightLanguage("rust")).toBe(true);
    expect(isSupportedHighlightLanguage("tsx")).toBe(true);
  });

  it("别名归一化后命中（shell/py/md 等）", () => {
    expect(isSupportedHighlightLanguage("shell")).toBe(true);
    expect(isSupportedHighlightLanguage("py")).toBe(true);
    expect(isSupportedHighlightLanguage("markdown")).toBe(true);
  });

  it("大小写与空白不敏感", () => {
    expect(isSupportedHighlightLanguage("  Python ")).toBe(true);
  });

  it("集合外语言与空值不支持（回退 plaintext）", () => {
    expect(isSupportedHighlightLanguage("brainfuck")).toBe(false);
    expect(isSupportedHighlightLanguage(undefined)).toBe(false);
    expect(isSupportedHighlightLanguage(null)).toBe(false);
  });

  it("原型链成员名不当作语言命中（自身属性判定）", () => {
    expect(isSupportedHighlightLanguage("constructor")).toBe(false);
    expect(isSupportedHighlightLanguage("__proto__")).toBe(false);
    expect(isSupportedHighlightLanguage("hasOwnProperty")).toBe(false);
  });

  it("SUPPORTED_HIGHLIGHT_LANGUAGES 覆盖核心语言且无别名重复", () => {
    for (const lang of ["bash", "css", "html", "js", "jsx", "json", "md", "python", "rust", "toml", "ts", "tsx", "yaml"]) {
      expect(SUPPORTED_HIGHLIGHT_LANGUAGES).toContain(lang);
    }
    expect(SUPPORTED_HIGHLIGHT_LANGUAGES).not.toContain("shell");
  });
});

describe("tokenizeCodeDualTheme（streamdown 高亮插件运行时冒烟）", () => {
  it("按需加载语言并产出双主题 TokensResult", async () => {
    const result = await tokenizeCodeDualTheme("const a: number = 1;", "ts");
    expect(result.tokens.length).toBeGreaterThan(0);
    const flat = result.tokens.flat();
    expect(flat.some((t) => t.content === "const")).toBe(true);
    // 双主题：token 携带 light 颜色，且经 htmlStyle/变体携带 dark 配色
    const colored = flat.find((t) => t.color || t.htmlStyle);
    expect(colored).toBeTruthy();
  });

  it("未支持语言回退 plaintext，不抛错", async () => {
    const result = await tokenizeCodeDualTheme("some plain text", "brainfuck");
    expect(result.tokens.length).toBeGreaterThan(0);
    expect(result.tokens.flat().map((t) => t.content).join("")).toContain("some plain text");
  });
});
