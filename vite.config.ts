import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

function isNodeModule(id: string, packageName: string) {
  return id.replace(/\\/g, "/").includes(`/node_modules/${packageName}/`);
}

function isMarkdownVendor(id: string) {
  const normalizedId = id.replace(/\\/g, "/");
  return [
    "react-markdown",
    "rehype-katex",
    "rehype-raw",
    "rehype-sanitize",
    "remark-gfm",
    "remark-math",
    "katex",
  ].some((packageName) => normalizedId.includes(`/node_modules/${packageName}/`));
}

// tldraw 画布独占依赖（经 lockfile 比对，均不在其余依赖树中）。
// 故意排除：@radix-ui/*（主包 UI 在用；子串匹配不会命中带 @ 前缀的路径）
// 与 use-sync-external-store（react-query 在用）。
const TL_DRAW_VENDOR_PACKAGES = [
  "tldraw",
  "@tldraw",
  "@tiptap",
  "radix-ui",
  "idb",
  "lz-string",
  "classnames",
  "rbush",
  "eventemitter3",
  "is-plain-object",
  "fast-equals",
  "rope-sequence",
  "orderedmap",
  "w3c-keyname",
];

function isTldrawVendor(id: string) {
  const normalizedId = id.replace(/\\/g, "/");
  return (
    TL_DRAW_VENDOR_PACKAGES.some((packageName) => isNodeModule(normalizedId, packageName)) ||
    // lodash 子包（lodash.uniq/isequal/throttle/isequalwith，@tldraw/utils 独占）
    // 走前缀匹配——若经 isNodeModule 会被强加尾斜杠（`lodash./`）而永不命中。
    normalizedId.includes("/node_modules/lodash.") ||
    normalizedId.includes("/node_modules/prosemirror-")
  );
}

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react()],
  optimizeDeps: {
    // `@tldraw/assets/imports.vite` 内含大量 `./xxx.json?url` 静态导入，
    // 让 rolldown 依赖预构建报 UNLOADABLE_DEPENDENCY；排除后由 Vite
    // 原生 ?url 资产管线按需处理（该模块仅被懒加载的架构画布引用）。
    exclude: ["@tldraw/assets"],
  },
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
              name: "markdown-vendor",
              test: isMarkdownVendor,
              minSize: 20 * 1024,
            },
            {
              name: "xterm-vendor",
              test: (id) => isNodeModule(id, "@xterm"),
              minSize: 20 * 1024,
            },
            {
              name: "tldraw-vendor",
              test: isTldrawVendor,
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
