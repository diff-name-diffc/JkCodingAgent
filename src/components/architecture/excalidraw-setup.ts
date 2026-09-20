/**
 * Excalidraw 资源自托管引导（必须早于画布首次渲染导入）。
 *
 * Excalidraw 运行时的字体（Excalifont/Virgil/Xiaolai 子集等）默认从
 * unpkg CDN 拉取，桌面应用离线时文字会回退成系统字体；这里把资源根指向
 * 本地打包的 public 副本（scripts/sync-excalidraw-assets.mjs 从
 * node_modules 同步，dev/build 前自动执行）。
 */

declare global {
  interface Window {
    EXCALIDRAW_ASSET_PATH?: string;
  }
}

window.EXCALIDRAW_ASSET_PATH = "/excalidraw-assets/";

export {};
