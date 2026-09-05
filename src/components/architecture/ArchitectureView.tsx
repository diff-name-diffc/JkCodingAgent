import { memo, useCallback, useEffect, useRef, useState } from "react";
import { Tldraw, type Editor } from "tldraw";
import "tldraw/tldraw.css";
import { Bot } from "lucide-react";
import { useIsDarkTheme } from "../../hooks/useIsDarkTheme";
import { isDarkActive } from "../../lib/theme";
import { useDockedBrowserPanel } from "../../hooks/useDockedBrowserPanel";
import { ErrorBoundary } from "../ErrorBoundary";
import { applyTldrawColorScheme } from "./architecture-theme";
import { tldrawAssetUrls } from "./tldraw-assets";
import { useArchRunListener } from "./arch-run-listener";
import { TLDR_LICENSE_GATE_SELECTOR, type CanvasBlockInfo } from "./canvas-block-info";
import { useArchitectureChat } from "./chat/useArchitectureChat";
import { ArchitectureChatPanel } from "./chat/ArchitectureChatPanel";
import { ARCH_CHAT_WIDTH_KEY } from "./chat/architecture-chat-prefs";

/**
 * 架构设计画布的本地持久化 key（IndexedDB 单文档，沿用 `jkcodingagent.*.v1` 惯例）。
 * 后续做多文档/后端存储时换掉 persistenceKey，改自建 TLStore + 快照接口。
 */
export const ARCHITECTURE_PERSISTENCE_KEY = "jkcodingagent.architecture.v1";

/**
 * 画布阻断面板：替代「画布无声消失」。三类原因——
 * 1. license：生产包无有效 tldraw 许可证，LicenseProvider 在 ~5 秒后把整个
 *    editor 子树替换为隐藏占位节点（这就是「拖拉时画布突然关闭」的根因）；
 * 2. crash：画布渲染抛错（ErrorBoundary 捕获），展示报错与堆栈；
 * 3. unexpected：editor 被意外卸载的兜底（门禁 testid 跨版本变更时的保险）。
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
      <div className="ai-arch-canvas-blocked-title">
        {info.kind === "license"
          ? "画布已被 tldraw 许可校验关闭"
          : info.kind === "crash"
            ? "画布渲染崩溃"
            : "画布意外关闭"}
      </div>
      <div className="ai-arch-canvas-blocked-message">
        {info.kind === "license" ? (
          <>
            <p>
              生产包未检测到有效的 tldraw 许可证（缺失、已过期或 host
              不匹配）：画布挂载约 5 秒后被官方许可门禁（unlicensed-production /
              expired）整体卸载，表现为「操作画布时突然关闭」。开发模式（
              tauri dev → http://localhost）被 tldraw 判定为开发环境、跳过该校验，因此开发调试一切正常。
            </p>
            <p>
              修复：获取 tldraw 许可证后，在构建时设置环境变量
              VITE_TLDRAW_LICENSE_KEY=&lt;许可证密钥&gt; 重新打包。许可类型：个人/非商业项目可申请免费的
              hobby 许可（画布显示 made with tldraw 水印，申请地址
              tldraw.dev/get-a-license/hobby）；评估可用 100 天试用许可（
              tldraw.dev/get-a-license/trial）；商用授权联系
              sales@tldraw.com。桌面端运行地址为
              tauri://localhost，申请时需说明以便 host 限制匹配（Native 授权）。
            </p>
          </>
        ) : info.kind === "crash" ? (
          <p>{info.message || "未知渲染错误"}</p>
        ) : (
          <p>画布在未切换视图的情况下被意外卸载。请重试；若反复出现，请查看控制台日志定位。</p>
        )}
      </div>
      {info.kind === "crash" && info.stack ? (
        <pre className="ai-arch-canvas-blocked-stack">{info.stack}</pre>
      ) : null}
      {onRetry ? (
        <button type="button" className="ai-error-boundary-btn" onClick={onRetry}>
          重试
        </button>
      ) : null}
    </div>
  );
}

/**
 * 画布子组件：memo 隔离——右侧聊天面板的高频状态变化（流式事件）不得触发
 * Tldraw 重渲染（props 变化可能重建 editor，丢视口/撤销栈，同 colorScheme 坑）。
 */
