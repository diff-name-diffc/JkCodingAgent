import type { ThemeRegistration, TokensResult } from "shiki";
import { isDarkActive } from "../lib/theme";
import { shikiCacheKey, shikiHighlightCache } from "./shiki-cache";

interface ShikiHighlighter {
  codeToHtml: (code: string, options: { lang: string; theme: string }) => string;
  codeToTokens: (
    code: string,
    options: { lang: string; themes: { light: string; dark: string } },
  ) => TokensResult;
  loadLanguage: (language: unknown) => Promise<void>;
  getLoadedLanguages: () => string[];
}

const LANGUAGE_ALIASES: Record<string, string> = {
  shell: "bash",
  sh: "bash",
  zsh: "bash",
  console: "bash",
  env: "bash",
  ts: "ts",
  tsx: "tsx",
  js: "js",
  jsx: "jsx",
  md: "md",
  markdown: "md",
  yml: "yaml",
  py: "python",
  rs: "rust",
  text: "plaintext",
  plain: "plaintext",
  txt: "plaintext",
};

const LANGUAGE_LOADERS: Record<string, () => Promise<unknown>> = {
  bash: () => import("shiki/dist/langs/bash.mjs"),
  css: () => import("shiki/dist/langs/css.mjs"),
  html: () => import("shiki/dist/langs/html.mjs"),
  js: () => import("shiki/dist/langs/js.mjs"),
  jsx: () => import("shiki/dist/langs/jsx.mjs"),
  json: () => import("shiki/dist/langs/json.mjs"),
  md: () => import("shiki/dist/langs/md.mjs"),
  python: () => import("shiki/dist/langs/python.mjs"),
  rust: () => import("shiki/dist/langs/rust.mjs"),
  toml: () => import("shiki/dist/langs/toml.mjs"),
  ts: () => import("shiki/dist/langs/ts.mjs"),
  tsx: () => import("shiki/dist/langs/tsx.mjs"),
  yaml: () => import("shiki/dist/langs/yaml.mjs"),
};

let highlighterPromise: Promise<ShikiHighlighter> | null = null;
const attemptedLanguages = new Set<string>(["plaintext"]);

/**
 * 中性代码阅读主题：灰阶底色与正文，语法高亮仅用克制的蓝、紫、橙区分。
 * 注意：`bg` 与 App.css `:root` 的 `--markdown-code-bg` (#f7f7f7) 是分别
 * 维护的两份取值，调整亮色面板色板时需同步两处。
 * 显式 ThemeRegistration 标注：tokenColors 字段名/结构错误可在编译期发现。
 */
export const NEUTRAL_LIGHT_THEME: ThemeRegistration = {
  name: "neutral-light",
  type: "light" as const,
  fg: "#262626",
  bg: "#f7f7f7",
  colors: {
    "editor.foreground": "#262626",
    "editor.background": "#f7f7f7",
  },
  tokenColors: [
    { scope: ["comment", "punctuation.definition.comment"], settings: { foreground: "#6b6b6b" } },
    { scope: ["string", "punctuation.definition.string"], settings: { foreground: "#315f8c" } },
    { scope: ["constant", "entity.name.constant"], settings: { foreground: "#9a551f" } },
    { scope: ["keyword", "storage.type", "storage.modifier"], settings: { foreground: "#7353a6" } },
    { scope: ["keyword.control"], settings: { foreground: "#7353a6" } },
    {
      scope: ["entity", "entity.name.function", "support.function"],
      settings: { foreground: "#365f9b" },
    },
    {
      scope: ["entity.name.type", "entity.name.class", "support.type", "support.class"],
      settings: { foreground: "#7b5a32" },
    },
    { scope: ["entity.name.tag"], settings: { foreground: "#365f9b" } },
    { scope: ["entity.other.attribute-name"], settings: { foreground: "#7353a6" } },
    { scope: ["variable", "variable.parameter"], settings: { foreground: "#262626" } },
    { scope: ["variable.language"], settings: { foreground: "#7353a6" } },
    { scope: ["support"], settings: { foreground: "#365f9b" } },
    { scope: ["meta.property-name", "meta.property-value"], settings: { foreground: "#315f8c" } },
    { scope: ["punctuation"], settings: { foreground: "#666666" } },
    { scope: ["markup.heading"], settings: { foreground: "#262626", fontStyle: "bold" } },
    { scope: ["markup.bold"], settings: { fontStyle: "bold" } },
    { scope: ["markup.italic"], settings: { fontStyle: "italic" } },
    { scope: ["markup.inserted"], settings: { foreground: "#1B7A4B" } },
    { scope: ["markup.deleted"], settings: { foreground: "#DC2626" } },
    { scope: ["markup.changed"], settings: { foreground: "#B45309" } },
  ],
};

/**
 * 亮色主题的暗色对偶，语法色提高亮度以适配深灰底色。
 * `bg` (#1b1b1b) 需与 App.css `.dark` 的 `--markdown-code-bg` 同步。
 */
