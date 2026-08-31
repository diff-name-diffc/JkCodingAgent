import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { DispatcherMessage, ImageSegment, McpStatus } from "../types";
import { useToast } from "./Toast";
import { useDispatcherSessionTokenUsage } from "../hooks/useDispatcherSessionTokenUsage";
import { useLiveSessionState } from "./dispatcher-chat/useLiveSessionState";
import { useDispatcherActions } from "./dispatcher-chat/useDispatcherActions";
import { ChatShell } from "./chat/chat-shell";
import type { ComposerMode } from "./chat/prompt-input";
import { PlainChatHeader, ProjectChatHeader } from "./chat-page-v2/ChatPageHeaders";
import { getUserMessagePayload } from "./chat-page-v2/message-utils";
import { useGraphPanelController } from "./chat-page-v2/useGraphPanelController";
import { usePythonRunController } from "./chat-page-v2/usePythonRunController";
import { useChatSessionController } from "./chat-page-v2/useChatSessionController";
import { useChatMessages } from "./chat-page-v2/useChatMessages";
import { ChatPageOverlays } from "./chat-page-v2/ChatPageOverlays";

/** 读取文件为裸 base64（去掉 data URL 前缀）；读取失败返回 null。 */
function readFileAsBase64(file: File): Promise<string | null> {
  return new Promise((resolve) => {
    const reader = new FileReader();
    reader.onload = () => {
      const result = reader.result;
      resolve(typeof result === "string" ? (result.split(",")[1] ?? null) : null);
    };
    reader.onerror = () => resolve(null);
    reader.readAsDataURL(file);
  });
}

/** 截断消息前的图片存在性校验（regenerate / 编辑重发共用）：文件缺失时
 * 后端返回带 chat-image:// 引用清单的错误，直接抛出，调用方 toast 透传。 */
async function validateImagesBeforeTruncate(images: ImageSegment[]): Promise<void> {
  if (images.length === 0) return;
  await invoke("chat_images_validate", { segmentsJson: JSON.stringify(images) });
}

/** HomeChatPage 与 ProjectPage 共用的聊天领域组合入口。 */
export interface ChatPageV2Props {
  sessionId?: string | null;
  onSessionChange?: (sessionId: string | null) => void;
  conversationKind?: "project" | "chat";
  projectPath?: string;
  mcpStatus?: McpStatus | null;
  mcpChecking?: boolean;
  onOpenSettings: () => void;
  onOpenMcpStatus?: () => void;
  onClosePanel?: () => void;
  embedded?: boolean;
}

