import { getFileIconSpec } from "./iconRegistry";
import type { FilePresentation, ResolveFilePresentationInput } from "./types";

const PREVIEWABLE_IMAGE_EXTENSIONS = new Set(["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg"]);
const BINARY_LIKE_ICON_KEYS = new Set([
  "image",
  "svg",
  "video",
  "audio",
  "archive",
  "binary",
  "wasm",
  "font",
  "pdf",
  "database",
]);

const EXACT_FILE_ICON_RULES: Record<string, string> = {
  dockerfile: "docker",
  "docker-compose.yml": "docker",
  "docker-compose.yaml": "docker",
  makefile: "make",
  gnumakefile: "make",
  justfile: "make",
  procfile: "config",
  gemfile: "ruby",
  rakefile: "ruby",
  "cargo.toml": "rust",
  "cargo.lock": "lock",
  "go.mod": "go",
  "go.sum": "lock",
  "package.json": "package",
  "package-lock.json": "lock",
  "yarn.lock": "lock",
  "pnpm-lock.yaml": "lock",
  "pnpm-workspace.yaml": "package",
  "bun.lock": "lock",
  "bun.lockb": "lock",
  "tsconfig.json": "config",
  "tsconfig.base.json": "config",
  "jsconfig.json": "config",
  "vite.config.ts": "config",
  "vite.config.js": "config",
  "vite.config.mts": "config",
  "vite.config.mjs": "config",
  "vitest.config.ts": "config",
  "vitest.config.js": "config",
  "eslint.config.js": "config",
  "eslint.config.mjs": "config",
  "eslint.config.ts": "config",
  "prettier.config.js": "config",
  "prettier.config.mjs": "config",
  "tailwind.config.js": "config",
  "tailwind.config.ts": "config",
  "postcss.config.js": "config",
  "postcss.config.cjs": "config",
  "next.config.js": "config",
  "next.config.mjs": "config",
  "nuxt.config.ts": "config",
  "svelte.config.js": "config",
  "astro.config.mjs": "config",
  "readme.md": "markdown",
  "readme.mdx": "markdown",
  "changelog.md": "markdown",
  license: "text",
  notice: "text",
  "requirements.txt": "python",
  "pyproject.toml": "python",
  "poetry.lock": "lock",
  pipfile: "python",
  "pipfile.lock": "lock",
  ".env": "env",
  ".env.local": "env",
  ".env.example": "env",
  ".gitignore": "git",
  ".gitattributes": "git",
  ".gitmodules": "git",
  ".dockerignore": "docker",
  ".editorconfig": "config",
  ".npmrc": "config",
  ".nvmrc": "config",
  ".prettierrc": "config",
  ".prettierignore": "config",
  ".eslintrc": "config",
  ".eslintignore": "config",
  ".stylelintrc": "config",
  ".tool-versions": "config",
  ".bashrc": "shell",
  ".bash_profile": "shell",
  ".zshrc": "shell",
  ".zprofile": "shell",
  ".profile": "shell",
};

const SUFFIX_ICON_RULES: Array<[string, string]> = [
  [".stories.tsx", "storybook"],
  [".stories.jsx", "storybook"],
  [".stories.ts", "storybook"],
  [".stories.js", "storybook"],
  [".story.tsx", "storybook"],
  [".story.jsx", "storybook"],
  [".story.ts", "storybook"],
  [".story.js", "storybook"],
  [".test.tsx", "test"],
  [".test.jsx", "test"],
  [".test.ts", "test"],
  [".test.js", "test"],
  [".spec.tsx", "test"],
  [".spec.jsx", "test"],
  [".spec.ts", "test"],
  [".spec.js", "test"],
  [".module.scss", "sass"],
  [".module.sass", "sass"],
  [".module.css", "css"],
  [".tar.gz", "archive"],
  [".tar.bz2", "archive"],
  [".tar.xz", "archive"],
  [".d.ts", "typescript"],
];

const PREFIX_ICON_RULES: Array<[string, string]> = [
  ["dockerfile.", "docker"],
  [".env.", "env"],
  [".git", "git"],
];

