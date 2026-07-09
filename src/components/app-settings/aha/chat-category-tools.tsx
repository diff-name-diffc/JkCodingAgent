import { RefreshCw } from "lucide-react";
import type { ChatCategoryAgentConfig } from "../../../types";
import { ToolsTab } from "./tools-tab";
import { SubAgentPicker } from "./sub-agent-picker";

export function ChatCategoryToolsTab({
  configs,
  activeCategoryId,
  onActiveCategoryChange,
  onReload,
  onChange,
}: {
  configs: ChatCategoryAgentConfig[];
  activeCategoryId: string | null;
  onActiveCategoryChange: (categoryId: string) => void;
  onReload: () => void;
  onChange: (categoryId: string, patch: Partial<ChatCategoryAgentConfig>) => void;
}) {
  const activeConfig =
    configs.find((config) => config.categoryId === activeCategoryId) ?? configs[0] ?? null;

  if (!activeConfig) {
    return (
      <section className="ai-aha-section">
        <div className="ai-aha-section-header">
          <div>
            <div className="ai-aha-section-title">分类工具配置</div>
            <div className="ai-aha-section-description">
              暂无聊天分类。创建分类后会自动生成配置。
            </div>
          </div>
          <button type="button" className="ai-aha-ghost-button" onClick={onReload}>
            <RefreshCw size={13} />
            刷新分类
          </button>
        </div>
      </section>
    );
  }

  return (
    <>
      <section className="ai-aha-section">
        <div className="ai-aha-section-header">
          <div>
            <div className="ai-aha-section-title">分类</div>
            <div className="ai-aha-section-description">
              每个聊天分类都有独立的工具集和系统提示词；新分类默认复制聊天智能体默认配置。
            </div>
          </div>
          <button type="button" className="ai-aha-ghost-button" onClick={onReload}>
            <RefreshCw size={13} />
            刷新分类
          </button>
        </div>
        <div className="ai-aha-category-chips">
          {configs.map((config) => {
            const selected = config.categoryId === activeConfig.categoryId;
            return (
              <button
                key={config.categoryId}
                type="button"
                className={
                  selected ? "ai-aha-category-chip is-active" : "ai-aha-category-chip"
                }
                onClick={() => onActiveCategoryChange(config.categoryId)}
              >
                {config.categoryName}
              </button>
            );
          })}
        </div>
      </section>

      <section className="ai-aha-section">
        <div className="ai-aha-section-title">{activeConfig.categoryName} · 系统提示词</div>
        <div className="ai-aha-section-description">
          该提示词只影响当前分类下的普通聊天会话；运行时会追加分类名称、分类 ID 和系统时间。
        </div>
        <label className="ai-aha-field">
          <span className="ai-aha-field-label">系统提示词</span>
          <textarea
            className="ai-settings-textarea"
            style={{ minHeight: 220 }}
            value={activeConfig.systemPrompt}
            onChange={(event) =>
              onChange(activeConfig.categoryId, { systemPrompt: event.target.value })
            }
            spellCheck={false}
          />
        </label>
      </section>

      <ToolsTab
        context="chat"
        allowedTools={activeConfig.allowedTools}
        onChange={(next) => onChange(activeConfig.categoryId, { allowedTools: next })}
      />
      <SubAgentPicker
        enabledIds={activeConfig.subAgentIds}
        onChange={(next) => onChange(activeConfig.categoryId, { subAgentIds: next })}
      />
    </>
  );
}