export function ChatPageV2({
  sessionId,
  onSessionChange,
  conversationKind = "chat",
  projectPath = "",
  mcpStatus = null,
  mcpChecking = false,
  onOpenSettings,
  onOpenMcpStatus,
  onClosePanel,
  embedded = false,
}: ChatPageV2Props) {
  const [uncontrolledSessionId, setUncontrolledSessionId] = useState<string | null>(null);
  const activeSessionId = sessionId !== undefined ? sessionId : uncontrolledSessionId;
  const setActiveSessionId = useCallback(
    (nextSessionId: string | null) => {
      if (sessionId === undefined) {
        setUncontrolledSessionId(nextSessionId);
      }
      onSessionChange?.(nextSessionId);
    },
    [onSessionChange, sessionId],
  );
  const [input, setInput] = useState("");
  const [attachedImages, setAttachedImages] = useState<ImageSegment[]>([]);
  const [editingMessageId, setEditingMessageId] = useState<string | null>(null);
  const [isSubmittingEdit, setIsSubmittingEdit] = useState(false);
  const { showToast } = useToast();
  const { messages, setMessages } = useChatMessages(activeSessionId, setEditingMessageId);
  const [isStopping, setIsStopping] = useState(false);

  const isPlainChat = conversationKind === "chat";
  const clearDraft = useCallback(() => {
    setInput("");
    setAttachedImages([]);
    setEditingMessageId(null);
  }, []);
  const resetConversation = useCallback(() => {
    clearDraft();
    setMessages([]);
  }, [clearDraft, setMessages]);
  const chatSessions = useChatSessionController({
    activeSessionId,
    isPlainChat,
    embedded,
    setActiveSessionId,
    resetConversation,
    onSessionChange,
  });

  // ── Streaming pipeline (reused unchanged) ───────────────────────────────
  const { liveState, updateLiveSessionState } = useLiveSessionState(activeSessionId ?? "");
  const { refresh: refreshSessionTokenUsage } = useDispatcherSessionTokenUsage(
    activeSessionId ?? "",
  );

  const currentSessionIdRef = useRef<string | null>(activeSessionId);
  currentSessionIdRef.current = activeSessionId;
  const pythonRuns = usePythonRunController(activeSessionId, currentSessionIdRef);
  const graphPanel = useGraphPanelController(activeSessionId, isPlainChat, currentSessionIdRef);
  const shouldStickToBottomRef = useRef(true);

  const scrollMessageListToBottom = useCallback(() => {
    // 发送时强制回到底部：useAutoScroll 在用户上滚阅读时会停止跟随，
    // 发送新消息应把视图拉回最新内容（此时 shouldStickToBottomRef 已被置真）。
    // MessageList 的滚动容器是 [role="log"]，直接滚动该元素即可。
    if (shouldStickToBottomRef.current) {
      const el = document.querySelector('[role="log"]');
      if (el) el.scrollTop = el.scrollHeight;
    }
  }, []);

  const actions = useDispatcherActions({
    sessionId: activeSessionId ?? "",
    projectPath,
    isPlainChat,
    updateLiveSessionState,
    scrollMessageListToBottom,
    currentSessionIdRef: currentSessionIdRef as React.RefObject<string>,
    refreshSessionTokenUsage,
    shouldStickToBottomRef,
    setInput,
    setAttachedImages,
  });

  // 会话切换时清空草稿（input / attachedImages / editingMessageId）。
  // 聊天模式经 handleActiveSessionChange 已显式重置；项目模式（embedded）切换
  // 会话只改上层 activeSessionId，不经过该回调，需要这里统一兜底。
  // ref 以挂载时的值初始化，首次挂载不清空，仅响应后续变化。
  const previousSessionIdRef = useRef(activeSessionId);
  useEffect(() => {
    if (previousSessionIdRef.current === activeSessionId) return;
    previousSessionIdRef.current = activeSessionId;
    clearDraft();
  }, [activeSessionId, clearDraft]);

  // ── Composer mode (send / stop) ─────────────────────────────────────────
  const isRunning = Boolean(liveState.hasPendingRun || liveState.isLoading || isStopping);
  const composerMode: ComposerMode = isRunning ? "stop" : "send";

  const handleSend = useCallback(() => {
    const text = input.trim();
    if ((!text && attachedImages.length === 0) || isSubmittingEdit) return;
    void (async () => {
      // 项目模式总会话 id 非空（ProjectPage 仅在 activeSessionId 存在时渲染本组件）；
      // 聊天模式可能为 null，懒创建后再发送。
      const targetSessionId = activeSessionId ?? (await chatSessions.ensureSession());
      if (!targetSessionId) return;

      if (editingMessageId) {
        const editIndex = messages.findIndex((message) => message.id === editingMessageId);
        if (editIndex === -1) {
          console.error(`编辑并重新发送失败：待编辑消息不存在：${editingMessageId}`);
          return;
        }
        setIsSubmittingEdit(true);
        try {
          // 先验后截断：图片失效时错误透传给用户，避免截断后发送失败丢消息。
          await validateImagesBeforeTruncate(attachedImages);
          await invoke("dispatcher_truncate_messages_from", {
            workspaceId: targetSessionId,
            messageId: editingMessageId,
          });
          setMessages((prev) => prev.slice(0, editIndex));
          setEditingMessageId(null);
          await actions.sendUserMessage(text, attachedImages, targetSessionId);
        } catch (err) {
          console.error("编辑并重新发送失败:", err);
          showToast(String(err), "error");
        } finally {
          setIsSubmittingEdit(false);
        }
        return;
      }

      await actions.sendUserMessage(text, attachedImages, targetSessionId);
    })();
  }, [
    actions,
    activeSessionId,
    attachedImages,
    editingMessageId,
    chatSessions,
    input,
    isSubmittingEdit,
    messages,
    setMessages,
    showToast,
  ]);

  // 暂存图片附件：FileReader 转 base64 → 后端统一落盘到
  // chat-images/{workspace_id}/ → push ImageSegment（只携带 imageId）。
  // 粘贴截图与回形针选图共用这一条管线；粘贴即确保会话存在（目录按会话 id 布局）。
  const handleAttachImages = useCallback(
    (files: File[]) => {
      void (async () => {
        const workspaceId = activeSessionId ?? (await chatSessions.ensureSession());
        if (!workspaceId) {
          showToast("图片保存失败：无法创建会话", "error");
          return;
        }
        for (const file of files) {
          const base64 = await readFileAsBase64(file);
          if (!base64) continue;
          try {
            const saved = await invoke<{ imageId: string; mimeType: string }>(
              "save_chat_image",
              {
                workspaceId,
                imageDataBase64: base64,
                mimeType: file.type || "image/png",
              },
            );
            setAttachedImages((prev) => [
              ...prev,
              {
                id: crypto.randomUUID(),
                type: "image",
                imageId: saved.imageId,
                source: "user_paste",
                mimeType: saved.mimeType,
              },
            ]);
          } catch (err) {
            console.error("保存图片失败:", err);
            showToast(`图片保存失败：${String(err)}`, "error");
          }
        }
      })();
    },
    [activeSessionId, chatSessions, showToast],
  );

  const handleRemoveAttachment = useCallback((id: string) => {
    setAttachedImages((prev) => prev.filter((image) => image.id !== id));
  }, []);

  const handleStop = useCallback(async () => {
    if (!activeSessionId || isStopping) return;
    setIsStopping(true);
    try {
      await invoke("dispatcher_stop_run", { workspaceId: activeSessionId });
    } catch (err) {
      console.error("停止生成失败:", err);
    } finally {
      setIsStopping(false);
    }
  }, [activeSessionId, isStopping]);

  // AI 回复下方的「重新生成」：绑定到该回复对应的用户消息，先截断再原样重发。
  const handleRegenerateFromMessage = useCallback(
    (message: DispatcherMessage) => {
      if (!activeSessionId || isRunning || isSubmittingEdit) return;

      const { text, images } = getUserMessagePayload(message);
      if (!text && images.length === 0) return;
      const messageIndex = messages.findIndex((item) => item.id === message.id);
      if (messageIndex === -1) {
        console.error(`重新生成失败：源用户消息不存在：${message.id}`);
        return;
      }

      void (async () => {
        setIsSubmittingEdit(true);
        try {
          // 先验后截断：图片失效时错误透传给用户，避免截断后发送失败丢消息。
          await validateImagesBeforeTruncate(images);
          await invoke("dispatcher_truncate_messages_from", {
            workspaceId: activeSessionId,
            messageId: message.id,
          });
          setMessages((prev) => prev.slice(0, messageIndex));
          await actions.sendUserMessage(text, images, activeSessionId);
        } catch (err) {
          console.error("重新生成失败:", err);
          showToast(String(err), "error");
        } finally {
          setIsSubmittingEdit(false);
        }
      })();
    },
    [
      actions,
      activeSessionId,
      isRunning,
      isSubmittingEdit,
      messages,
      setMessages,
      showToast,
    ],
  );

  const handleEditMessage = useCallback(
    (message: DispatcherMessage) => {
      if (isRunning || isSubmittingEdit) return;
      const { text, images } = getUserMessagePayload(message);
      if (!text && images.length === 0) return;

      setEditingMessageId(message.id);
      setInput(text);
      setAttachedImages(images);
      window.requestAnimationFrame(() => {
        const textarea = document.querySelector<HTMLTextAreaElement>(
          'textarea[aria-label="消息输入框"]',
        );
        textarea?.focus();
        textarea?.setSelectionRange(text.length, text.length);
      });
    },
    [isRunning, isSubmittingEdit],
  );

  const handleCancelEdit = useCallback(() => {
    if (isSubmittingEdit) return;
    clearDraft();
  }, [clearDraft, isSubmittingEdit]);

  const handleClearMessages = useCallback(async () => {
    if (!activeSessionId) return;
    await invoke("dispatcher_clear_messages", { workspaceId: activeSessionId });
    clearDraft();
    setMessages([]);
  }, [activeSessionId, clearDraft, setMessages]);

  // 聊天模式（主页）也提供顶部栏：会话标题 + 运行状态 + 清空/设置，
  // 让宽屏下的消息区有视觉锚点；embedded（项目内嵌面板）下保持紧凑不加栏。
  const chatHeader = !isPlainChat ? (
    <ProjectChatHeader
      isLoading={liveState.isLoading || liveState.hasPendingRun}
      hasMessages={messages.length > 0}
      mcpStatus={mcpStatus}
      mcpChecking={mcpChecking}
      graphAvailable={graphPanel.latestPlanId !== null}
      onOpenGraphPanel={graphPanel.open}
      onOpenMcpStatus={onOpenMcpStatus}
      onClearMessages={handleClearMessages}
      onOpenSettings={onOpenSettings}
      onClosePanel={onClosePanel}
    />
  ) : embedded ? undefined : (
    <PlainChatHeader
      title={chatSessions.activeTitle}
      isLoading={liveState.isLoading || liveState.hasPendingRun}
      hasMessages={messages.length > 0}
      mcpStatus={mcpStatus}
      mcpChecking={mcpChecking}
      onOpenMcpStatus={onOpenMcpStatus}
      onClearMessages={handleClearMessages}
      onOpenSettings={onOpenSettings}
    />
  );

  return (
    <div className="flex h-full w-full min-w-0 overflow-hidden">
      <div className="min-w-0 flex-1">
        <ChatShell
          sessionId={activeSessionId}
          messages={messages}
          sessions={chatSessions.sessions}
          categories={chatSessions.categories}
          sessionsLoading={chatSessions.sessionsLoading}
          sessionsError={chatSessions.sessionsError}
          searchActive={chatSessions.searchActive}
          onActiveSessionChange={chatSessions.selectSession}
          onNewConversation={chatSessions.newConversation}
          onNewSessionInCategory={
            isPlainChat && !embedded ? chatSessions.newSessionInCategory : undefined
          }
          onDeleteSession={isPlainChat && !embedded ? chatSessions.deleteChatSession : undefined}
          searchValue={chatSessions.search}
          onSearchChange={chatSessions.setSearch}
          onOpenSettings={onOpenSettings}
          onCreateCategory={isPlainChat && !embedded ? chatSessions.createChatCategory : undefined}
          onRenameCategory={isPlainChat && !embedded ? chatSessions.renameChatCategory : undefined}
          onDeleteCategory={isPlainChat && !embedded ? chatSessions.deleteChatCategory : undefined}
          onMoveSessionToCategory={isPlainChat && !embedded ? chatSessions.moveSession : undefined}
          input={input}
          onInputChange={setInput}
          composerMode={composerMode}
          onSend={handleSend}
          onStop={handleStop}
          attachments={attachedImages}
          onAttachImages={handleAttachImages}
          onRemoveAttachment={handleRemoveAttachment}
          onRegenerateFromMessage={handleRegenerateFromMessage}
          onEditMessage={handleEditMessage}
          editingMessageId={editingMessageId}
          onCancelEdit={handleCancelEdit}
          composerDisabled={isSubmittingEdit}
          pythonRunRecords={pythonRuns.records}
          onRunPython={pythonRuns.run}
          embedded={embedded}
          projectHeader={chatHeader}
        />
      </div>
      <ChatPageOverlays
        graphPlanId={graphPanel.planId}
        pythonDrawerOpen={pythonRuns.drawerOpen}
        pythonTarget={pythonRuns.target}
        pythonRecord={pythonRuns.selectedRecord}
        pythonRunning={pythonRuns.selectedRunning}
        onCloseGraph={graphPanel.close}
        onClosePython={() => pythonRuns.setDrawerOpen(false)}
        onRunPython={pythonRuns.run}
        onStopPython={pythonRuns.stop}
        onClearPython={pythonRuns.clear}
      />
    </div>
  );
}
