/// <reference types="vite/client" />

interface ImportMetaEnv {
  /**
   * tldraw SDK 生产许可证密钥（构建时注入，例：`VITE_TLDRAW_LICENSE_KEY=xxx pnpm tauri build`）。
   * tldraw v5 在生产包中强制许可校验：无有效密钥时画布挂载约 5 秒后即被许可
   * 门禁整体卸载（见 components/architecture/ArchitectureView 的阻断面板说明）。
   */
  readonly VITE_TLDRAW_LICENSE_KEY?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
