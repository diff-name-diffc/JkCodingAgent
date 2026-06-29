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
      style={{
        position: "fixed",
        inset: 0,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        background: "rgba(0,0,0,0.45)",
        zIndex: 100,
        animation: "fadeIn 0.12s ease-out",
      }}
      onClick={onClose}
    >
      <form
        onSubmit={handleSubmit}
        onClick={(e) => e.stopPropagation()}
        style={{
          width: showAgentConfig ? 560 : 340,
          maxHeight: "86vh",
          overflowY: "auto",
          padding: "20px 22px 18px",
          background: "var(--bg-card)",
          border: "1px solid var(--border-medium)",
          borderRadius: 12,
          boxShadow: "0 12px 40px rgba(0,0,0,0.3)",
          position: "relative",
        }}
      >
        <button
          type="button"
          onClick={onClose}
          style={{
            position: "absolute",
            top: 10,
            right: 10,
            background: "none",
            border: "none",
            cursor: "pointer",
            padding: 2,
            color: "var(--text-muted)",
          }}
        >
          <X size={14} />
        </button>
        <div style={{ fontSize: 14, fontWeight: 650, marginBottom: 14, color: "var(--text-primary)" }}>
          {title}
        </div>
        <input
          ref={inputRef}
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="输入分类名称"
          style={{
            width: "100%",
            padding: "9px 12px",
            border: "1px solid var(--border-medium)",
            borderRadius: 8,
            background: "var(--bg-input)",
            color: "var(--text-primary)",
            fontSize: 13,
            outline: "none",
            boxSizing: "border-box",
            marginBottom: 16,
          }}
        />
        {showAgentConfig && (
          <div style={{ display: "flex", flexDirection: "column", gap: 12, marginBottom: 16 }}>
            <button
              type="button"
              onClick={() => setShowAdvanced((value) => !value)}
              style={{
                display: "flex",
                alignItems: "center",
                justifyContent: "space-between",
                gap: 8,
                width: "100%",
                border: "1px solid var(--border-dim)",
                background: "var(--bg-subtle)",
                color: "var(--text-primary)",
                borderRadius: 8,
                padding: "8px 10px",
                cursor: "pointer",
                fontSize: 12.5,
              }}
            >
              <span>提示词与工具集合</span>
              <ChevronDown
                size={14}
                style={{
                  transform: showAdvanced ? "rotate(180deg)" : "none",
                  transition: "transform 0.15s",
                }}
              />
            </button>
            {showAdvanced && (
              <>
                <label style={{ display: "flex", flexDirection: "column", gap: 6 }}>
                  <span style={{ fontSize: 12, color: "var(--text-secondary)" }}>
                    系统提示词
                  </span>
                  <textarea
                    value={systemPrompt}
                    onChange={(event) => setSystemPrompt(event.target.value)}
                    placeholder="留空时按分类场景自动初始化提示词"
                    spellCheck={false}
                    style={{
                      width: "100%",
                      minHeight: 150,
                      resize: "vertical",
                      padding: "9px 12px",
                      border: "1px solid var(--border-medium)",
                      borderRadius: 8,
                      background: "var(--bg-input)",
                      color: "var(--text-primary)",
                      fontSize: 12.5,
                      lineHeight: 1.5,
                      outline: "none",
                      boxSizing: "border-box",
                    }}
                  />
                </label>
                <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
                  <label
                    style={{
                      display: "flex",
                      alignItems: "center",
                      gap: 8,
                      fontSize: 12.5,
                      color: "var(--text-primary)",
                    }}
                  >
                    <input
                      type="checkbox"
                      checked={customTools}
                      onChange={(event) => setCustomTools(event.target.checked)}
                    />
                    自定义工具集合
                  </label>
                  <div
                    style={{
                      display: "flex",
                      alignItems: "center",
                      justifyContent: "space-between",
                      gap: 8,
                    }}
                  >
                    <span style={{ fontSize: 11.5, color: "var(--text-hint)" }}>
                      {customTools
                        ? `已选 ${selectedTools.length} / ${availableTools.length}`
                        : "未启用自定义时，后端按分类场景初始化工具集合"}
                      {loadError ? ` · ${loadError}` : ""}
                    </span>
                    <button
                      type="button"
                      onClick={loadTools}
                      disabled={loadingTools}
                      style={{
                        display: "inline-flex",
                        alignItems: "center",
                        gap: 5,
                        padding: "5px 8px",
                        border: "1px solid var(--border-dim)",
                        borderRadius: 6,
                        background: "var(--bg-subtle)",
                        color: "var(--text-secondary)",
                        cursor: loadingTools ? "not-allowed" : "pointer",
                        fontSize: 11.5,
                        opacity: loadingTools ? 0.65 : 1,
                      }}
                    >
                      <RefreshCw size={12} className={loadingTools ? "spin" : undefined} />
                      刷新工具
                    </button>
                  </div>
                  {customTools && (
                    <div
                      style={{
                        display: "grid",
                        gridTemplateColumns: "1fr 1fr",
                        gap: 6,
                        maxHeight: 220,
                        overflowY: "auto",
                        border: "1px solid var(--border-dim)",
                        borderRadius: 8,
                        padding: 8,
                      }}
                    >
                      {availableTools.map((tool) => (
                        <label
                          key={tool.name}
                          title={tool.description}
                          style={{
                            display: "flex",
                            alignItems: "center",
                            gap: 6,
                            minWidth: 0,
                            padding: "5px 6px",
                            borderRadius: 6,
                            background: selectedTools.includes(tool.name)
                              ? "var(--accent-subtle)"
                              : "transparent",
                            cursor: "pointer",
                          }}
                        >
                          <input
                            type="checkbox"
                            checked={selectedTools.includes(tool.name)}
                            onChange={() => toggleTool(tool.name)}
                          />
                          <span
                            style={{
                              fontSize: 11.5,
                              color: "var(--text-primary)",
                              fontFamily: "var(--font-mono)",
                              overflow: "hidden",
                              textOverflow: "ellipsis",
                              whiteSpace: "nowrap",
                            }}
                          >
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
        <div style={{ display: "flex", justifyContent: "flex-end", gap: 8 }}>
          <button
            type="button"
            onClick={onClose}
            style={{
              padding: "7px 14px",
              background: "var(--bg-subtle)",
              border: "1px solid var(--border-medium)",
              borderRadius: 7,
              fontSize: 12.5,
              color: "var(--text-secondary)",
              cursor: "pointer",
            }}
          >
            取消
          </button>
          <button
            type="submit"
            disabled={!name.trim()}
            style={{
              padding: "7px 16px",
              background: "var(--accent)",
              border: "none",
              borderRadius: 7,
              fontSize: 12.5,
              color: "white",
              fontWeight: 600,
              cursor: name.trim() ? "pointer" : "not-allowed",
              opacity: name.trim() ? 1 : 0.5,
            }}
          >
            {confirmLabel}
          </button>
        </div>
      </form>
    </div>
  );
}
