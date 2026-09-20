import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Excalidraw } from "@excalidraw/excalidraw";
import "@excalidraw/excalidraw/index.css";
// 字体离线自托管（EXCALIDRAW_ASSET_PATH），必须早于画布首次渲染。
import "./excalidraw-setup";
import type { ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";
import type { ExcalidrawElement } from "@excalidraw/excalidraw/element/types";
import { Bot } from "lucide-react";
import { useIsDarkTheme } from "../../hooks/useIsDarkTheme";
import { useDockedBrowserPanel } from "../../hooks/useDockedBrowserPanel";
import { ErrorBoundary } from "../ErrorBoundary";
import { resolveCanvasTheme } from "./architecture-theme";
import { useArchRunListener } from "./arch-run-listener";
import { type CanvasBlockInfo } from "./canvas-block-info";
import { createScenePersister, loadCanvasScene } from "./canvas-persistence";
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

const ArchitectureCanvas = memo(function ArchitectureCanvas({
  onApi,
  onBlockState,
  onSceneChange,
}: {
  onApi: (api: ExcalidrawImperativeAPI | null) => void;
  onBlockState: (info: CanvasBlockInfo | null) => void;
  onSceneChange: (elements: readonly ExcalidrawElement[]) => void;
}) {
  const [blocked, setBlocked] = useState<CanvasBlockInfo | null>(null);
  const [remountKey, setRemountKey] = useState(0);
  const persisterRef = useRef(createScenePersister());
  const dark = useIsDarkTheme();

  // 场景持久化数据仅在（重）挂载时读取一次（remountKey 变化即有意重读）。
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const initialData = useMemo(() => ({ elements: loadCanvasScene() }), [remountKey]);

  const handleApi = useCallback(
    (api: ExcalidrawImperativeAPI) => {
      onApi(api);
      onBlockState(null);
      const elements = api.getSceneElements();
      onSceneChange(elements);
      // 恢复的场景首次挂载时适配视口（空画布 scrollToContent 无意义）。
      if (elements.length > 0) api.scrollToContent(elements, { fitToContent: true });
    },
    [onApi, onBlockState, onSceneChange],
  );

  const handleChange = useCallback(
    (elements: readonly ExcalidrawElement[]) => {
      onSceneChange(elements);
      persisterRef.current(elements);
    },
    [onSceneChange],
  );

  // 崩溃重试：重挂载 Excalidraw（持久化场景在 initialData 恢复，内容无损）。
  const retryRemount = useCallback(() => {
    setBlocked(null);
    onBlockState(null);
    setRemountKey((key) => key + 1);
  }, [onBlockState]);

  return (
    <div className="ai-arch-canvas">
      {blocked ? (
        <CanvasBlockedPanel info={blocked} onRetry={retryRemount} />
      ) : (
        <ErrorBoundary
          label="架构画布"
          fallback={(error, reset) => (
            <CanvasBlockedPanel
              info={{ kind: "crash", message: error.message, stack: error.stack }}
              onRetry={() => {
                reset();
                retryRemount();
              }}
              onShown={onBlockState}
            />
          )}
        >
          <div className="ai-arch-excalidraw-host">
            <Excalidraw
              key={remountKey}
              initialData={initialData}
              excalidrawAPI={handleApi}
              // theme prop 是响应式的（内部 updateScene），切换主题不重建画布。
              theme={resolveCanvasTheme(dark)}
              langCode="zh-CN"
              onChange={handleChange}
            />
          </div>
        </ErrorBoundary>
      )}
    </div>
  );
});

export function ArchitectureView() {
  const apiRef = useRef<ExcalidrawImperativeAPI | null>(null);
  /** 画布阻断原因：供执行监听器（architecture_run 回传）附加诊断上下文。 */
  const blockInfoRef = useRef<CanvasBlockInfo | null>(null);
  const [shapeCount, setShapeCount] = useState(0);

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
  const handleSceneChange = useCallback((elements: readonly ExcalidrawElement[]) => {
    setShapeCount((prev) => (prev === elements.length ? prev : elements.length));
  }, []);

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
        <ArchitectureCanvas
          onApi={handleApi}
          onBlockState={handleBlockState}
          onSceneChange={handleSceneChange}
        />

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
