interface ShikiHighlighter {
  codeToHtml: (code: string, options: { lang: string; theme: string }) => string;
  loadLanguage: (language: string) => Promise<void>;
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

let highlighterPromise: Promise<ShikiHighlighter> | null = null;
const attemptedLanguages = new Set<string>(["plaintext"]);

async function getHighlighter() {
  if (!highlighterPromise) {
    highlighterPromise = import("shiki").then(async ({ createHighlighter }) => {
      const highlighter = (await createHighlighter({
        themes: ["github-dark", "github-light"],
        langs: ["plaintext"],
      })) as unknown as ShikiHighlighter;
      return highlighter;
    });
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

  if (!attemptedLanguages.has(normalized) && !highlighter.getLoadedLanguages().includes(normalized)) {
    attemptedLanguages.add(normalized);

    try {
      await highlighter.loadLanguage(normalized);
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
