/**
 * 主页架构 pane 的挂载/可见两态机（UI-15 遗留领取，纯函数）。
 *
 * 语义（对齐 project/terminal-dock.ts 与 ProjectWorkspaceLayout 的保活先例）：
 * - mounted：ArchitectureView 已挂载——tldraw editor 实例、视口 camera、
 *   撤销栈与选中态存活（形状另经 IndexedDB persistenceKey 持久化）；
 * - visible：pane 在主页显示（隐藏 = visibility:hidden 叠层，不卸载组件；
 *   不用 display:none——tldraw 的 .tl-container 依赖容器确定尺寸，
 *   visibility 路线保留布局尺寸，与多项目保活同款）。
 *
 * mounted 单调不回退：主页视图切换没有「终止画布」动作，只有显示/隐藏；
 * 首次切到架构视图才挂载（lazy import 语义保留，不 always-mounted）。
 */

export interface HomePaneKeepAlive {
  mounted: boolean;
  visible: boolean;
}

export const HOME_PANE_UNMOUNTED: HomePaneKeepAlive = { mounted: false, visible: false };

/**
 * 按「架构视图是否为当前视图」推进两态：
 * - 激活 → mounted=true, visible=true；
 * - 切走 → 已挂载则 {mounted:true, visible:false}（保活隐藏），
 *   从未挂载则保持 UNMOUNTED（不产生「隐藏但挂载」的空转态）。
 */
export function nextHomePaneKeepAlive(
  state: HomePaneKeepAlive,
  viewActive: boolean,
): HomePaneKeepAlive {
  if (viewActive) return { mounted: true, visible: true };
  return state.mounted ? { mounted: true, visible: false } : HOME_PANE_UNMOUNTED;
}
