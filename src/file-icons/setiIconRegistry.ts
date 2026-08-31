import type { FilePresentation } from "./types";

const setiIconModules = import.meta.glob("../assets/seti-icons/*.svg", {
  eager: true,
  query: "?raw",
  import: "default",
}) as Record<string, string>;

const SETI_THEME_COLORS = {
  white: "#d4d7d6",
  grey: "#4d5a5e",
  "grey-light": "#6d8086",
  blue: "#519aba",
  green: "#8dc149",
  orange: "#e37933",
  pink: "#f55385",
  purple: "#a074c4",
  red: "#cc3e44",
  yellow: "#cbcb41",
} as const;

const SETI_ICON_SVGS = Object.fromEntries(
  Object.entries(setiIconModules).map(([modulePath, svg]) => {
    const filename = modulePath.split("/").pop() ?? modulePath;
    return [filename.replace(/\.svg$/i, ""), svg];
  }),
) as Record<string, string>;

const SETI_ICON_COLORS: Record<string, string> = {
  audio: SETI_THEME_COLORS.purple,
  c: SETI_THEME_COLORS.blue,
  "checkbox-unchecked": SETI_THEME_COLORS.orange,
  config: SETI_THEME_COLORS["grey-light"],
  db: SETI_THEME_COLORS.pink,
  default: SETI_THEME_COLORS.white,
  editorconfig: SETI_THEME_COLORS["grey-light"],
  eslint: SETI_THEME_COLORS.purple,
  font: SETI_THEME_COLORS.red,
  lock: SETI_THEME_COLORS.green,
  lua: SETI_THEME_COLORS.blue,
  makefile: SETI_THEME_COLORS.orange,
  pdf: SETI_THEME_COLORS.red,
  rollup: SETI_THEME_COLORS.red,
  rust: SETI_THEME_COLORS["grey-light"],
  settings: SETI_THEME_COLORS["grey-light"],
  svelte: SETI_THEME_COLORS.red,
  svg: SETI_THEME_COLORS.purple,
  video: SETI_THEME_COLORS.pink,
  vite: SETI_THEME_COLORS.yellow,
  vue: SETI_THEME_COLORS.green,
  wasm: SETI_THEME_COLORS.blue,
  xls: SETI_THEME_COLORS.green,
  yarn: SETI_THEME_COLORS.blue,
  zip: SETI_THEME_COLORS["grey-light"],
};

const EXACT_FILE_ICON_NAMES: Record<string, string> = {
  ".editorconfig": "editorconfig",
  ".gitignore": "git_ignore",
  ".gitattributes": "git",
  ".gitmodules": "git",
  ".npmrc": "npm",
  "docker-compose.yml": "docker",
  "docker-compose.yaml": "docker",
  "eslint.config.js": "eslint",
  "eslint.config.mjs": "eslint",
  "eslint.config.ts": "eslint",
  "gulpfile.js": "gulp",
  "gulpfile.mjs": "gulp",
  license: "license",
  notice: "license",
  makefile: "makefile",
  "package-lock.json": "npm",
  "package.json": "npm",
  "pnpm-lock.yaml": "npm",
  "pnpm-workspace.yaml": "npm",
  "rollup.config.js": "rollup",
  "rollup.config.mjs": "rollup",
  "rollup.config.ts": "rollup",
  "tsconfig.base.json": "tsconfig",
  "tsconfig.json": "tsconfig",
  "vite.config.cts": "vite",
  "vite.config.js": "vite",
  "vite.config.mjs": "vite",
  "vite.config.mts": "vite",
  "vite.config.ts": "vite",
  "webpack.config.js": "webpack",
  "webpack.config.mjs": "webpack",
  "webpack.config.ts": "webpack",
  "yarn.lock": "yarn",
};

const PREFIX_FILE_ICON_NAMES: Array<[string, string]> = [
  [".env.", "settings"],
  ["dockerfile.", "docker"],
  ["tsconfig.", "tsconfig"],
];

