import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import type { DispatcherMessage, DispatcherMessageWire } from "../../types";
import { mergeDispatcherMessages } from "../dispatcher-chat/dispatcherChatUtils";
import { reconcileSessionMessages, subscribeDispatcherMessages } from "../dispatcherSessionStore";

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
    // dispatcher-session-updated 是会话记录事件（建会话 / 异步标题生成 /
    // 工作流执行回执落库）。其中工作流回执在 run Channel 之外持久化消息，监听它
    // 是回执即时进聊天流的唯一通道；复用 dispatcherSessionStore 的统一
    // 对账（而非私有 fetch+merge），自带「在途期间新 run 开启则丢弃过期
    // 全量」守卫，与 finished / failed / 断连释放路径同一条拉取管线。
    const unlisten = listen<{ id: string }>("dispatcher-session-updated", ({ payload }) => {
      if (payload.id !== activeSessionId) return;
      reconcileSessionMessages(activeSessionId);
    });
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, [activeSessionId]);

  return { messages, setMessages };
}
