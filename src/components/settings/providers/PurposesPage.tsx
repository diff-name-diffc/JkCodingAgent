import { useAhaSettings } from "../use-aha-settings";
import { Section } from "../Section";
import { Tooltip, TooltipContent, TooltipTrigger } from "../../ui/tooltip";
import type { ModelCategory } from "../../../types";
import { PURPOSE_DEFS, type PurposeKind } from "./provider-registry";
import { PurposeSelect } from "./PurposeSelect";

const PURPOSE_GROUPS: Array<{
  id: string;
  title: string;
  description: string;
  kinds: PurposeKind[];
}> = [
  {
    id: "shared",
    title: "能力模型",
    description: "视觉、图片、语音、向量等共享能力，供所有智能体按需调用。",
    kinds: ["vision", "image", "imageEdit", "asr", "tts", "embedding"],
  },
  {
    id: "project",
    title: "项目智能体",
    description: "项目会话使用的对话与摘要模型。",
    kinds: ["projectChat", "projectSummary"],
  },
  {
    id: "chat",
    title: "聊天智能体",
    description: "聊天会话使用的对话与摘要模型。",
    kinds: ["chatChat", "chatSummary"],
  },
  {
    id: "ssh",
    title: "SSH 安全审查",
    description: "SSH 命令执行前的安全门禁所使用的模型。",
    kinds: ["review"],
  },
];

/**
 * 「模型用途」页：每个用途一个下拉，选项来自「模型服务」页对应分类的模型库条目。
 * 页内分区 + 顶部锚点导航；切换即自动保存。
 */
export function PurposesPage({
  onNavigateProviders,
}: {
  onNavigateProviders: (category: ModelCategory) => void;
}) {
  const store = useAhaSettings();

  if (store.loading || !store.settings) {
    return <div className="ai-settings-empty">加载中...</div>;
  }

  const settings = store.settings;

  return (
    <div className="ai-set-page">
      <div className="ai-set-page-head">
        <div>
          <h2 className="ai-set-page-title">模型用途</h2>
          <p className="ai-set-page-description">
            为每个功能选择要使用的模型。模型在「模型服务」页按分类统一维护。
          </p>
        </div>
      </div>

      <nav className="ai-set-anchor-nav" aria-label="页内导航">
        {PURPOSE_GROUPS.map((group) => (
          <a key={group.id} className="ai-set-anchor" href={`#purpose-group-${group.id}`}>
            {group.title}
          </a>
        ))}
        <a className="ai-set-anchor" href="#purpose-group-behavior">
          行为
        </a>
      </nav>

      {PURPOSE_GROUPS.map((group) => (
        <Section
          key={group.id}
          id={`purpose-group-${group.id}`}
          title={group.title}
          description={group.description}
          aside={<span className="ai-set-section-count">{group.kinds.length} 项</span>}
        >
          <div className="ai-set-purpose-list">
            {group.kinds.map((kind) => {
              const def = PURPOSE_DEFS.find((item) => item.kind === kind)!;
              return (
                <div key={kind} className="ai-set-purpose-row">
                  <div className="ai-set-purpose-meta">
                    <span className="ai-set-purpose-title">{def.title}</span>
                    <span className="ai-set-purpose-description">{def.description}</span>
                  </div>
                  <PurposeSelect def={def} onNavigateProviders={onNavigateProviders} />
                </div>
              );
            })}
          </div>
        </Section>
      ))}

      <Section
        id="purpose-group-behavior"
        title="行为"
        description="这些开关影响智能体的执行方式（项目和聊天共享）。"
      >
        <SwitchRow
          label="自动批准操作"
          tip="开启后，智能体在运行子任务前不再额外请求确认。"
          checked={settings.autoApproveDispatch}
          onChange={(value) =>
            store.updateSettings(
              (prev) => ({ ...prev, autoApproveDispatch: value }),
              "behavior:autoApprove",
            )
          }
        />
        <SwitchRow
          label="上下文调试日志"
          tip="仅在调试时开启。日志文件位于项目根目录的 logs/agent.debug。"
          checked={settings.contextDebug}
          onChange={(value) =>
            store.updateSettings((prev) => ({ ...prev, contextDebug: value }), "behavior:debug")
          }
        />
      </Section>
    </div>
  );
}

function SwitchRow({
  label,
  tip,
  checked,
  onChange,
}: {
  label: string;
  tip: string;
  checked: boolean;
  onChange: (value: boolean) => void;
}) {
  return (
    <div className="ai-set-switch-row">
      <span className="ai-set-switch-label">{label}</span>
      <Tooltip>
        <TooltipTrigger asChild>
          <button
            type="button"
            role="switch"
            aria-checked={checked}
            aria-label={label}
            title={tip}
            className={checked ? "ai-set-switch is-on" : "ai-set-switch"}
            onClick={() => onChange(!checked)}
          >
            <span className="ai-set-switch-thumb" />
          </button>
        </TooltipTrigger>
        <TooltipContent side="top" className="max-w-64">
          {tip}
        </TooltipContent>
      </Tooltip>
    </div>
  );
}
