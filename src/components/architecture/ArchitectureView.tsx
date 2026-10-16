import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Excalidraw } from "@excalidraw/excalidraw";
import "@excalidraw/excalidraw/index.css";
// 字体离线自托管（EXCALIDRAW_ASSET_PATH），必须早于画布首次渲染。
import "./excalidraw-setup";
import type { AppState, BinaryFiles, ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";
import type { ExcalidrawElement } from "@excalidraw/excalidraw/element/types";
import { Bot } from "lucide-react";
import { useIsDarkTheme } from "../../hooks/useIsDarkTheme";
import { useDockedBrowserPanel } from "../../hooks/useDockedBrowserPanel";
import { ErrorBoundary } from "../ErrorBoundary";
import { resolveCanvasTheme } from "./architecture-theme";
import { useArchRunListener } from "./arch-run-listener";
import { type CanvasBlockInfo } from "./canvas-block-info";
import { EMPTY_CANVAS_SCENE, type CanvasScene } from "./canvas-files";
import { CanvasFileBar } from "./CanvasFileBar";
import { CanvasNameDialog } from "./CanvasNameDialog";
import { useCanvasDocument } from "./use-canvas-document";
import { useArchitectureChat } from "./chat/useArchitectureChat";
import { ArchitectureChatPanel } from "./chat/ArchitectureChatPanel";
import { ARCH_CHAT_WIDTH_KEY } from "./chat/architecture-chat-prefs";

/**
 * 画布阻断面板：渲染崩溃（ErrorBoundary 捕获）时替代「画布无声消失」，
 * 展示报错与堆栈，可重试重挂载。
 */
function CanvasBlockedPanel({
  info,
  onRetry,
  onShown,
}: {
  info: CanvasBlockInfo;
  onRetry?: () => void;
  onShown?: (info: CanvasBlockInfo) => void;
}) {
  useEffect(() => {
    onShown?.(info);
  }, [info, onShown]);

  return (
    <div className="ai-arch-canvas-blocked">
      <div className="ai-arch-canvas-blocked-icon">⚠</div>
      <div className="ai-arch-canvas-blocked-title">画布渲染崩溃</div>
      <div className="ai-arch-canvas-blocked-message">
        <p>{info.message || "未知渲染错误"}</p>
      </div>
      {info.stack ? <pre className="ai-arch-canvas-blocked-stack">{info.stack}</pre> : null}
      {onRetry && (
        <button type="button" className="ai-arch-canvas-blocked-retry" onClick={onRetry}>
          重新加载画布
        </button>
      )}
    </div>
  );
}

/**
 * Excalidraw 画布壳：initialData 随 epoch（画板切换/崩溃重载）更换并重挂载，
 * onChange 原样上抛（elements/appState/files，父级负责防抖落盘）。
 */
const ArchitectureCanvas = memo(function ArchitectureCanvas({
  epoch,
  scene,
  onApi,
  onBlockState,
  onSceneChange,
}: {
  epoch: number;
  scene: CanvasScene;
  onApi: (api: ExcalidrawImperativeAPI | null) => void;
  onBlockState: (info: CanvasBlockInfo | null) => void;
  onSceneChange: (
    elements: readonly ExcalidrawElement[],
    appState: AppState,
    files: BinaryFiles,
  ) => void;
}) {
  const dark = useIsDarkTheme();

  // scene 引用仅随画板切换/崩溃重载更换（父级与 epoch 同步替换），
  // 以此memo即可；key={epoch} 驱动 Excalidraw 重挂载完成完整回显。
  const initialData = useMemo(
    () => ({ elements: scene.elements, appState: scene.appState, files: scene.files }),
    [scene],
  );

  const handleApi = useCallback(
    (api: ExcalidrawImperativeAPI) => {
      onApi(api);
      onBlockState(null);
      const elements = api.getSceneElements();
      // 恢复的场景首次挂载时适配视口（空画布 scrollToContent 无意义）。
      if (elements.length > 0) api.scrollToContent(elements, { fitToContent: true });
    },
    [onApi, onBlockState],
  );

  return (
    <div className="ai-arch-excalidraw-host">
      <Excalidraw
        key={epoch}
        initialData={initialData}
        excalidrawAPI={handleApi}
        // theme prop 是响应式的（内部 updateScene），切换主题不重建画布。
        theme={resolveCanvasTheme(dark)}
        langCode="zh-CN"
        onChange={onSceneChange}
      />
    </div>
  );
});

export function ArchitectureView() {
  const apiRef = useRef<ExcalidrawImperativeAPI | null>(null);
  /** 画布阻断原因：供执行监听器（architecture_run 回传）附加诊断上下文。 */
  const blockInfoRef = useRef<CanvasBlockInfo | null>(null);
  const [shapeCount, setShapeCount] = useState(0);
  const doc = useCanvasDocument();

  const handleApi = useCallback((api: ExcalidrawImperativeAPI | null) => {
    apiRef.current = api;
  }, []);
  const getCanvasApi = useCallback(() => apiRef.current, []);
  const getBlockInfo = useCallback(() => blockInfoRef.current, []);
  // 阻断原因首写生效；挂载成功清空
  const handleBlockState = useCallback((info: CanvasBlockInfo | null) => {
    if (info === null) {
      blockInfoRef.current = null;
      return;
    }
    blockInfoRef.current ??= info;
  }, []);
  const queueSceneSave = doc.handleSceneChange;
  const handleSceneChange = useCallback(
    (elements: readonly ExcalidrawElement[], appState: AppState, files: BinaryFiles) => {
      setShapeCount((prev) => (prev === elements.length ? prev : elements.length));
      queueSceneSave({ elements, appState, files });
    },
    [queueSceneSave],
  );

  // ── Ctrl/Cmd+S 显式保存 ──────────────────────────────────────────
  const [saveNameDialogOpen, setSaveNameDialogOpen] = useState(false);
  /** 窗口级快捷键监听器注册一次，经 ref 读最新画板身份。 */
  const saveStateRef = useRef<{ ready: boolean; canvasId: string | null }>({
    ready: false,
    canvasId: null,
  });
  const { ready: docReady, current: docCurrent } = doc;
  useEffect(() => {
    saveStateRef.current = { ready: docReady, canvasId: docCurrent.id };
  }, [docReady, docCurrent]);

  /** 显式保存以画布实时快照为准（不依赖防抖待保存状态）。 */
  const snapshotScene = useCallback((): CanvasScene => {
    const api = apiRef.current;
    if (!api) return EMPTY_CANVAS_SCENE;
    return {
      elements: api.getSceneElements(),
      appState: api.getAppState(),
      files: api.getFiles(),
    };
  }, []);

  const saveNow = doc.saveNow;
  // capture 阶段抢在 Excalidraw/浏览器默认行为前；仅在架构视图可见（或焦点
  // 在视图内）时接管——视图保活隐藏后不劫持全局快捷键。
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.key !== "s" && event.key !== "S") || (!event.ctrlKey && !event.metaKey)) return;
      const target = event.target as HTMLElement | null;
      const inShell = Boolean(target?.closest?.(".ai-arch-shell"));
      const shell = document.querySelector(".ai-arch-shell");
      const shellVisible =
        shell && typeof shell.checkVisibility === "function"
          ? shell.checkVisibility({ checkVisibilityCSS: true })
          : false;
      if (!inShell && !shellVisible) return;
      event.preventDefault();
      event.stopPropagation();
      if (!saveStateRef.current.ready) return;
      // 未命名画板先让用户命名；已命名直接落盘最新快照。
      if (saveStateRef.current.canvasId === null) setSaveNameDialogOpen(true);
      else void saveNow(undefined, snapshotScene());
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [saveNow, snapshotScene]);

  const chat = useArchitectureChat({ getCanvasApi });
  // 画布执行监听：architecture_run 工具 ↔ 前端解释器往返
  useArchRunListener(getCanvasApi, getBlockInfo);

  // 助手默认宽像素锚定 360（设计 §5.6 规格 320–400px）；此前按视口 28% 计算，
  // 1920px 视口默认会漂到 ~537px，画布不再占主导。
  const { effectiveWidth, handleResizeStart } = useDockedBrowserPanel(ARCH_CHAT_WIDTH_KEY, {
    minWidth: 320,
    defaultWidthPx: 360,
    maxRatio: 0.6,
  });

  const collapsed = chat.prefs.collapsed;

  return (
    <div className="ai-home-pane">
      <div className="ai-arch-shell">
        <div className="ai-arch-canvas">
          {doc.ready ? (
            <>
              <ErrorBoundary
                label="架构画布"
                fallback={(error, reset) => (
                  <CanvasBlockedPanel
                    info={{ kind: "crash", message: error.message, stack: error.stack }}
                    onRetry={() => {
                      reset();
                      // 崩溃重试：以磁盘最新保存重载画板（epoch 推进触发重挂载）。
                      void doc.reloadCurrent();
                    }}
                    onShown={handleBlockState}
                  />
                )}
              >
                <ArchitectureCanvas
                  epoch={doc.epoch}
                  scene={doc.initialScene}
                  onApi={handleApi}
                  onBlockState={handleBlockState}
                  onSceneChange={handleSceneChange}
                />
              </ErrorBoundary>
              <CanvasFileBar
                current={doc.current}
                canvases={doc.canvases}
                onOpen={(canvasId) => void doc.openCanvas(canvasId)}
                onCreate={() => void doc.createCanvas()}
                onRename={(name) => void doc.renameCanvas(name)}
                onDelete={(canvasId) => void doc.deleteCanvas(canvasId)}
              />
              <CanvasNameDialog
                open={saveNameDialogOpen}
                title="保存画板"
                initialName={doc.current.name}
                onCancel={() => setSaveNameDialogOpen(false)}
                onSubmit={(name) => {
                  setSaveNameDialogOpen(false);
                  void doc.saveNow(name, snapshotScene());
                }}
              />
            </>
          ) : (
            <div className="ai-arch-canvas-loading">正在加载画板…</div>
          )}
        </div>

        {collapsed ? (
          <button
            type="button"
            className="ai-arch-chat-dock"
            onClick={() => chat.updatePrefs({ collapsed: false })}
            title="展开架构助手"
          >
            <Bot size={15} strokeWidth={1.9} />
            <span className="ai-arch-chat-dock-label">架构助手</span>
          </button>
        ) : (
          <>
            <div
              className="ai-arch-chat-resizer"
              onMouseDown={handleResizeStart}
              role="separator"
              aria-orientation="vertical"
              aria-label="调整架构助手面板宽度"
            />
            <aside className="ai-arch-chat-aside" style={{ width: effectiveWidth }}>
              <ArchitectureChatPanel chat={chat} canvasShapeCount={shapeCount} />
            </aside>
          </>
        )}
      </div>
    </div>
  );
}
