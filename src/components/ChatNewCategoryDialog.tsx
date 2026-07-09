import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ChevronDown, RefreshCw, X } from "lucide-react";
import type { AgentToolInfo } from "../types";

export interface ChatCategoryCreateConfig {
  systemPrompt?: string;
  allowedTools?: string[];
}

interface ChatNewCategoryDialogProps {
  open: boolean;
  initialName: string;
  onSubmit: (name: string, config?: ChatCategoryCreateConfig) => void;
  onClose: () => void;
  title: string;
  confirmLabel: string;
  showAgentConfig?: boolean;
}

export function ChatNewCategoryDialog({
  open,
  initialName,
  onSubmit,
  onClose,
  title,
  confirmLabel,
  showAgentConfig = false,
}: ChatNewCategoryDialogProps) {
  const [name, setName] = useState(initialName);
  const [systemPrompt, setSystemPrompt] = useState("");
  const [customTools, setCustomTools] = useState(false);
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [availableTools, setAvailableTools] = useState<AgentToolInfo[]>([]);
  const [selectedTools, setSelectedTools] = useState<string[]>([]);
  const [loadingTools, setLoadingTools] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (open) {
      setName(initialName);
      setSystemPrompt("");
      setCustomTools(false);
      setShowAdvanced(false);
      setSelectedTools([]);
      setTimeout(() => inputRef.current?.focus(), 50);
    }
  }, [open, initialName]);

  const loadTools = useCallback(async () => {
    if (!showAgentConfig) return;
    setLoadingTools(true);
    setLoadError(null);
    try {
      const tools = await invoke<AgentToolInfo[]>("aha_list_agent_tools", {
        context: "chat",
        projectPath: null,
      });
      setAvailableTools(tools);
    } catch (error) {
      setLoadError(String(error));
    } finally {
      setLoadingTools(false);
    }
  }, [showAgentConfig]);

  useEffect(() => {
    if (open && showAgentConfig) {
      loadTools();
    }
  }, [loadTools, open, showAgentConfig]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open) return null;

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = name.trim();
    if (!trimmed) return;
    const config: ChatCategoryCreateConfig = {};
    const prompt = systemPrompt.trim();
    if (prompt) config.systemPrompt = prompt;
    if (customTools) config.allowedTools = selectedTools;
    onSubmit(trimmed, Object.keys(config).length > 0 ? config : undefined);
  };

  function toggleTool(name: string) {
    setSelectedTools((prev) =>
      prev.includes(name) ? prev.filter((tool) => tool !== name) : [...prev, name],
    );
  }

  return (
    <div
      className="ai-dialog-overlay"
      onClick={onClose}
    >
      <form
        onSubmit={handleSubmit}
        onClick={(e) => e.stopPropagation()}
        className={showAgentConfig ? "ai-dialog ai-category-dialog ai-category-dialog-wide" : "ai-dialog ai-category-dialog"}
      >
        <button
          type="button"
          onClick={onClose}
          className="ai-dialog-close"
        >
          <X size={14} />
        </button>
        <div className="ai-dialog-title">
          {title}
        </div>
        <input
          ref={inputRef}
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="输入分类名称"
          className="ai-field ai-dialog-input"
        />
        {showAgentConfig && (
          <div className="ai-category-advanced">
            <button
              type="button"
              onClick={() => setShowAdvanced((value) => !value)}
              className="ai-category-advanced-toggle"
            >
              <span>提示词与工具集合</span>
              <ChevronDown
                size={14}
                className={showAdvanced ? "ai-rotate-180" : undefined}
              />
            </button>
            {showAdvanced && (
              <>
                <label className="ai-field-stack">
                  <span className="ai-field-label">系统提示词</span>
                  <textarea
                    value={systemPrompt}
                    onChange={(event) => setSystemPrompt(event.target.value)}
                    placeholder="留空时按分类场景自动初始化提示词"
                    spellCheck={false}
                    className="ai-field ai-dialog-textarea"
                  />
                </label>
                <div className="ai-category-tools">
                  <label className="ai-check-row">
                    <input
                      type="checkbox"
                      checked={customTools}
                      onChange={(event) => setCustomTools(event.target.checked)}
                    />
                    自定义工具集合
                  </label>
                  <div className="ai-category-tools-meta">
                    <span>
                      {customTools
                        ? `已选 ${selectedTools.length} / ${availableTools.length}`
                        : "未启用自定义时，后端按分类场景初始化工具集合"}
                      {loadError ? ` · ${loadError}` : ""}
                    </span>
                    <button
                      type="button"
                      onClick={loadTools}
                      disabled={loadingTools}
                      className="ai-secondary-button"
                    >
                      <RefreshCw size={12} className={loadingTools ? "spin" : undefined} />
                      刷新工具
                    </button>
                  </div>
                  {customTools && (
                    <div className="ai-tool-grid chat-scroll">
                      {availableTools.map((tool) => (
                        <label
                          key={tool.name}
                          title={tool.description}
                          className={
                            selectedTools.includes(tool.name)
                              ? "ai-tool-option is-selected"
                              : "ai-tool-option"
                          }
                        >
                          <input
                            type="checkbox"
                            checked={selectedTools.includes(tool.name)}
                            onChange={() => toggleTool(tool.name)}
                          />
                          <span>
                            {tool.name}
                          </span>
                        </label>
                      ))}
                    </div>
                  )}
                </div>
              </>
            )}
          </div>
        )}
        <div className="ai-dialog-actions">
          <button
            type="button"
            onClick={onClose}
            className="ai-secondary-button"
          >
            取消
          </button>
          <button
            type="submit"
            disabled={!name.trim()}
            className="ai-primary-button"
          >
            {confirmLabel}
          </button>
        </div>
      </form>
    </div>
  );
}
