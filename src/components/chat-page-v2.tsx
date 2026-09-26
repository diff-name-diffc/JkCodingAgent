import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { DispatcherMessage, ImageSegment, McpStatus, OpenSettingsOptions } from "../types";
import { useToast } from "./Toast";
import { useDispatcherSessionTokenUsage } from "../hooks/useDispatcherSessionTokenUsage";
import {
  useDispatcherSessionRunning,
  useLiveSessionUpdater,
} from "./dispatcher-chat/useLiveSessionState";
import { useDispatcherActions } from "./dispatcher-chat/useDispatcherActions";
import { ChatShell } from "./chat/chat-shell";
import { resolveChatEmptyState } from "./chat/chat-empty-content";
import { CategoryPickerState } from "./chat/category-picker-state";
import type { ComposerMode } from "./chat/prompt-input";
import { PlainChatHeader, ProjectChatHeader } from "./chat-page-v2/ChatPageHeaders";
import { getUserMessagePayload } from "./chat-page-v2/message-utils";
import { useGraphPanelController } from "./chat-page-v2/useGraphPanelController";
import { usePythonRunController } from "./chat-page-v2/usePythonRunController";
import { useChatSessionController } from "./chat-page-v2/useChatSessionController";
import { useChatMessages } from "./chat-page-v2/useChatMessages";
import { ChatPageOverlays } from "./chat-page-v2/ChatPageOverlays";
import { useCurrentGitBranch } from "../hooks/use-current-git-branch";
import { useProjectSessionTitle } from "../hooks/use-project-session-title";

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
  /** 项目模式头部语境（UI-11）：会话标题查询与项目名展示。 */
  projectId?: string | null;
  projectName?: string | null;
  mcpStatus?: McpStatus | null;
  mcpChecking?: boolean;
  onOpenSettings: (options?: OpenSettingsOptions) => void;
  onOpenMcpStatus?: () => void;
  onClosePanel?: () => void;
  embedded?: boolean;
  /** 多项目保活挂载时隐藏工作区为 false；据此关闭分支轮询等常驻副作用
   * （执行图已迁主区标签，其保活门控由编辑 pane 的 active 承担，UI-13）。 */
  workspaceVisible?: boolean;
}

