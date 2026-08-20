import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import {
  Cpu,
  Database,
  Plug,
  Server,
  Settings2,
  SquareTerminal,
  Users,
  Workflow,
  Wrench,
  X,
  type LucideIcon,
} from "lucide-react";
import { AhaSettingsProvider, useAhaSettingsStore } from "./settings/use-aha-settings";
import { Toaster } from "./settings/Toaster";
import { ConfirmDialog } from "./settings/ConfirmDialog";
import { GeneralPage } from "./settings/GeneralPage";
import { GraphPage } from "./settings/GraphPage";
import { ToolsPage } from "./settings/ToolsPage";
import { SubAgentsPage } from "./settings/SubAgentsPage";
import { ProvidersPage } from "./settings/providers/ProvidersPage";
import { PurposesPage } from "./settings/providers/PurposesPage";
import { SshServersPage } from "./settings/ssh/SshServersPage";
import { McpServersPage } from "./settings/mcp/McpServersPage";
import { RagKbConfigPanel } from "./app-settings/rag/RagKbConfigPanel";
import type { ModelCategory } from "../types";

export type SettingsNavKey =
  | "general"
  | "providers"
  | "purposes"
  | "tools"
  | "graph"
  | "subAgents"
  | "mcp"
  | "ssh"
  | "rag";

const NAV_ITEMS: Array<{ key: SettingsNavKey; label: string; icon: LucideIcon }> = [
  { key: "general", label: "通用", icon: Settings2 },
  { key: "providers", label: "模型服务", icon: Server },
  { key: "purposes", label: "模型用途", icon: Cpu },
  { key: "tools", label: "工具", icon: Wrench },
  { key: "graph", label: "执行图", icon: Workflow },
  { key: "subAgents", label: "子智能体", icon: Users },
  { key: "mcp", label: "MCP 服务器", icon: Plug },
  { key: "ssh", label: "SSH", icon: SquareTerminal },
  { key: "rag", label: "RAG 知识库", icon: Database },
];

/** 校验调用点传入的导航 key。 */
function normalizeInitialTab(tab?: string): SettingsNavKey {
  switch (tab) {
    case "providers":
    case "purposes":
    case "tools":
    case "graph":
    case "subAgents":
    case "mcp":
    case "ssh":
    case "rag":
    case "general":
      return tab;
    default:
      return "general";
  }
}

export function AppSettingsDialog({
  onClose,
  initialTab,
  projectId,
  projectPath,
}: {
  onClose: () => void;
  /** 兼容旧的 `aha` 导航 key，新 key 见 SettingsNavKey。 */
  initialTab?: string;
  projectId?: string;
  projectPath?: string;
}) {
  const store = useAhaSettingsStore();
  const [activeNav, setActiveNav] = useState<SettingsNavKey>(() =>
    normalizeInitialTab(initialTab),
  );
  const [confirmingClose, setConfirmingClose] = useState(false);
  // 「模型用途」跳转「模型服务」时携带的目标分类（激活对应标签）。
  const [providersCategory, setProvidersCategory] = useState<ModelCategory | null>(null);
  // 通用页仍使用手动保存，通过 reportDirty 上报未保存状态。
  const manualDirtyRef = useRef(new Set<string>());
  const [manualDirty, setManualDirty] = useState(false);

  const reportDirty = useCallback((page: string) => {
    return (dirty: boolean) => {
      const set = manualDirtyRef.current;
      if (dirty) set.add(page);
      else set.delete(page);
      setManualDirty(set.size > 0);
    };
  }, []);

  const dirty = store.dirty || manualDirty;

  const requestClose = useCallback(() => {
    if (dirty) setConfirmingClose(true);
    else onClose();
  }, [dirty, onClose]);

  // Esc 关闭前检查未保存修改；Radix 内部弹层（Select/Dialog）已处理 Esc 时不重复触发。
  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key !== "Escape" || event.defaultPrevented || confirmingClose) return;
      event.preventDefault();
      requestClose();
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [requestClose, confirmingClose]);

  function handleOverlayClick(e: React.MouseEvent) {
    if (e.target === e.currentTarget) requestClose();
  }

  async function handleConfirmClose() {
    setConfirmingClose(false);
    if (store.dirty) {
      // 自动保存仍在进行时，先落盘再关闭；失败也允许关闭（错误已 toast）。
      await store.flush();
    }
    onClose();
  }

  const activeItem = NAV_ITEMS.find((item) => item.key === activeNav)!;
  const ActiveIcon = activeItem.icon;

  // 挂到 body：项目页 `.ai-*-shell > *` 会给祖先创建层叠上下文，
  // 内联渲染时弹窗会被主内容区遮挡；portal 后 z-index 在根层级生效。
  return createPortal(
    <AhaSettingsProvider value={store}>
      <div className="ai-dialog-overlay ai-settings-overlay" onClick={handleOverlayClick}>
        <div className="ai-settings-shell ai-migrated-settings">
          <nav className="ai-settings-nav" aria-label="应用设置">
            <div className="ai-settings-nav-title">
              <span>应用设置</span>
            </div>
            {NAV_ITEMS.map((item) => {
              const Icon = item.icon;
              return (
                <button
                  key={item.key}
                  type="button"
                  className={
                    activeNav === item.key
                      ? "ai-settings-nav-item is-active"
                      : "ai-settings-nav-item"
                  }
                  onClick={() => setActiveNav(item.key)}
                >
                  <Icon size={16} strokeWidth={1.5} />
                  <span className="ai-settings-nav-label">{item.label}</span>
                </button>
              );
            })}
          </nav>

          <section className="ai-settings-content">
            <div className="ai-settings-header">
              <div className="ai-settings-title-wrap">
                <ActiveIcon size={16} strokeWidth={1.5} />
                <span className="ai-settings-content-title">{activeItem.label}</span>
              </div>
              <button
                className="ai-settings-close"
                onClick={requestClose}
                title="关闭"
                type="button"
              >
                <X size={16} strokeWidth={2} />
              </button>
            </div>

            <div className="ai-settings-panel-host chat-scroll">
              {activeNav === "general" && <GeneralPage reportDirty={reportDirty("general")} />}
              {activeNav === "providers" && (
                <ProvidersPage
                  key={providersCategory ?? "default"}
                  initialCategory={providersCategory ?? undefined}
                />
              )}
              {activeNav === "purposes" && (
                <PurposesPage
                  onNavigateProviders={(category) => {
                    setProvidersCategory(category);
                    setActiveNav("providers");
                  }}
                />
              )}
              {activeNav === "tools" && <ToolsPage projectPath={projectPath} />}
              {activeNav === "graph" && <GraphPage />}
              {activeNav === "subAgents" && <SubAgentsPage />}
              {activeNav === "mcp" && <McpServersPage />}
              {activeNav === "ssh" && <SshServersPage />}
              {activeNav === "rag" && (
                <RagKbConfigPanel projectId={projectId} projectPath={projectPath} />
              )}
            </div>
          </section>
        </div>

        <Toaster />
        <ConfirmDialog
          open={confirmingClose}
          title="有未保存的修改"
          description={
            store.dirty
              ? "部分修改还在保存中。关闭前将先完成保存。"
              : "当前页面有尚未保存的修改，关闭后将丢失。"
          }
          confirmLabel={store.dirty ? "保存并关闭" : "仍要关闭"}
          cancelLabel="继续编辑"
          onConfirm={() => void handleConfirmClose()}
          onCancel={() => setConfirmingClose(false)}
        />
      </div>
    </AhaSettingsProvider>,
    document.body,
  );
}
