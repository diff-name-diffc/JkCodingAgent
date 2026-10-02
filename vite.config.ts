import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

function isNodeModule(id: string, packageName: string) {
  return id.replace(/\\/g, "/").includes(`/node_modules/${packageName}/`);
}

// 聊天 markdown 渲染栈（streamdown 生态 + shiki 内核 + katex）。mermaid、
// @streamdown/mermaid 与 shiki 语言包刻意排除：它们按需动态加载，并入本组
// 会被提升为入口级 chunk。遗留 react-markdown 管线（MarkdownRendererImpl）
// 整体走 React.lazy，不再单列 vendor 组——其模块自动归入懒加载 chunk，
// 避免分组误匹配聊天管线共享的 unified/rehype 依赖导致入口预加载回归。
const STREAMDOWN_VENDOR_PACKAGES = [
  "streamdown",
  "@streamdown",
  "marked",
  "remend",
  "shiki",
  "@shikijs",
  "katex",
];

function isStreamdownVendor(id: string) {
  const normalizedId = id.replace(/\\/g, "/");
  if (
    normalizedId.includes("/node_modules/mermaid/") ||
    normalizedId.includes("/node_modules/@streamdown/mermaid/") ||
    normalizedId.includes("/node_modules/@shikijs/langs/") ||
    /\/node_modules\/shiki\/dist\/langs\//.test(normalizedId)
  ) {
    return false;
  }
  return STREAMDOWN_VENDOR_PACKAGES.some((packageName) => isNodeModule(normalizedId, packageName));
}

// Excalidraw 画布独占依赖（经 lockfile 比对，均不在其余依赖树中）。
const EXCALIDRAW_VENDOR_PACKAGES = ["@excalidraw", "roughjs", "perfect-freehand", "fractional-indexing"];

function isExcalidrawVendor(id: string) {
  const normalizedId = id.replace(/\\/g, "/");
  return EXCALIDRAW_VENDOR_PACKAGES.some((packageName) => isNodeModule(normalizedId, packageName));
}

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react()],
  build: {
    rolldownOptions: {
      output: {
        codeSplitting: {
          minSize: 20 * 1024,
          groups: [
            {
              name: "monaco-vendor",
              test: (id) => isNodeModule(id, "monaco-editor") || isNodeModule(id, "@monaco-editor"),
              minSize: 20 * 1024,
            },
            {
              name: "streamdown-vendor",
              test: isStreamdownVendor,
              minSize: 20 * 1024,
            },
            {
              name: "xterm-vendor",
              test: (id) => isNodeModule(id, "@xterm"),
              minSize: 20 * 1024,
            },
            {
              name: "excalidraw-vendor",
              test: isExcalidrawVendor,
              minSize: 20 * 1024,
            },
          ],
        },
      },
    },
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