export function ChatPageV2({
  sessionId,
  onSessionChange,
  conversationKind = "chat",
  projectPath = "",
  projectId = null,
  projectName = null,
  mcpStatus = null,
  mcpChecking = false,
  onOpenSettings,
  onOpenMcpStatus,
  onClosePanel,
  embedded = false,
  workspaceVisible = true,
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

  // 领域化空态（UI-25 A06）：普通聊天与项目分别有贴合语境的起步提示，
  // 不再共用同一套通用空态文案。普通聊天挂上分类后进一步强化「当前
  // 分类决定系统提示词与工具」的提示。
  const activeCategory = isPlainChat ? chatSessions.activeCategory : null;
  const chatEmptyState = useMemo(() => {
    const base = resolveChatEmptyState(isPlainChat ? "plain" : "project");
    if (!isPlainChat || !activeCategory) return base;
    return {
      ...base,
      title: `「${activeCategory.name}」新对话`,
      copy: `本场对话运行在「${activeCategory.name}」分类下——已启用该分类的系统提示词与工具集，侧边栏与顶部徽标可随时确认所属分类。`,
    };
  }, [activeCategory, isPlainChat]);
  // 普通聊天未选会话：不再默认开新聊天（隐式落 "tech" 分类），改为分类
  // 选择空态；发送/贴图在无会话时给引导提示。
  const needsCategoryChoice = isPlainChat && !embedded && !activeSessionId;

  // ── Streaming pipeline (reused unchanged) ───────────────────────────────
  // 页面层只订阅「运行中」布尔（run 边界变化时才触发重渲染），完整流式
  // live state 由 MessageList 内部订阅——否则 token 流的每帧合帧通知会把
  // 整页（会话控制器/头部/输入区）拖进每帧重渲染。
  const updateLiveSessionState = useLiveSessionUpdater();
  const isSessionRunning = useDispatcherSessionRunning(activeSessionId);
  const { entries: sessionTokenUsageEntries, refresh: refreshSessionTokenUsage } =
    useDispatcherSessionTokenUsage(activeSessionId ?? "");

  const currentSessionIdRef = useRef<string | null>(activeSessionId);
  currentSessionIdRef.current = activeSessionId;
  // 上下文占用快照（容量回路闭环）：最近一次请求的 prompt 占用 / 窗口容量，
  // 取 primary 来源里最新的一条；头部指示器据此提示历史折叠行为。
  const contextUsage = useMemo(() => {
    const primary = sessionTokenUsageEntries.filter((entry) => entry.sourceKind === "primary");
    if (primary.length === 0) return null;
    const latest = primary.reduce((a, b) => (a.updatedAt > b.updatedAt ? a : b));
    if (latest.contextWindowTokens <= 0 || latest.contextWindowCapacity <= 0) return null;
    return {
      usedTokens: latest.contextWindowTokens,
      capacityTokens: latest.contextWindowCapacity,
    };
  }, [sessionTokenUsageEntries]);
  // 布局根节点 ref（UI-23a）：把输入框聚焦等 DOM 查询限定在本实例子树内——
  // 多项目保活时页面同时挂载多套聊天 DOM，全局选择器会命中隐藏工作区。
  const shellContainerRef = useRef<HTMLDivElement>(null);
  const pythonRuns = usePythonRunController(activeSessionId, currentSessionIdRef);
  const graphPanel = useGraphPanelController(activeSessionId, isPlainChat, currentSessionIdRef);
  // 头部任务语境（UI-11）：项目模式显示会话标题与当前分支；plain chat 传 null 关闭查询。
  const projectSessionTitle = useProjectSessionTitle(
    isPlainChat ? null : projectId,
    isPlainChat ? null : activeSessionId,
  );
  const currentBranch = useCurrentGitBranch(
    !isPlainChat && projectPath ? projectPath : null,
    !isPlainChat && workspaceVisible,
  );
  // 稳定引用：截断（regenerate / 编辑重发）后关闭旧画布并刷新「最近计划」入口。
  const { close: closeGraphPanel, refreshLatestPlan } = graphPanel;
  const shouldStickToBottomRef = useRef(true);

  const scrollMessageListToBottom = useCallback(() => {
    // 发送时强制回到底部：useAutoScroll 在用户上滚阅读时会停止跟随，
    // 发送新消息应把视图拉回最新内容（此时 shouldStickToBottomRef 已被置真）。
    // MessageList 的滚动容器是 [role="log"]——作用域化到本实例子树查询
    // （UI-24a-2）：多项目保活时 DOM 存在多个 [role="log"]（含架构面板复用
    // 实例），全局 querySelector 会滚动到隐藏项目的列表。
    if (shouldStickToBottomRef.current) {
      const el = shellContainerRef.current?.querySelector('[role="log"]');
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
    // 组件实例级在途标志随会话切换复位：它们描述的是上一个会话的操作
    //（编辑重发提交中 / 停止请求中），旧会话的 run 由 store 按会话键控
    // 继续推进，不能把新会话的输入区泄漏成转圈禁用。旧流程的 finally
    // 收尾仍会执行，对已复位的值无影响。
    setIsSubmittingEdit(false);
    setIsStopping(false);
  }, [activeSessionId, clearDraft]);

  // ── Composer mode (send / stop) ─────────────────────────────────────────
  const isRunning = isSessionRunning || isStopping;
  const composerMode: ComposerMode = isRunning ? "stop" : "send";

  const handleSend = useCallback(() => {
    const text = input.trim();
    if ((!text && attachedImages.length === 0) || isSubmittingEdit) return;
    void (async () => {
      // 项目模式总会话 id 非空（ProjectPage 仅在 activeSessionId 存在时渲染本组件）；
      // 聊天模式无会话时不再隐式创建（旧默认 "tech"），引导用户先选分类。
      if (!activeSessionId) {
        showToast("请先选择分类开始新对话：点击上方分类卡片，或用左侧分类行的 ＋。", "warning");
        return;
      }
      const targetSessionId = activeSessionId;

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
          // 截断已连带删除被删轮次的图计划：关闭旧画布并刷新「最近计划」入口。
          closeGraphPanel();
          refreshLatestPlan();
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
    closeGraphPanel,
    input,
    isSubmittingEdit,
    messages,
    refreshLatestPlan,
    setMessages,
    showToast,
  ]);

  // 暂存图片附件：FileReader 转 base64 → 后端统一落盘到
  // chat-images/{workspace_id}/ → push ImageSegment（只携带 imageId）。
  // 粘贴截图与回形针选图共用这一条管线；图片目录按会话 id 布局，无会话
  // 时不再隐式创建，先引导选择分类。
  const handleAttachImages = useCallback(
    (files: File[]) => {
      void (async () => {
        if (!activeSessionId) {
          showToast("请先选择分类开始新对话，再添加图片。", "warning");
          return;
        }
        const workspaceId = activeSessionId;
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
    [activeSessionId, showToast],
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
          // 截断已连带删除被删轮次的图计划：关闭旧画布并刷新「最近计划」入口。
          closeGraphPanel();
          refreshLatestPlan();
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
      closeGraphPanel,
      isRunning,
      isSubmittingEdit,
      messages,
      refreshLatestPlan,
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
        // 作用域化查询（UI-23a）：见 shellContainerRef 注释。
        const textarea = shellContainerRef.current?.querySelector<HTMLTextAreaElement>(
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
    // 与后端 fail-closed 守卫同口径：运行中的会话拒绝清空（后端也会拒绝，
    // 这里让用户免于先看到空结果、刷新后又冒出 run 中途写入的消息）。
    if (isRunning) {
      showToast("会话正在运行中，请先停止生成后再清空对话。", "warning");
      return;
    }
    try {
      await invoke("dispatcher_clear_messages", { workspaceId: activeSessionId });
    } catch (err) {
      console.error("清空对话失败:", err);
      showToast(String(err), "error");
      return;
    }
    clearDraft();
    setMessages([]);
    // 清空会删掉 token 用量行。占用指示只在运行事件里刷新，这里不拉一次就会
    // 继续显示清空前的百分比。
    await refreshSessionTokenUsage(activeSessionId);
  }, [activeSessionId, clearDraft, isRunning, refreshSessionTokenUsage, setMessages, showToast]);

  // 聊天模式（主页）也提供顶部栏：会话标题 + 运行状态 + 更多菜单，
  // 让宽屏下的消息区有视觉锚点；embedded（项目内嵌面板）下保持紧凑不加栏。
  const chatHeader = !isPlainChat ? (
    <ProjectChatHeader
      sessionTitle={projectSessionTitle}
      projectName={projectName}
      branchName={currentBranch}
      isLoading={isSessionRunning}
      isStopping={isStopping}
      hasMessages={messages.length > 0}
      mcpStatus={mcpStatus}
      mcpChecking={mcpChecking}
      contextUsage={contextUsage}
      graphAvailable={graphPanel.latestPlanId !== null}
      onOpenGraphPanel={graphPanel.open}
      onOpenMcpStatus={onOpenMcpStatus}
      onClearMessages={handleClearMessages}
      onOpenSettings={onOpenSettings}
      onClosePanel={onClosePanel}
    />
  ) : embedded ? undefined : (
    <PlainChatHeader
      title={needsCategoryChoice ? "开始新对话" : chatSessions.activeTitle}
      category={chatSessions.activeCategory}
      isLoading={isSessionRunning}
      isStopping={isStopping}
      hasMessages={messages.length > 0}
      mcpStatus={mcpStatus}
      mcpChecking={mcpChecking}
      contextUsage={contextUsage}
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
          enabled={workspaceVisible}
          containerRef={shellContainerRef}
          messages={messages}
          sessions={chatSessions.sessions}
          categories={chatSessions.categories}
          sessionsLoading={chatSessions.sessionsLoading}
          sessionsError={chatSessions.sessionsError}
          onSessionsRetry={chatSessions.retrySessions}
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
          isStopping={isStopping}
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
          emptyState={chatEmptyState}
          categoryPicker={
            isPlainChat && !embedded ? (
              <CategoryPickerState
                categories={chatSessions.categories}
                loading={chatSessions.categoriesLoading}
                onPickCategory={chatSessions.newSessionInCategory}
                onCreateCategory={chatSessions.createChatCategory}
              />
            ) : undefined
          }
          composerPlaceholder={needsCategoryChoice ? "先选择分类，开始新对话…" : undefined}
        />
      </div>
      <ChatPageOverlays
        pythonDrawerOpen={pythonRuns.drawerOpen}
        pythonTarget={pythonRuns.target}
        pythonRecord={pythonRuns.selectedRecord}
        pythonRunning={pythonRuns.selectedRunning}
        onClosePython={() => pythonRuns.setDrawerOpen(false)}
        onRunPython={pythonRuns.run}
        onStopPython={pythonRuns.stop}
        onClearPython={pythonRuns.clear}
      />
    </div>
  );
}