const ArchitectureCanvas = memo(function ArchitectureCanvas({
  onEditor,
  onBlockState,
}: {
  onEditor: (editor: Editor | null) => void;
  onBlockState: (info: CanvasBlockInfo | null) => void;
}) {
  const editorRef = useRef<Editor | null>(null);
  const hostRef = useRef<HTMLDivElement | null>(null);
  /** 主动重挂载标记：重试触发的旧 editor 卸载回调需吞掉，避免误判为新的阻断。 */
  const remountingRef = useRef(false);
  /** ErrorBoundary 捕获的错误：在回退渲染阶段同步记录，先于 editor 卸载回调。 */
  const boundaryErrorRef = useRef<Error | null>(null);
  const [remountKey, setRemountKey] = useState(0);
  const [blocked, setBlocked] = useState<CanvasBlockInfo | null>(null);
  const dark = useIsDarkTheme();

  // THEME_CHANGE_EVENT → useIsDarkTheme → 增量更新偏好，不重建画布
  useEffect(() => {
    if (editorRef.current) applyTldrawColorScheme(editorRef.current, dark);
  }, [dark]);

  // 阻断上报：本地首写生效（门禁先触发，后续伴随的卸载事件不覆盖先到的原因）
  const reportBlock = useCallback(
    (info: CanvasBlockInfo) => {
      setBlocked((prev) => prev ?? info);
      onBlockState(info);
    },
    [onBlockState],
  );

  // tldraw 生产许可门禁探测：LicenseGate 隐藏占位节点出现 → 画布即将被整体替换
  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const gatePresent = () => Boolean(host.querySelector(TLDR_LICENSE_GATE_SELECTOR));
    if (gatePresent()) {
      reportBlock({ kind: "license" });
      return;
    }
    const observer = new MutationObserver(() => {
      if (gatePresent()) {
        observer.disconnect();
        reportBlock({ kind: "license" });
      }
    });
    observer.observe(host, { childList: true, subtree: true });
    return () => observer.disconnect();
  }, [reportBlock]);

  // 意外关闭的重试：重挂载 Tldraw（旧实例卸载回调由 remountingRef 吞掉）
  const retryRemount = useCallback(() => {
    remountingRef.current = true;
    setBlocked(null);
    onBlockState(null);
    setRemountKey((key) => key + 1);
  }, [onBlockState]);

  return (
    <div className="ai-arch-canvas" ref={hostRef}>
      {blocked ? (
        <CanvasBlockedPanel
          info={blocked}
          onRetry={blocked.kind === "license" ? undefined : retryRemount}
        />
      ) : (
        <ErrorBoundary
          label="架构画布"
          fallback={(error, reset) => {
            // 渲染阶段同步记录（先于提交阶段的 editor 卸载回调），供下方
            // 卸载探测把本次关闭归类为「崩溃」而非「意外关闭」。
            boundaryErrorRef.current = error;
            return (
              <CanvasBlockedPanel
                info={{ kind: "crash", message: error.message, stack: error.stack }}
                onRetry={reset}
                onShown={onBlockState}
              />
            );
          }}
        >
          <Tldraw
            key={remountKey}
            persistenceKey={ARCHITECTURE_PERSISTENCE_KEY}
            assetUrls={tldrawAssetUrls}
            locale="zh-cn"
            // 生产许可证（tldraw SDK 商用生产需授权，开发免费）：构建时经
            // VITE_TLDRAW_LICENSE_KEY 注入；未配置时生产包中 editor 会在 ~5 秒后
            // 被许可门禁关闭（由上方 LicenseGate 探测展示原因，不再无声消失）。
            licenseKey={import.meta.env.VITE_TLDRAW_LICENSE_KEY || undefined}
            onMount={(editor) => {
              editorRef.current = editor;
              onEditor(editor);
              onBlockState(null);
              // 重试后新 editor 挂载成功：复位 retryRemount 的吞并标记与旧
              // 崩溃记录。crash/unexpected 阻断出现时旧 editor 的卸载回调早已
              // 执行，点重试时无人消费该标记——不复位会误吞新 editor 下一次
              // 真实的阻断上报（画布再次无声消失）。
              remountingRef.current = false;
              boundaryErrorRef.current = null;
              // onMount 可能晚于首个 effect，这里兜底应用初始主题
              applyTldrawColorScheme(editor, isDarkActive());
              return () => {
                editorRef.current = null;
                onEditor(null);
                // 兜底探测：视图未卸载而 editor 被销毁。正常视图切换时父组件同步
                // 卸载，这里的上报随组件销毁而失效，无副作用。
                if (remountingRef.current) {
                  remountingRef.current = false;
                  return;
                }
                const boundaryError = boundaryErrorRef.current;
                if (boundaryError) {
                  boundaryErrorRef.current = null;
                  reportBlock({
                    kind: "crash",
                    message: boundaryError.message,
                    stack: boundaryError.stack,
                  });
                  return;
                }
                const gated = hostRef.current?.querySelector(TLDR_LICENSE_GATE_SELECTOR);
                reportBlock(gated ? { kind: "license" } : { kind: "unexpected" });
              };
            }}
          />
        </ErrorBoundary>
      )}
    </div>
  );
});

export function ArchitectureView() {
  const editorRef = useRef<Editor | null>(null);
  /** 画布阻断原因：供执行监听器（architecture_run 回传）附加诊断上下文。 */
  const blockInfoRef = useRef<CanvasBlockInfo | null>(null);

  const handleEditor = useCallback((editor: Editor | null) => {
    editorRef.current = editor;
  }, []);
  const getEditor = useCallback(() => editorRef.current, []);
  const getBlockInfo = useCallback(() => blockInfoRef.current, []);
  // 阻断原因首写生效：门禁触发后伴随的卸载事件不得覆盖先到的原因；挂载成功清空
  const handleBlockState = useCallback((info: CanvasBlockInfo | null) => {
    if (info === null) {
      blockInfoRef.current = null;
      return;
    }
    blockInfoRef.current ??= info;
  }, []);

  const chat = useArchitectureChat({ getEditor });
  // 画布执行监听：architecture_run 工具 ↔ 前端解释器往返
  useArchRunListener(getEditor, getBlockInfo);

  const { effectiveWidth, handleResizeStart } = useDockedBrowserPanel(ARCH_CHAT_WIDTH_KEY, {
    minWidth: 320,
    defaultRatio: 0.28,
    maxRatio: 0.6,
  });

  const collapsed = chat.prefs.collapsed;

  return (
    <div className="ai-home-pane">
      <div className="ai-arch-shell">
        <ArchitectureCanvas onEditor={handleEditor} onBlockState={handleBlockState} />

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
              <ArchitectureChatPanel chat={chat} />
            </aside>
          </>
        )}
      </div>
    </div>
  );
}
