import { useAhaSettings } from "./use-aha-settings";
import { Section } from "./Section";
import { ToolsTab } from "../app-settings/aha/tools-tab";
import { ChatCategoryToolsTab } from "../app-settings/aha/chat-category-tools";

/** 「工具」页：项目智能体可用工具 + 聊天分类工具配置（变更走自动保存管线）。 */
export function ToolsPage({ projectPath }: { projectPath?: string }) {
  const store = useAhaSettings();

  if (store.loading || !store.settings) {
    return <div className="ai-settings-empty">加载中...</div>;
  }

  return (
    <div className="ai-set-page">
      <Section title="项目智能体工具" description="项目会话中允许智能体调用的工具。">
        <ToolsTab
          context="project"
          projectPath={projectPath}
          allowedTools={store.settings.project.allowedTools}
          onChange={(next) =>
            store.updateSettings((prev) => ({
              ...prev,
              project: { ...prev.project, allowedTools: next },
            }))
          }
        />
      </Section>
      <Section title="聊天分类工具" description="按聊天分类配置可用工具与子智能体。">
        <ChatCategoryToolsTab
          configs={store.chatCategoryConfigs}
          activeCategoryId={store.activeChatCategoryId}
          onActiveCategoryChange={store.setActiveChatCategoryId}
          onReload={() => {
            store.reloadChatCategoryConfigs().catch((error) => {
              // 重载失败沿用现有配置，仅提示。
              console.error(error);
            });
          }}
          onChange={(categoryId, patch) =>
            store.updateChatCategoryConfigs(
              store.chatCategoryConfigs.map((config) =>
                config.categoryId === categoryId ? { ...config, ...patch } : config,
              ),
            )
          }
        />
      </Section>
    </div>
  );
}