const FALLBACK_FILE_ICON_NAMES: Record<string, string> = {
  archive: "zip",
  astro: "default",
  audio: "audio",
  binary: "default",
  c: "c",
  certificate: "lock",
  config: "config",
  cpp: "cpp",
  csharp: "c-sharp",
  css: "css",
  database: "db",
  default: "default",
  docker: "docker",
  env: "settings",
  font: "font",
  git: "git",
  go: "go",
  graphql: "graphql",
  html: "html",
  image: "image",
  ini: "config",
  java: "java",
  javascript: "javascript",
  json: "json",
  jsx: "react",
  key: "lock",
  kotlin: "kotlin",
  lock: "lock",
  log: "default",
  lua: "lua",
  make: "makefile",
  markdown: "markdown",
  notebook: "notebook",
  package: "npm",
  pdf: "pdf",
  php: "php",
  protobuf: "default",
  python: "python",
  r: "R",
  react: "react",
  ruby: "ruby",
  rust: "rust",
  sass: "sass",
  shell: "shell",
  spreadsheet: "xls",
  sql: "db",
  storybook: "default",
  svelte: "svelte",
  svg: "svg",
  swift: "default",
  test: "checkbox-unchecked",
  text: "default",
  toml: "settings",
  tsx: "react",
  typescript: "typescript",
  video: "video",
  vue: "vue",
  wasm: "wasm",
  xml: "xml",
  yaml: "yml",
};

const FALLBACK_FOLDER_ICON_NAMES: Record<string, string> = {
  "folder-git": "git_folder",
  "folder-github": "github",
};

function getSetiSvgByName(iconName: string): string {
  return SETI_ICON_SVGS[iconName] ?? SETI_ICON_SVGS.default;
}

function getSetiIconColor(iconName: string, presentation: FilePresentation): string {
  return SETI_ICON_COLORS[iconName] ?? presentation.accentColor;
}

function withSetiPresentationAttributes(svg: string, fillColor: string): string {
  return svg.replace(/<svg\b([^>]*)>/i, (_match, attrs: string) => {
    const cleanedAttrs = attrs
      .replace(/\s(?:width|height|fill|color|class|aria-hidden|focusable)=(".*?"|'.*?')/gi, "")
      .trim();

    const normalizedAttrs = cleanedAttrs ? ` ${cleanedAttrs}` : "";
    return `<svg${normalizedAttrs} width="100%" height="100%" fill="${fillColor}" color="${fillColor}" aria-hidden="true" focusable="false">`;
  });
}

/* 图标名 × 颜色的组合通常有限，缓存避免每次渲染重复 SVG 正则替换。
   防御性上限：fillColor 可能来自 presentation.accentColor（取值范围不受
   SETI_ICON_COLORS 约束），若来源扩展导致组合膨胀，超限整体清空重建。 */
const SVG_CACHE_MAX_ENTRIES = 512;
const svgMarkupCache = new Map<string, string>();

function resolveSetiIconName(presentation: FilePresentation): string {
  if (presentation.isDir) {
    return FALLBACK_FOLDER_ICON_NAMES[presentation.iconKey] ?? "folder";
  }

  if (EXACT_FILE_ICON_NAMES[presentation.normalizedName]) {
    return EXACT_FILE_ICON_NAMES[presentation.normalizedName];
  }

  for (const [prefix, iconName] of PREFIX_FILE_ICON_NAMES) {
    if (presentation.normalizedName.startsWith(prefix)) {
      return iconName;
    }
  }

  return FALLBACK_FILE_ICON_NAMES[presentation.iconKey] ?? "default";
}

export function getSetiIconSvgMarkup(presentation: FilePresentation): string {
  const iconName = resolveSetiIconName(presentation);
  const fillColor = getSetiIconColor(iconName, presentation);
  const cacheKey = `${iconName}:${fillColor}`;

  const cached = svgMarkupCache.get(cacheKey);
  if (cached) {
    return cached;
  }

  const markup = withSetiPresentationAttributes(getSetiSvgByName(iconName), fillColor);
  if (svgMarkupCache.size >= SVG_CACHE_MAX_ENTRIES) {
    svgMarkupCache.clear();
  }
  svgMarkupCache.set(cacheKey, markup);
  return markup;
}