const EXTENSION_ICON_RULES: Record<string, string> = {
  ts: "typescript",
  mts: "typescript",
  cts: "typescript",
  tsx: "tsx",
  js: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  jsx: "jsx",
  json: "json",
  jsonc: "json",
  json5: "json",
  md: "markdown",
  mdx: "markdown",
  markdown: "markdown",
  txt: "text",
  rst: "text",
  adoc: "text",
  log: "log",
  py: "python",
  pyw: "python",
  rs: "rust",
  go: "go",
  java: "java",
  kt: "kotlin",
  kts: "kotlin",
  swift: "swift",
  rb: "ruby",
  php: "php",
  cs: "csharp",
  c: "c",
  h: "c",
  cpp: "cpp",
  cxx: "cpp",
  cc: "cpp",
  hpp: "cpp",
  hxx: "cpp",
  lua: "lua",
  r: "r",
  sql: "sql",
  gql: "graphql",
  graphql: "graphql",
  html: "html",
  htm: "html",
  css: "css",
  scss: "sass",
  sass: "sass",
  less: "sass",
  vue: "vue",
  svelte: "svelte",
  astro: "astro",
  xml: "xml",
  xsd: "xml",
  xsl: "xml",
  svg: "svg",
  png: "image",
  jpg: "image",
  jpeg: "image",
  gif: "image",
  webp: "image",
  bmp: "image",
  avif: "image",
  ico: "image",
  mp4: "video",
  webm: "video",
  mov: "video",
  mkv: "video",
  mp3: "audio",
  wav: "audio",
  flac: "audio",
  ogg: "audio",
  m4a: "audio",
  zip: "archive",
  gz: "archive",
  bz2: "archive",
  xz: "archive",
  rar: "archive",
  "7z": "archive",
  tar: "archive",
  tgz: "archive",
  jar: "archive",
  war: "archive",
  pdf: "pdf",
  csv: "spreadsheet",
  tsv: "spreadsheet",
  xls: "spreadsheet",
  xlsx: "spreadsheet",
  exe: "binary",
  dll: "binary",
  so: "binary",
  dylib: "binary",
  bin: "binary",
  dat: "binary",
  class: "binary",
  o: "binary",
  a: "binary",
  obj: "binary",
  db: "database",
  sqlite: "database",
  sqlite3: "database",
  db3: "database",
  sqlitedb: "database",
  wasm: "wasm",
  proto: "protobuf",
  toml: "toml",
  yaml: "yaml",
  yml: "yaml",
  ini: "ini",
  conf: "config",
  cfg: "config",
  properties: "config",
  lock: "lock",
  pem: "certificate",
  crt: "certificate",
  cert: "certificate",
  cer: "certificate",
  p12: "certificate",
  key: "key",
  pub: "key",
  asc: "key",
  sh: "shell",
  bash: "shell",
  zsh: "shell",
  fish: "shell",
  command: "shell",
  dockerfile: "docker",
  font: "font",
  ttf: "font",
  otf: "font",
  woff: "font",
  woff2: "font",
  eot: "font",
  ipynb: "notebook",
};

const EXACT_FOLDER_ICON_RULES: Record<string, string> = {
  src: "folder-src",
  components: "folder-components",
  component: "folder-components",
  pages: "folder-pages",
  page: "folder-pages",
  routes: "folder-pages",
  route: "folder-pages",
  app: "folder-pages",
  hooks: "folder-hooks",
  hook: "folder-hooks",
  assets: "folder-assets",
  asset: "folder-assets",
  images: "folder-assets",
  image: "folder-assets",
  icons: "folder-assets",
  public: "folder-public",
  static: "folder-public",
  styles: "folder-styles",
  style: "folder-styles",
  css: "folder-styles",
  scss: "folder-styles",
  scripts: "folder-scripts",
  script: "folder-scripts",
  bin: "folder-scripts",
  docs: "folder-docs",
  doc: "folder-docs",
  test: "folder-tests",
  tests: "folder-tests",
  "__tests__": "folder-tests",
  spec: "folder-tests",
  ".github": "folder-github",
  ".git": "folder-git",
  ".config": "folder-config",
  config: "folder-config",
  configs: "folder-config",
  node_modules: "folder-node",
  dist: "folder-dist",
  build: "folder-build",
  out: "folder-dist",
  coverage: "folder-build",
};

const EXACT_LANGUAGE_RULES: Record<string, string> = {
  dockerfile: "dockerfile",
  makefile: "makefile",
  ".gitignore": "shell",
  ".dockerignore": "shell",
  ".env": "shell",
  ".env.local": "shell",
  ".env.example": "shell",
  ".npmrc": "ini",
  "readme.md": "markdown",
  "readme.mdx": "markdown",
  "changelog.md": "markdown",
};

