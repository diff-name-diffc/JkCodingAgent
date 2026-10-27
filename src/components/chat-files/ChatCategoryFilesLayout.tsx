import { lazy, Suspense } from "react";
import { useChatFilesStore } from "../../stores/chat-files-store";
import { AppLayout, type AppLayoutProps } from "../layout/app-layout";

const ChatCategoryFilesOpenLayout = lazy(() =>
  import("./ChatCategoryFilesOpenLayout").then((module) => ({
    default: module.ChatCategoryFilesOpenLayout,
  })),
);

export interface ChatCategoryFilesLayoutProps extends AppLayoutProps {
  chatFilesEnabled?: boolean;
}

export function ChatCategoryFilesLayout({
  chatFilesEnabled = false,
  ...layoutProps
}: ChatCategoryFilesLayoutProps) {
  const target = useChatFilesStore((state) => state.target);

  if (!chatFilesEnabled || !target) {
    return <AppLayout {...layoutProps} />;
  }

  return (
    <Suspense fallback={<AppLayout {...layoutProps} />}>
      <ChatCategoryFilesOpenLayout
        key={`${target.categoryId}:${target.sessionId}`}
        categoryId={target.categoryId}
        sessionId={target.sessionId}
        {...layoutProps}
      />
    </Suspense>
  );
}