export const NEUTRAL_DARK_THEME: ThemeRegistration = {
  name: "neutral-dark",
  type: "dark" as const,
  fg: "#e5e5e5",
  bg: "#1b1b1b",
  colors: {
    "editor.foreground": "#e5e5e5",
    "editor.background": "#1b1b1b",
  },
  tokenColors: [
    { scope: ["comment", "punctuation.definition.comment"], settings: { foreground: "#a3a3a3" } },
    { scope: ["string", "punctuation.definition.string"], settings: { foreground: "#a1bfdd" } },
    { scope: ["constant", "entity.name.constant"], settings: { foreground: "#d8a373" } },
    { scope: ["keyword", "storage.type", "storage.modifier"], settings: { foreground: "#b6a0d8" } },
    { scope: ["keyword.control"], settings: { foreground: "#b6a0d8" } },
    {
      scope: ["entity", "entity.name.function", "support.function"],
      settings: { foreground: "#91b4df" },
    },
    {
      scope: ["entity.name.type", "entity.name.class", "support.type", "support.class"],
      settings: { foreground: "#d4b58d" },
    },
    { scope: ["entity.name.tag"], settings: { foreground: "#91b4df" } },
    { scope: ["entity.other.attribute-name"], settings: { foreground: "#b6a0d8" } },
    { scope: ["variable", "variable.parameter"], settings: { foreground: "#e5e5e5" } },
    { scope: ["variable.language"], settings: { foreground: "#b6a0d8" } },
    { scope: ["support"], settings: { foreground: "#91b4df" } },
    { scope: ["meta.property-name", "meta.property-value"], settings: { foreground: "#a1bfdd" } },
    { scope: ["punctuation"], settings: { foreground: "#a3a3a3" } },
    { scope: ["markup.heading"], settings: { foreground: "#e5e5e5", fontStyle: "bold" } },
    { scope: ["markup.bold"], settings: { fontStyle: "bold" } },
    { scope: ["markup.italic"], settings: { fontStyle: "italic" } },
    { scope: ["markup.inserted"], settings: { foreground: "#7ee0a8" } },
    { scope: ["markup.deleted"], settings: { foreground: "#f87171" } },
    { scope: ["markup.changed"], settings: { foreground: "#f5a97f" } },
  ],
};

async function getHighlighter() {
  if (!highlighterPromise) {
    highlighterPromise = Promise.all([
      import("shiki/core"),
      import("shiki/dist/engine-javascript.mjs"),
    ]).then(async ([{ createHighlighterCore }, { createJavaScriptRegexEngine }]) => {
      const highlighter = (await createHighlighterCore({
        engine: createJavaScriptRegexEngine(),
        themes: [NEUTRAL_LIGHT_THEME, NEUTRAL_DARK_THEME],
      })) as unknown as ShikiHighlighter;
      return highlighter;
    });
  }

  return highlighterPromise;
}

/** 按需加载支持的语言集合（不含别名），供 streamdown 高亮插件声明能力面。 */
export const SUPPORTED_HIGHLIGHT_LANGUAGES: readonly string[] = Object.keys(LANGUAGE_LOADERS);

function normalizeLanguage(language?: string | null) {
  if (!language) {
    return "plaintext";
  }

  const normalized = language.trim().toLowerCase();
  // 自身属性判定：原型链成员（"constructor"/"__proto__" 等天然小写的继承名）
  // 不是语言别名，直接索引会取到继承成员并当作返回值泄漏出去。
  return Object.prototype.hasOwnProperty.call(LANGUAGE_ALIASES, normalized)
    ? LANGUAGE_ALIASES[normalized]
    : normalized;
}

/** 语言是否在本应用的按需加载集合内（供 streamdown 插件 supportsLanguage）。 */
export function isSupportedHighlightLanguage(language?: string | null): boolean {
  return Object.prototype.hasOwnProperty.call(LANGUAGE_LOADERS, normalizeLanguage(language));
}

async function ensureLanguage(language?: string | null) {
  const highlighter = await getHighlighter();
  const normalized = normalizeLanguage(language);
  const loadLanguage = LANGUAGE_LOADERS[normalized];

  if (
    loadLanguage &&
    !attemptedLanguages.has(normalized) &&
    !highlighter.getLoadedLanguages().includes(normalized)
  ) {
    attemptedLanguages.add(normalized);

    try {
      const module = await loadLanguage();
      await highlighter.loadLanguage((module as { default?: unknown }).default ?? module);
    } catch {
      return "plaintext";
    }
  }

  return highlighter.getLoadedLanguages().includes(normalized) ? normalized : "plaintext";
}

export async function highlightCodeToHtml(
  code: string,
  language?: string | null,
  dark = isDarkActive(),
) {
  const highlighter = await getHighlighter();
  const resolvedLanguage = await ensureLanguage(language);

  // UI-24b-2：LRU 缓存高亮产物——窗口化行重挂载/主题不变的重渲染直接命中，
  // 不重跑 Shiki（大 JSON 重复高亮是纯浪费，且重挂载会闪纯文本）。
  const cacheKey = shikiCacheKey(code, resolvedLanguage, dark);
  const cached = shikiHighlightCache.get(cacheKey);
  if (cached !== undefined) return cached;

  const html = highlighter.codeToHtml(code, {
    lang: resolvedLanguage,
    theme: dark ? "neutral-dark" : "neutral-light",
  });
  shikiHighlightCache.set(cacheKey, html, code.length);
  return html;
}

/**
 * 双主题 token 化（供 streamdown 高亮插件）：一次产出 light/dark 两套配色
 * 的 TokensResult，由 streamdown 经 `--shiki-dark` CSS 变量随 `html.dark`
 * 纯 CSS 切换。语言走与 `highlightCodeToHtml` 相同的按需加载与回退逻辑；
 * 主题固定为预载的双中性主题，不参与缓存键。
 */
export async function tokenizeCodeDualTheme(
  code: string,
  language?: string | null,
): Promise<TokensResult> {
  const highlighter = await getHighlighter();
  const resolvedLanguage = await ensureLanguage(language);
  return highlighter.codeToTokens(code, {
    lang: resolvedLanguage,
    themes: { light: "neutral-light", dark: "neutral-dark" },
  });
}