const EXTENSION_LANGUAGE_RULES: Record<string, string> = {
  ts: "typescript",
  mts: "typescript",
  cts: "typescript",
  tsx: "typescript",
  js: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  jsx: "javascript",
  json: "json",
  jsonc: "json",
  json5: "json",
  md: "markdown",
  mdx: "markdown",
  markdown: "markdown",
  py: "python",
  rs: "rust",
  go: "go",
  java: "java",
  c: "cpp",
  h: "cpp",
  cpp: "cpp",
  cxx: "cpp",
  cc: "cpp",
  hpp: "cpp",
  hxx: "cpp",
  css: "css",
  scss: "scss",
  sass: "scss",
  less: "css",
  html: "html",
  htm: "html",
  xml: "xml",
  yml: "yaml",
  yaml: "yaml",
  toml: "ini",
  ini: "ini",
  conf: "ini",
  cfg: "ini",
  sh: "shell",
  bash: "shell",
  zsh: "shell",
  fish: "shell",
  sql: "sql",
  lua: "lua",
  swift: "swift",
  kt: "kotlin",
  kts: "kotlin",
  rb: "ruby",
  php: "php",
  r: "r",
  proto: "protobuf",
  graphql: "graphql",
  gql: "graphql",
};

function getDisplayName(input: ResolveFilePresentationInput): string {
  const raw = input.name ?? input.path ?? "";
  const normalized = raw.replace(/\\/g, "/");
  return normalized.split("/").pop() ?? normalized;
}

function getNormalizedName(name: string): string {
  return name.trim().toLowerCase();
}

function getExtension(name: string, explicitExtension?: string): string {
  if (explicitExtension) {
    return explicitExtension.toLowerCase();
  }

  const lastDot = name.lastIndexOf(".");
  if (lastDot <= 0 || lastDot === name.length - 1) {
    return "";
  }

  return name.slice(lastDot + 1).toLowerCase();
}

function resolveFolderIconKey(name: string): string {
  return EXACT_FOLDER_ICON_RULES[name] ?? "folder";
}

function resolveFileIconKey(name: string, extension: string): string {
  if (EXACT_FILE_ICON_RULES[name]) {
    return EXACT_FILE_ICON_RULES[name];
  }

  for (const [prefix, iconKey] of PREFIX_ICON_RULES) {
    if (name.startsWith(prefix)) {
      return iconKey;
    }
  }

  for (const [suffix, iconKey] of SUFFIX_ICON_RULES) {
    if (name.endsWith(suffix)) {
      return iconKey;
    }
  }

  if (extension && EXTENSION_ICON_RULES[extension]) {
    return EXTENSION_ICON_RULES[extension];
  }

  return "default";
}

function resolveMonacoLanguage(name: string, extension: string, isDir: boolean): string {
  if (isDir) {
    return "plaintext";
  }

  if (EXACT_LANGUAGE_RULES[name]) {
    return EXACT_LANGUAGE_RULES[name];
  }

  if (name.startsWith("dockerfile.")) {
    return "dockerfile";
  }

  if (name.startsWith(".env.")) {
    return "shell";
  }

  return EXTENSION_LANGUAGE_RULES[extension] ?? "plaintext";
}

export function resolveFilePresentation(input: ResolveFilePresentationInput): FilePresentation {
  const name = getDisplayName(input);
  const normalizedName = getNormalizedName(name);
  const isDir = Boolean(input.isDir);
  const extension = isDir ? "" : getExtension(normalizedName, input.extension);
  const iconKey = isDir
    ? resolveFolderIconKey(normalizedName)
    : resolveFileIconKey(normalizedName, extension);
  const icon = getFileIconSpec(iconKey);
  const monacoLanguage = resolveMonacoLanguage(normalizedName, extension, isDir);
  const isMarkdown = !isDir && (extension === "md" || extension === "mdx" || extension === "markdown");
  const isPreviewableImage = !isDir && PREVIEWABLE_IMAGE_EXTENSIONS.has(extension);
  const isBinaryLike = !isDir && BINARY_LIKE_ICON_KEYS.has(iconKey);

  return {
    name,
    path: input.path ?? null,
    normalizedName,
    extension,
    isDir,
    iconKey,
    category: icon.category,
    accentColor: icon.accent,
    monacoLanguage,
    isMarkdown,
    isPreviewableImage,
    isBinaryLike,
  };
}
