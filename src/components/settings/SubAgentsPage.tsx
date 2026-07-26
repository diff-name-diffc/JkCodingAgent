import { useAhaSettings } from "./use-aha-settings";
import { Section } from "./Section";
import { SubAgentManagePanel } from "../app-settings/sub-agents/SubAgentManagePanel";
import { SubAgentPicker } from "../app-settings/aha/sub-agent-picker";

/** 「子智能体」页：子智能体管理 + 全局启用列表（勾选走自动保存管线）。 */
export function SubAgentsPage() {
  const store = useAhaSettings();

  if (store.loading) {
    return <div className="ai-settings-empty">加载中...</div>;
  }

  return (
    <div className="ai-set-page">
      <Section title="全局启用" description="勾选的子智能体对所有会话（项目与聊天）自动生效。">
        <SubAgentPicker
          enabledIds={store.globalEnabledIds}
          title="全局启用子智能体"
          description="勾选的子智能体对所有会话（项目与聊天）自动生效。"
          onChange={store.updateGlobalEnabledIds}
        />
      </Section>
      {/* SubAgentManagePanel 自带标题栏与「新建」按钮，直接挂载（内部滚动由 CSS 改为随页面滚动） */}
      <SubAgentManagePanel />
    </div>
  );
}
