import { readFile, readdir } from "node:fs/promises";
import path from "node:path";
import process from "node:process";

const ROOT = process.cwd();
const SOURCE_ROOT = path.join(ROOT, "src");
const STYLE_FILE = path.join(SOURCE_ROOT, "styles/tailwind.css");
const APP_CSS_FILE = path.join(SOURCE_ROOT, "App.css");
const SOURCE_EXTENSIONS = new Set([".ts", ".tsx", ".css"]);

// 动态生成 `.ai-*` class 时必须在这里完整枚举所有实际 class，防止静态扫描误删。
// 当前代码没有动态 `.ai-*` class；`is-*` 等状态 modifier 不属于本清单。
const DYNAMIC_AI_CLASS_VALUES = new Map([
  ["ai-workflow-node--", ["pending", "running", "succeeded", "failed", "skipped", "cancelled"]],
  ["ai-workflow-notice-row--", ["running", "succeeded", "failed"]],
  ["ai-workflow-tool-card--", ["running", "succeeded", "failed"]],
  ["ai-workflow-edge--", ["waiting", "ready", "active", "done", "failed"]],
  ["ai-status-pill--", ["success", "error", "running", "warn", "neutral", "pending"]],
  ["ai-tool-call-node--", ["running", "success", "error", "planned"]],
  ["ai-activity-timeline-dot--", ["running", "success", "error", "neutral"]],
]);
const AI_CLASS_SAFELIST = new Set(
  [...DYNAMIC_AI_CLASS_VALUES].flatMap(([prefix, values]) => values.map((value) => `${prefix}${value}`)),
);

// App.css 遗留类（非 ai-*）的动态拼接枚举，与上方 ai 清单同理。
const DYNAMIC_LEGACY_CLASS_VALUES = new Map([
  ["git-diff-split-side--", ["add", "del", "ctx", "empty"]],
  ["git-diff-word-hl--", ["add", "del"]],
  ["markdown-surface--", ["chat", "document"]],
  ["markdown-image-thumbnail-wrap--", ["full", "thumb"]],
]);
const LEGACY_CLASS_SAFELIST = new Set(
  [...DYNAMIC_LEGACY_CLASS_VALUES].flatMap(([prefix, values]) =>
    values.map((value) => `${prefix}${value}`),
  ),
);

// 纳入检查的 App.css 自建类（定义可落在 App.css 或 styles/tailwind.css 任一）：
// - 前缀族：整族纳入；
// - 精确名：单词类（避免误伤 monaco 主题名 "nezha-dark" 等字符串常量）。
// 第三方 DOM 类（cm- / shiki / katex 等只作样式目标、源码无引用的）不纳入。
const LEGACY_TRACKED_PREFIXES = [
  "radix-select-",
  "branch-popover-",
  "file-viewer-tab-",
  "file-explorer-context-menu-",
  "markdown-",
  "md-preview-",
  "python-inline-",
  "git-diff-",
];
const LEGACY_TRACKED_EXACT = new Set([
  "spin",
  "session-selectable",
  "chat-attachment-preview",
  "nezha-brand-surface",
  "nezha-chat-home",
]);

// 现存历史债务基线：新增无引用定义会让 CI 失败；清理后应同步下调，禁止反向增长。
const MAX_UNREFERENCED_AI_CLASSES = 0;
const MAX_UNREFERENCED_LEGACY_CLASSES = 0;

async function listSourceFiles(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const nested = await Promise.all(
    entries.map(async (entry) => {
      const entryPath = path.join(directory, entry.name);
      if (entry.isDirectory()) return listSourceFiles(entryPath);
      if (!SOURCE_EXTENSIONS.has(path.extname(entry.name))) return [];
      return path.resolve(entryPath) === path.resolve(STYLE_FILE) ? [] : [entryPath];
    }),
  );
  return nested.flat();
}

function collectMatches(source, pattern) {
  return new Set([...source.matchAll(pattern)].map((match) => match[1]));
}

