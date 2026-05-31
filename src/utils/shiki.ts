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

async function getHighlighter() {
  if (!highlighterPromise) {
    highlighterPromise = Promise.all([
      import("shiki/core"),
      import("shiki/dist/engine-javascript.mjs"),
      import("shiki/dist/themes/github-dark.mjs"),
      import("shiki/dist/themes/github-light.mjs"),
    ]).then(
      async ([{ createHighlighterCore }, { createJavaScriptRegexEngine }, darkTheme, lightTheme]) => {
        const highlighter = (await createHighlighterCore({
          engine: createJavaScriptRegexEngine(),
          themes: [darkTheme.default, lightTheme.default],
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

export async function highlightCodeToHtml(code: string, language?: string | null, isDark = false) {
  const highlighter = await getHighlighter();
  const resolvedLanguage = await ensureLanguage(language);

  return highlighter.codeToHtml(code, {
    lang: resolvedLanguage,
    theme: isDark ? "github-dark" : "github-light",
  });
}
