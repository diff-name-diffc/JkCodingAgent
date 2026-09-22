import { useState } from "react";
import { DEFAULT_ACP_COMMAND, type GraphExecutionConfig } from "../../types";
import { ApiKeyInput } from "./ApiKeyInput";
import { FieldLabel } from "./FieldLabel";
import { Section } from "./Section";
import { useAhaSettings } from "./use-aha-settings";

/** 历史默认启动命令（npx 运行时拉取）：与后端一致归一为托管模式（空串）。 */
const LEGACY_NPX_ACP_COMMAND = "npx -y @agentclientprotocol/claude-agent-acp@0.79.0";

function graphOrDefault(graph: GraphExecutionConfig | undefined): GraphExecutionConfig {
  const resolved = graph ?? { pauseBeforeWrite: true, acp: { command: DEFAULT_ACP_COMMAND } };
  if (resolved.acp?.command === LEGACY_NPX_ACP_COMMAND) {
    return { ...resolved, acp: { ...resolved.acp, command: DEFAULT_ACP_COMMAND } };
  }
  return resolved;
}

/**
 * 「执行图」设置页：执行图编排的运行期行为开关与节点执行器（ACP）配置。
 * 数据存于 `AhaSettingsV2.graph`，走 use-aha-settings 自动保存管线。
 * 文本类字段本地暂存、失焦提交（与模型条目卡同一保存节奏）。
 */
export function GraphPage() {
  const { settings, updateSettings } = useAhaSettings();
  const graph = graphOrDefault(settings?.graph);
  const acp = graph.acp ?? { command: DEFAULT_ACP_COMMAND };

  const [command, setCommand] = useState(acp.command);
  const [apiKey, setApiKey] = useState(acp.apiKey ?? "");
  const [baseUrl, setBaseUrl] = useState(acp.baseUrl ?? "");

  function commitAcp(patch: Partial<NonNullable<GraphExecutionConfig["acp"]>>) {
    updateSettings((prev) => {
      const current = graphOrDefault(prev.graph);
      return {
        ...prev,
        graph: { ...current, acp: { command: DEFAULT_ACP_COMMAND, ...current.acp, ...patch } },
      };
    });
  }

  return (
    <div className="ai-set-page">
      <Section
        title="高危写操作检查点"
        description="执行图中第一个修改类（coding）节点启动前暂停运行，等你在图面板确认后继续。适合在执行文件修改/命令前人工把关；关闭后执行图将一口气跑到结束。"
      >
        <div className="flex items-center gap-1.5">
          <button
            type="button"
            role="switch"
            aria-checked={graph.pauseBeforeWrite}
            aria-label="写操作前暂停确认"
            className={graph.pauseBeforeWrite ? "ai-set-switch is-on" : "ai-set-switch"}
            onClick={() =>
              updateSettings((prev) => ({
                ...prev,
                graph: {
                  ...graphOrDefault(prev.graph),
                  pauseBeforeWrite: !(prev.graph?.pauseBeforeWrite ?? true),
                },
              }))
            }
          >
            <span className="ai-set-switch-thumb" />
          </button>
          <span className="ai-aha-hint">
            {graph.pauseBeforeWrite ? "已开启：写操作前暂停" : "已关闭：直接执行"}
          </span>
        </div>
      </Section>
      <Section
        title="节点执行器（Claude Agent / ACP）"
        description="执行图节点由 claude-agent-acp 子进程执行，要求本机已安装 Node.js ≥ 22。默认托管模式：应用把版本锁定的官方包安装到 ~/.jkcodingagent/acp-agent/ 后以固定路径启动（首次运行需联网安装），子进程仅继承白名单环境变量。凭据留空时依赖本机 ~/.claude 登录态；填写后注入子进程环境变量（ANTHROPIC_API_KEY / ANTHROPIC_BASE_URL）。"
      >
        <div className="ai-set-field">
          <FieldLabel label="启动命令" tip="留空 = 托管模式（推荐）：自动安装并锁定官方包版本，固定路径启动。填写自定义命令则按空白拆分为程序与参数原样执行。" />
          <input
            className="ai-settings-input"
            value={command}
            onChange={(e) => setCommand(e.target.value)}
            onBlur={() => {
              const trimmed = command.trim();
              commitAcp({ command: trimmed || DEFAULT_ACP_COMMAND });
              setCommand(trimmed || DEFAULT_ACP_COMMAND);
            }}
            placeholder="留空：托管模式（自动安装并锁定官方执行器）"
            spellCheck={false}
            autoComplete="off"
          />
        </div>
        <div className="ai-set-field">
          <FieldLabel label="API Key" tip="可选；留空使用本机 Claude 登录态。" />
          <ApiKeyInput
            value={apiKey}
            onChange={setApiKey}
            onBlur={() => commitAcp({ apiKey: apiKey.trim() || null })}
          />
        </div>
        <div className="ai-set-field">
          <FieldLabel label="Base URL" tip="可选；自定义 Anthropic 兼容网关地址。" />
          <input
            className="ai-settings-input"
            value={baseUrl}
            onChange={(e) => setBaseUrl(e.target.value)}
            onBlur={() => commitAcp({ baseUrl: baseUrl.trim() || null })}
            placeholder="https://api.anthropic.com"
            spellCheck={false}
            autoComplete="off"
          />
        </div>
      </Section>
    </div>
  );
}