// CSS 注释里提到的类名（如「去掉 .markdown-surface img」）既非定义也非引用，
// 定义收集前先剥离注释，否则注释里的退役类名会造成无引用误报/漏报。
const stripCssComments = (css) => css.replace(/\/\*[\s\S]*?\*\//g, "");
const styleSource = stripCssComments(await readFile(STYLE_FILE, "utf8"));
const appCssSource = stripCssComments(await readFile(APP_CSS_FILE, "utf8"));
const sourceFiles = await listSourceFiles(SOURCE_ROOT);
const sources = await Promise.all(sourceFiles.map((file) => readFile(file, "utf8")));
// ai-* 消费者口径保持历史行为（含 App.css 中 base 层选择器对 ai-* 类的引用）。
const consumerSource = sources.join("\n");
// 遗留类消费者口径：排除两个 CSS 定义文件自身（防止把定义当引用）。
const legacyConsumerSource = sourceFiles
  .map((file, index) => (path.resolve(file) === path.resolve(APP_CSS_FILE) ? "" : sources[index]))
  .join("\n");

// ── .ai-*（styles/tailwind.css）────────────────────────────────────
const defined = collectMatches(styleSource, /\.((?:ai)-[a-zA-Z0-9_-]+)/g);
const referenced = collectMatches(consumerSource, /\b((?:ai)-[a-zA-Z0-9_-]+)\b/g);
for (const className of AI_CLASS_SAFELIST) referenced.add(className);

const dynamicPrefixes = new Set(
  [...DYNAMIC_AI_CLASS_VALUES.keys()].map((prefix) => prefix.replace(/-+$/, "")),
);
const undefinedReferences = [...referenced]
  .filter(
    (className) =>
      !defined.has(className) &&
      !AI_CLASS_SAFELIST.has(className) &&
      !dynamicPrefixes.has(className),
  )
  .sort();
const unreferencedDefinitions = [...defined].filter((className) => !referenced.has(className)).sort();

// ── App.css 遗留类 ─────────────────────────────────────────────────
const legacyClassDefRe = /\.([a-zA-Z][\w-]*)/g;
const isTrackedLegacy = (name) =>
  LEGACY_TRACKED_EXACT.has(name) || LEGACY_TRACKED_PREFIXES.some((prefix) => name.startsWith(prefix));
const definedLegacy = new Set(
  [...styleSource.matchAll(legacyClassDefRe), ...appCssSource.matchAll(legacyClassDefRe)]
    .map((match) => match[1])
    .filter(isTrackedLegacy),
);
// 引用匹配用否定后顾排除路径段（"./markdown-renderer"）与前缀词内嵌（"animate-spin"）；
// 结尾 \b 与 ai 通道一致：模板拼接（`markdown-surface--${variant}`）会回退到基名匹配。
const legacyRefRe = new RegExp(
  `(?<![\\w-/])((?:${LEGACY_TRACKED_PREFIXES.join("|")})[\\w-]+|${[...LEGACY_TRACKED_EXACT].join("|")})\\b`,
  "g",
);
const referencedLegacy = collectMatches(legacyConsumerSource, legacyRefRe);
for (const className of LEGACY_CLASS_SAFELIST) referencedLegacy.add(className);

const legacyDynamicPrefixes = new Set(
  [...DYNAMIC_LEGACY_CLASS_VALUES.keys()].map((prefix) => prefix.replace(/-+$/, "")),
);
const legacyUndefinedReferences = [...referencedLegacy]
  .filter(
    (className) =>
      !definedLegacy.has(className) &&
      !LEGACY_CLASS_SAFELIST.has(className) &&
      !legacyDynamicPrefixes.has(className),
  )
  .sort();
const legacyUnreferencedDefinitions = [...definedLegacy]
  .filter((className) => !referencedLegacy.has(className))
  .sort();

// ── 动态拼接探测（ai-*）────────────────────────────────────────────
const dynamicExpressions = sourceFiles.flatMap((file, index) => {
  // 只匹配动态生成 `.ai-*` token 的写法（`ai-prefix-${value}`）；
  // `ai-static${condition ? " is-active" : ""}` 只动态追加非 ai modifier，不在此列。
  const matches = [...sources[index].matchAll(/ai-[a-zA-Z0-9_-]*-\$\{/g)];
  return matches.map((match) => ({
    location: `${path.relative(ROOT, file)}:${match.index}`,
    prefix: match[0].slice(0, -2),
  }));
});
const unsafelistedDynamicExpressions = dynamicExpressions.filter(
  ({ prefix }) => !DYNAMIC_AI_CLASS_VALUES.has(prefix),
);

console.log(
  `AI class 引用报告：${defined.size} 个定义，${referenced.size} 个引用，${unreferencedDefinitions.length} 个无引用定义。`,
);
if (unreferencedDefinitions.length > 0) {
  console.log(`无引用定义：\n${unreferencedDefinitions.join("\n")}`);
}
console.log(
  `遗留类（App.css 族）引用报告：${definedLegacy.size} 个定义，${referencedLegacy.size} 个引用，${legacyUnreferencedDefinitions.length} 个无引用定义。`,
);
if (legacyUnreferencedDefinitions.length > 0) {
  console.log(`无引用定义：\n${legacyUnreferencedDefinitions.join("\n")}`);
}

const failures = [];
if (unreferencedDefinitions.length > MAX_UNREFERENCED_AI_CLASSES) {
  failures.push(
    `无引用定义从基线 ${MAX_UNREFERENCED_AI_CLASSES} 增长到 ${unreferencedDefinitions.length}；请删除死样式或确认后更新基线`,
  );
}
if (undefinedReferences.length > 0) {
  failures.push(`静态引用但未定义：${undefinedReferences.join(", ")}`);
}
if (legacyUnreferencedDefinitions.length > MAX_UNREFERENCED_LEGACY_CLASSES) {
  failures.push(
    `遗留类无引用定义从基线 ${MAX_UNREFERENCED_LEGACY_CLASSES} 增长到 ${legacyUnreferencedDefinitions.length}；请删除死样式或确认后更新基线`,
  );
}
if (legacyUndefinedReferences.length > 0) {
  failures.push(`遗留类静态引用但未定义：${legacyUndefinedReferences.join(", ")}`);
}
if (unsafelistedDynamicExpressions.length > 0) {
  failures.push(
    `发现未枚举的动态 .ai-* class：${unsafelistedDynamicExpressions
      .map(({ location, prefix }) => `${location} (${prefix}*)`)
      .join(", ")}`,
  );
}
if (failures.length > 0) {
  throw new Error(`AI class 静态检查失败：\n- ${failures.join("\n- ")}`);
}
