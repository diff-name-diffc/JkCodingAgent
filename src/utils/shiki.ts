interface ShikiHighlighter {
  codeToHtml: (code: string, options: { lang: string; theme: string }) => string;
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
 */
const TEAL_LIGHT_THEME = {
  name: "teal-light",
  type: "light",
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

async function getHighlighter() {
  if (!highlighterPromise) {
    highlighterPromise = Promise.all([
      import("shiki/core"),
      import("shiki/dist/engine-javascript.mjs"),
    ]).then(
      async ([{ createHighlighterCore }, { createJavaScriptRegexEngine }]) => {
        const highlighter = (await createHighlighterCore({
          engine: createJavaScriptRegexEngine(),
          themes: [TEAL_LIGHT_THEME] as never,
        })) as unknown as ShikiHighlighter;
        return highlighter;
      },
    );
  }

  return highlighterPromise;
}

function normalizeLanguage(language?: string | null) {
  if (!language) {
    return "plaintext";
  }

  const normalized = language.trim().toLowerCase();
  return LANGUAGE_ALIASES[normalized] ?? normalized;
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

export async function highlightCodeToHtml(code: string, language?: string | null) {
  const highlighter = await getHighlighter();
  const resolvedLanguage = await ensureLanguage(language);

  return highlighter.codeToHtml(code, {
    lang: resolvedLanguage,
    theme: "teal-light",
  });
}
