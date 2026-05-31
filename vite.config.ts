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
              name: "markdown-vendor",
              test: isMarkdownVendor,
              minSize: 20 * 1024,
            },
            {
              name: "three-vendor",
              test: (id) => isNodeModule(id, "three"),
              minSize: 20 * 1024,
            },
            {
              name: "xterm-vendor",
              test: (id) => isNodeModule(id, "@xterm"),
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
