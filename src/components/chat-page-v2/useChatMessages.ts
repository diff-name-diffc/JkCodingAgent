import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import type { DispatcherMessage, DispatcherMessageWire } from "../../types";
import { mergeDispatcherMessages } from "../dispatcher-chat/dispatcherChatUtils";
import { subscribeDispatcherMessages } from "../dispatcherSessionStore";

export function useChatMessages(
  activeSessionId: string | null,
  resetEditingMessage: (messageId: string | null) => void,
) {
  const [messages, setMessages] = useState<DispatcherMessage[]>([]);

  useEffect(() => {
    if (!activeSessionId) {
      setMessages([]);
      resetEditingMessage(null);
      return;
    }
    resetEditingMessage(null);
    let cancelled = false;
    setMessages([]);
    void invoke<DispatcherMessageWire[]>("dispatcher_list_messages", {
      workspaceId: activeSessionId,
    })
      .then((initial) => {
        if (!cancelled) setMessages(mergeDispatcherMessages([], initial));
      })
      .catch((error) => console.error("加载会话消息失败:", error));

    const unsubscribe = subscribeDispatcherMessages(activeSessionId, (incoming) => {
      setMessages((previous) => mergeDispatcherMessages(previous, incoming));
    });
    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, [activeSessionId, resetEditingMessage]);

  useEffect(() => {
    if (!activeSessionId) return;
    const unlisten = listen<{ id: string }>("dispatcher-session-updated", ({ payload }) => {
      if (payload.id !== activeSessionId) return;
      void invoke<DispatcherMessageWire[]>("dispatcher_list_messages", {
        workspaceId: activeSessionId,
      })
        .then((fresh) => setMessages((previous) => mergeDispatcherMessages(previous, fresh)))
        .catch((error) => console.error("刷新会话消息失败:", error));
    });
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, [activeSessionId]);

  return { messages, setMessages };
}
