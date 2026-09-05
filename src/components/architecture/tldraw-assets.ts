import { getAssetUrlsByImport } from "@tldraw/assets/imports.vite";

/**
 * tldraw 静态资源自托管（字体/图标/翻译）。
 *
 * tldraw v5 运行时默认从 `https://cdn.tldraw.com/<version>/` 拉取这些资源，
 * 桌面应用离线时工具栏图标与字体会空白，因此改为经 Vite `?url` 导入本地化。
 *
 * 模块级单例保证引用稳定；本模块仅被懒加载的架构设计视图导入，不进主包。
 * 注意：`@tldraw/assets` 必须与 `tldraw` 锁定同一版本（资源清单与运行时强耦合）。
 */
export const tldrawAssetUrls = getAssetUrlsByImport();
