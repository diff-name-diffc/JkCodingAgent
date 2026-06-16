import { createContext, useContext } from "react";
import type { ReactNode } from "react";

type MarkdownLinkHandler = (url: string) => void | Promise<void>;

const MarkdownLinkContext = createContext<MarkdownLinkHandler | null>(null);

export function MarkdownLinkProvider({
  onOpenUrl,
  children,
}: {
  onOpenUrl: MarkdownLinkHandler;
  children: ReactNode;
}) {
  return (
    <MarkdownLinkContext.Provider value={onOpenUrl}>
      {children}
    </MarkdownLinkContext.Provider>
  );
}

export function useMarkdownLinkHandler() {
  return useContext(MarkdownLinkContext);
}
