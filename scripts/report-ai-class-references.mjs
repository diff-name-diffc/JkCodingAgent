import { readFile, readdir } from "node:fs/promises";
import path from "node:path";
import process from "node:process";

const ROOT = process.cwd();
const SOURCE_ROOT = path.join(ROOT, "src");
const STYLE_FILE = path.join(SOURCE_ROOT, "styles/tailwind.css");
const SOURCE_EXTENSIONS = new Set([".ts", ".tsx", ".css"]);

// 动态生成 `.ai-*` class 时必须在这里完整枚举所有实际 class，防止静态扫描误删。
// 当前代码没有动态 `.ai-*` class；`is-*` 等状态 modifier 不属于本清单。
const DYNAMIC_AI_CLASS_VALUES = new Map([
  ["ai-graph-chip--node-", ["pending", "running", "succeeded", "failed", "skipped", "cancelled"]],
  ["ai-graph-node--", ["pending", "running", "succeeded", "failed", "skipped", "cancelled"]],
  ["ai-graph-node-status--", ["pending", "running", "succeeded", "failed", "skipped", "cancelled"]],
  ["ai-graph-notice-row--", ["running", "succeeded", "failed"]],
  ["ai-graph-tool-card--", ["running", "succeeded", "failed"]],
  ["ai-graph-edge--", ["waiting", "ready", "active", "done", "failed"]],
  ["ai-status-pill--", ["neutral", "accent", "success", "warning", "danger"]],
  ["ai-tool-call-node--", ["running", "success", "error"]],
]);
const AI_CLASS_SAFELIST = new Set(
  [...DYNAMIC_AI_CLASS_VALUES].flatMap(([prefix, values]) => values.map((value) => `${prefix}${value}`)),
);

// 现存历史债务基线：新增无引用定义会让 CI 失败；清理后应同步下调，禁止反向增长。
const MAX_UNREFERENCED_AI_CLASSES = 0;

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

const styleSource = await readFile(STYLE_FILE, "utf8");
const defined = collectMatches(styleSource, /\.((?:ai)-[a-zA-Z0-9_-]+)/g);
const sourceFiles = await listSourceFiles(SOURCE_ROOT);
const sources = await Promise.all(sourceFiles.map((file) => readFile(file, "utf8")));
const consumerSource = sources.join("\n");
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

const failures = [];
if (unreferencedDefinitions.length > MAX_UNREFERENCED_AI_CLASSES) {
  failures.push(
    `无引用定义从基线 ${MAX_UNREFERENCED_AI_CLASSES} 增长到 ${unreferencedDefinitions.length}；请删除死样式或确认后更新基线`,
  );
}
if (undefinedReferences.length > 0) {
  failures.push(`静态引用但未定义：${undefinedReferences.join(", ")}`);
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
