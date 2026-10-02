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
 * 自定义青绿调亮色主题，配色与应用 `--accent` (#297c70) 体系一致，
 * 避免 github-light 的蓝紫高亮与整体令牌冲突。
 * 注意：`bg` 与 App.css `:root` 的 `--markdown-code-bg` (#f9fdfc) 是分别
 * 维护的两份取值，调整亮色面板色板时需同步两处。
 * 显式 ThemeRegistration 标注：tokenColors 字段名/结构错误可在编译期发现。
 */
export const TEAL_LIGHT_THEME: ThemeRegistration = {
  name: "teal-light",
  type: "light" as const,
  fg: "#17201D",
  bg: "#f9fdfc",
  colors: {
    "editor.foreground": "#17201D",
    "editor.background": "#f9fdfc",
  },
  tokenColors: [
    { scope: ["comment", "punctuation.definition.comment"], settings: { foreground: "#89928E" } },
    { scope: ["string", "punctuation.definition.string"], settings: { foreground: "#1B7A4B" } },
    { scope: ["constant", "entity.name.constant"], settings: { foreground: "#0F766E" } },
    { scope: ["keyword", "storage.type", "storage.modifier"], settings: { foreground: "#B45309" } },
    { scope: ["keyword.control"], settings: { foreground: "#9A3412" } },
    { scope: ["entity", "entity.name.function", "support.function"], settings: { foreground: "#1F665D" } },
    { scope: ["entity.name.type", "entity.name.class", "support.type", "support.class"], settings: { foreground: "#155E54" } },
    { scope: ["entity.name.tag"], settings: { foreground: "#0F766E" } },
    { scope: ["entity.other.attribute-name"], settings: { foreground: "#B45309" } },
    { scope: ["variable", "variable.parameter"], settings: { foreground: "#17201D" } },
    { scope: ["variable.language"], settings: { foreground: "#9A3412" } },
    { scope: ["support"], settings: { foreground: "#0F766E" } },
    { scope: ["meta.property-name", "meta.property-value"], settings: { foreground: "#0F766E" } },
    { scope: ["punctuation"], settings: { foreground: "#4A605C" } },
    { scope: ["markup.heading"], settings: { foreground: "#1F665D", fontStyle: "bold" } },
    { scope: ["markup.bold"], settings: { fontStyle: "bold" } },
    { scope: ["markup.italic"], settings: { fontStyle: "italic" } },
    { scope: ["markup.inserted"], settings: { foreground: "#1B7A4B" } },
    { scope: ["markup.deleted"], settings: { foreground: "#DC2626" } },
    { scope: ["markup.changed"], settings: { foreground: "#B45309" } },
  ],
};

/**
 * teal-light 的暗色对偶，与 `.dark` 面板色板一致。
 * 注意：`bg` (#101412) 对应 App.css `.dark` 的 `--markdown-code-bg`，
 * entity/tag 主色 (#55c7ad) 对应 `.dark` 的 `--accent`——两者在此为硬编码，
 * 调整暗色面板色板时需同步 App.css 对应 CSS 变量。
 */
export const TEAL_DARK_THEME: ThemeRegistration = {
  name: "teal-dark",
  type: "dark" as const,
  fg: "#e7ece9",
  bg: "#101412",
  colors: {
    "editor.foreground": "#e7ece9",
    "editor.background": "#101412",
  },
  tokenColors: [
    { scope: ["comment", "punctuation.definition.comment"], settings: { foreground: "#7d8a85" } },
    { scope: ["string", "punctuation.definition.string"], settings: { foreground: "#7ee0a8" } },
    { scope: ["constant", "entity.name.constant"], settings: { foreground: "#5eead4" } },
    { scope: ["keyword", "storage.type", "storage.modifier"], settings: { foreground: "#f5a97f" } },
    { scope: ["keyword.control"], settings: { foreground: "#fda883" } },
    { scope: ["entity", "entity.name.function", "support.function"], settings: { foreground: "#70d6be" } },
    { scope: ["entity.name.type", "entity.name.class", "support.type", "support.class"], settings: { foreground: "#55c7ad" } },
    { scope: ["entity.name.tag"], settings: { foreground: "#55c7ad" } },
    { scope: ["entity.other.attribute-name"], settings: { foreground: "#f5a97f" } },
    { scope: ["variable", "variable.parameter"], settings: { foreground: "#e7ece9" } },
    { scope: ["variable.language"], settings: { foreground: "#fda883" } },
    { scope: ["support"], settings: { foreground: "#55c7ad" } },
    { scope: ["meta.property-name", "meta.property-value"], settings: { foreground: "#55c7ad" } },
    { scope: ["punctuation"], settings: { foreground: "#9ba7a2" } },
    { scope: ["markup.heading"], settings: { foreground: "#70d6be", fontStyle: "bold" } },
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
    ]).then(
      async ([{ createHighlighterCore }, { createJavaScriptRegexEngine }]) => {
        const highlighter = (await createHighlighterCore({
          engine: createJavaScriptRegexEngine(),
          themes: [TEAL_LIGHT_THEME, TEAL_DARK_THEME],
        })) as unknown as ShikiHighlighter;
        return highlighter;
      },
    );
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
  return Object.prototype.hasOwnProperty.call(
    LANGUAGE_LOADERS,
    normalizeLanguage(language),
  );
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
    theme: dark ? "teal-dark" : "teal-light",
  });
  shikiHighlightCache.set(cacheKey, html, code.length);
  return html;
}

/**
 * 双主题 token 化（供 streamdown 高亮插件）：一次产出 light/dark 两套配色
 * 的 TokensResult，由 streamdown 经 `--shiki-dark` CSS 变量随 `html.dark`
 * 纯 CSS 切换。语言走与 `highlightCodeToHtml` 相同的按需加载与回退逻辑；
 * 主题固定为预载的双 teal 主题，不参与缓存键。
 */
export async function tokenizeCodeDualTheme(
  code: string,
  language?: string | null,
): Promise<TokensResult> {
  const highlighter = await getHighlighter();
  const resolvedLanguage = await ensureLanguage(language);
  return highlighter.codeToTokens(code, {
    lang: resolvedLanguage,
    themes: { light: "teal-light", dark: "teal-dark" },
  });
}
