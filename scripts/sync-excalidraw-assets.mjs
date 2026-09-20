/**
 * 同步 Excalidraw 运行时资源（字体/子集 worker 数据）到 public/ 供离线自托管。
 *
 * Excalidraw 运行时经 window.EXCALIDRAW_ASSET_PATH 拉取字体子集（CJK 按需
 * 裁剪），默认指向 unpkg CDN；桌面应用必须离线可用，故在 dev/build 前把
 * node_modules 里的 dist/prod/{fonts,data} 复制到 public/excalidraw-assets/。
 * 该目录不入库（.gitignore），由本脚本按需重建。
 */
import { cpSync, existsSync, rmSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const root = join(dirname(fileURLToPath(import.meta.url)), "..");
// 包未导出 ./package.json，改为解析入口文件再回溯包根（pnpm 下穿透 .pnpm 符号链接）。
const entry = require.resolve("@excalidraw/excalidraw");
const src = join(entry.split(`${join("dist", "prod")}`)[0] || dirname(entry), "dist", "prod");
const dest = join(root, "public", "excalidraw-assets");

if (!existsSync(join(src, "fonts"))) {
  console.error(`未找到 Excalidraw 资源目录：${src}（请先 pnpm install）`);
  process.exit(1);
}

rmSync(dest, { recursive: true, force: true });
for (const dir of ["fonts", "data"]) {
  if (existsSync(join(src, dir))) {
    cpSync(join(src, dir), join(dest, dir), { recursive: true });
  }
}
console.log(`Excalidraw 资源已同步 → ${dest}`);
