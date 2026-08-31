import { readFile, readdir } from "node:fs/promises";
import path from "node:path";
import process from "node:process";

const ROOT = process.cwd();
const APP_MODULE = path.join(ROOT, "src-tauri/src/app/mod.rs");
const FRONTEND_ROOT = path.join(ROOT, "src");
const FRONTEND_EXTENSIONS = new Set([".ts", ".tsx"]);

async function listSourceFiles(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const nested = await Promise.all(
    entries.map(async (entry) => {
      const entryPath = path.join(directory, entry.name);
      if (entry.isDirectory()) return listSourceFiles(entryPath);
      return FRONTEND_EXTENSIONS.has(path.extname(entry.name)) ? [entryPath] : [];
    }),
  );
  return nested.flat();
}

export function parseRegisteredCommands(source) {
  const match = source.match(/\.invoke_handler\(tauri::generate_handler!\[([\s\S]*?)\]\)/);
  if (!match) throw new Error("未找到唯一的 tauri::generate_handler! 命令注册表");

  return match[1]
    .split(",")
    .map((entry) => entry.replace(/\/\/.*$/gm, "").trim())
    .filter(Boolean)
    .map((entry) => {
      const segments = entry.split("::");
      const name = segments.at(-1);
      if (!name || !/^[a-z][a-z0-9_]*$/.test(name)) {
        throw new Error(`无法解析 Tauri 命令注册项：${entry}`);
      }
      return name;
    });
}

export function parseDirectInvokes(source) {
  const commands = new Set();
  const pattern = /\b(?:invoke|safeInvoke)(?:\s*<[\s\S]{0,500}?>)?\s*\(\s*["'`]([a-z][a-z0-9_]*)["'`]/g;
  for (const match of source.matchAll(pattern)) commands.add(match[1]);
  return commands;
}

function quotedLiteralPattern(command) {
  return new RegExp(`["'\\x60]${command}["'\\x60]`);
}

function duplicates(values) {
  const seen = new Set();
  const repeated = new Set();
  for (const value of values) {
    if (seen.has(value)) repeated.add(value);
    seen.add(value);
  }
  return [...repeated].sort();
}

const appSource = await readFile(APP_MODULE, "utf8");
const registered = parseRegisteredCommands(appSource);
const registeredSet = new Set(registered);
const sourceFiles = await listSourceFiles(FRONTEND_ROOT);
const frontendSources = await Promise.all(sourceFiles.map((file) => readFile(file, "utf8")));
const frontendSource = frontendSources.join("\n");
const directInvokes = parseDirectInvokes(frontendSource);

const duplicateRegistrations = duplicates(registered);
const missingFrontendConsumers = registered.filter(
  (command) => !quotedLiteralPattern(command).test(frontendSource),
);
const unregisteredDirectInvokes = [...directInvokes]
  .filter((command) => !registeredSet.has(command))
  .sort();

const failures = [];
if (duplicateRegistrations.length > 0) {
  failures.push(`重复注册：${duplicateRegistrations.join(", ")}`);
}
if (missingFrontendConsumers.length > 0) {
  failures.push(`后端已注册但前端无命令字面量消费者：${missingFrontendConsumers.join(", ")}`);
}
if (unregisteredDirectInvokes.length > 0) {
  failures.push(`前端直接调用但后端未注册：${unregisteredDirectInvokes.join(", ")}`);
}

if (failures.length > 0) {
  throw new Error(`Tauri 命令契约检查失败：\n- ${failures.join("\n- ")}`);
}

console.log(
  `Tauri 命令契约通过：${registered.length} 个后端注册命令，${directInvokes.size} 个前端直接调用命令。`,
);
