import type { MouseEvent } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useMarkdownLinkHandler } from "./MarkdownLinkContext";

function isBrowserUrl(url: string | undefined): url is string {
  if (!url) return false;
  try {
    const parsed = new URL(url);
    return parsed.protocol === "http:" || parsed.protocol === "https:";
  } catch {
    return false;
  }
}

/**
 * 两条 markdown 管线共用的链接组件：http(s) 链接优先交给
 * MarkdownLinkProvider 注入的内嵌浏览器 handler；未包 provider 的场景
 * （如架构助手）回退系统浏览器，避免在应用 webview 内导航。
 */
export function MarkdownLink({
  href,
  children,
  ...props
}: React.AnchorHTMLAttributes<HTMLAnchorElement>) {
  const openMarkdownLink = useMarkdownLinkHandler();
  const handleClick = (event: MouseEvent<HTMLAnchorElement>) => {
    if (!isBrowserUrl(href)) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    if (openMarkdownLink) {
      void openMarkdownLink(href);
    } else {
      void openUrl(href);
    }
  };

  return (
    <a {...props} href={href} rel="noreferrer" onClick={handleClick}>
      {children}
    </a>
  );
}
