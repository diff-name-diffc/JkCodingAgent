/**
 * 架构助手轻量聊天面板：头部（标题/模型切换/新会话/折叠）+ 感知开关
 * + 消息流（复用 MessageList）+ 极简输入区。
 *
 * 模型下拉数据源 = 设置中心「视觉」分类的启用库条目，默认高亮视觉用途
 * 绑定条目；选择仅面板内持久化（不回写全局绑定）。
 */

import { useState } from "react";
import { Bot, Eye, LayoutList, PanelRightClose, Plus, Send, Square } from "lucide-react";
import { cn } from "../../../lib/cn";
import { isImeComposing } from "../../../utils";
import { useAhaSettingsStore } from "../../settings/use-aha-settings";
import {
  entriesForCategory,
  entryLabel,
  findEnabledEntryForConfig,
} from "../../settings/providers/model-library";
import { getPurposeBinding } from "../../settings/providers/provider-registry";
import { ModelSelector } from "../../chat/model-selector";
import { MessageList } from "../../chat/message-list";
import type { UseArchitectureChatResult } from "./useArchitectureChat";

function PerceptionToggle({
  icon,
  label,
  active,
  onClick,
}: {
  icon: React.ReactNode;
  label: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className={cn("ai-arch-perception-toggle", active && "is-active")}
      onClick={onClick}
      title={`${label}（${active ? "已开启" : "已关闭"}）`}
      aria-pressed={active}
    >
      {icon}
      <span>{label}</span>
    </button>
  );
}

export function ArchitectureChatPanel({ chat }: { chat: UseArchitectureChatResult }) {
  const [input, setInput] = useState("");
  const settingsStore = useAhaSettingsStore();
  const settings = settingsStore.settings;

  const visionEntries = settings
    ? entriesForCategory(settings.modelLibrary, "vision", { enabledOnly: true })
    : [];
  const visionBinding = settings ? getPurposeBinding(settings, "vision") : null;
  const defaultEntry = settings
    ? findEnabledEntryForConfig(settings.modelLibrary, visionBinding)
    : undefined;
  const activeEntryId = chat.prefs.modelLibraryId ?? defaultEntry?.id;
  const activeEntry =
    visionEntries.find((entry) => entry.id === activeEntryId) ?? defaultEntry;

  const handleSend = () => {
    const text = input.trim();
    if (!text || chat.isRunning) return;
    setInput("");
    void chat.send(text).then((sent) => {
      // 发送失败（如会话创建失败）：恢复已输入文本，错误经 chat.sendError 展示。
      if (!sent) setInput(text);
    });
  };

  const handleKeyDown = (event: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === "Enter" && !event.shiftKey && !isImeComposing(event)) {
      event.preventDefault();
      handleSend();
    }
  };

  return (
    <div className="ai-arch-chat-panel">
      <header className="ai-arch-chat-header">
        <span className="ai-arch-chat-title">
          <Bot size={14} strokeWidth={2} />
          架构助手
        </span>
        <div className="ai-arch-chat-header-actions">
          <ModelSelector
            models={visionEntries}
            activeEntryId={activeEntryId}
            activeLabel={activeEntry ? entryLabel(activeEntry) : undefined}
            menuLabel="视觉模型"
            onSelect={(entryId) => chat.updatePrefs({ modelLibraryId: entryId })}
            className="ai-arch-model-selector"
          />
          <button
            type="button"
            className="ai-arch-icon-btn"
            onClick={chat.newConversation}
            disabled={chat.isRunning}
            title="新对话"
          >
            <Plus size={14} strokeWidth={2} />
          </button>
          <button
            type="button"
            className="ai-arch-icon-btn"
            onClick={() => chat.updatePrefs({ collapsed: true })}
            title="折叠面板"
          >
            <PanelRightClose size={14} strokeWidth={2} />
          </button>
        </div>
      </header>

      <div className="ai-arch-chat-toggles">
        <PerceptionToggle
          icon={<Eye size={12} strokeWidth={2} />}
          label="附截图"
          active={chat.prefs.attachScreenshot}
          onClick={() => chat.updatePrefs({ attachScreenshot: !chat.prefs.attachScreenshot })}
        />
        <PerceptionToggle
          icon={<LayoutList size={12} strokeWidth={2} />}
          label="附快照"
          active={chat.prefs.attachSnapshot}
          onClick={() => chat.updatePrefs({ attachSnapshot: !chat.prefs.attachSnapshot })}
        />
      </div>

      <MessageList
        sessionId={chat.sessionId}
        messages={chat.messages}
        liveState={chat.liveState}
        onPickPrompt={(prompt) => setInput(prompt)}
        className="ai-arch-chat-messages"
      />

      {chat.sendError && (
        <div className="ai-arch-chat-error" role="alert">
          {chat.sendError}
        </div>
      )}

      <div className="ai-arch-chat-input-row">
        <textarea
          className="ai-arch-chat-input"
          rows={2}
          value={input}
          placeholder="描述要绘制的架构图…（Enter 发送，Shift+Enter 换行）"
          onChange={(event) => setInput(event.target.value)}
          onKeyDown={handleKeyDown}
        />
        {chat.isRunning ? (
          <button
            type="button"
            className="ai-arch-send-btn is-stop"
            onClick={() => void chat.stop()}
            title="停止本轮"
          >
            <Square size={13} strokeWidth={2.2} />
          </button>
        ) : (
          <button
            type="button"
            className="ai-arch-send-btn"
            onClick={handleSend}
            disabled={!input.trim()}
            title="发送"
          >
            <Send size={13} strokeWidth={2.2} />
          </button>
        )}
      </div>
    </div>
  );
}
