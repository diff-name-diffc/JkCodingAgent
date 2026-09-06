import { useEffect, useRef, useState } from "react";
import type { KeyboardEvent } from "react";
import { RefreshCw } from "lucide-react";
import { isImeComposing } from "../../utils";

export interface BrowserAddressBarProps {
  /** 当前页面 URL（status.url，未连接时为空串）。 */
  url: string;
  connected: boolean;
  busy: boolean;
  hasSession: boolean;
  onReload: () => void;
  /** 提交导航（归一化与防重在命令层）。 */
  onNavigate: (url: string) => void;
}

/** 地址栏（UI-18 拆分）：显示态点击进入编辑；Enter/blur 提交，Escape 取消；IME 组合期不提交。 */
export function BrowserAddressBar({
  url,
  connected,
  busy,
  hasSession,
  onReload,
  onNavigate,
}: BrowserAddressBarProps) {
  const displayUrl = url || "about:blank";
  const [urlInput, setUrlInput] = useState(displayUrl);
  const [isEditingUrl, setIsEditingUrl] = useState(false);
  const urlInputRef = useRef<HTMLInputElement>(null);

  // 非编辑态时输入框内容跟随页面 URL
  useEffect(() => {
    if (!isEditingUrl) {
      setUrlInput(displayUrl);
    }
  }, [displayUrl, isEditingUrl]);

  const submit = () => {
    setIsEditingUrl(false);
    const value = urlInput.trim();
    if (!value || value === "about:blank") return;
    onNavigate(value);
  };

  // blur 仅在 URL 实际变更时提交（与拆分前一致）；Enter 后的连带 blur
  // 由命令层 submittingUrlRef 防重。
  const handleBlur = () => {
    const value = urlInput.trim();
    if (value && value !== displayUrl) {
      submit();
    } else {
      setIsEditingUrl(false);
    }
  };

  return (
    <div className="ai-browser-urlbar">
      <button
        type="button"
        title="刷新页面"
        onClick={onReload}
        disabled={!hasSession || !connected || busy}
        className="ai-browser-icon-button"
      >
        <RefreshCw size={13} />
      </button>
      <div className={isEditingUrl ? "ai-browser-address is-editing" : "ai-browser-address"}>
        {connected && url && url !== "about:blank" && url.startsWith("https://") && (
          <span className="ai-browser-lock">https</span>
        )}
        {isEditingUrl ? (
          <input
            ref={urlInputRef}
            type="text"
            value={urlInput}
            onChange={(e) => setUrlInput(e.target.value)}
            onKeyDown={(e: KeyboardEvent<HTMLInputElement>) => {
              if (isImeComposing(e)) return;
              if (e.key === "Enter") {
                e.preventDefault();
                submit();
              } else if (e.key === "Escape") {
                setIsEditingUrl(false);
                setUrlInput(displayUrl);
              }
            }}
            onBlur={handleBlur}
            autoFocus
            className="ai-browser-address-input"
          />
        ) : (
          <span
            onClick={() => {
              setIsEditingUrl(true);
              setUrlInput(url || "");
              setTimeout(() => urlInputRef.current?.select(), 0);
            }}
            className={
              url && url !== "about:blank"
                ? "ai-browser-address-text"
                : "ai-browser-address-text is-empty"
            }
            title={url}
          >
            {displayUrl}
          </span>
        )}
      </div>
    </div>
  );
}
