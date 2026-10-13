import type { SubAgentConfig, SubAgentRecord } from "../../../types";
import { BROWSER_AGENT_ID } from "../../../lib/sub-agent-runtime";

export function toBackendConfig(config: SubAgentConfig): string {
  return JSON.stringify({
    agent_id: config.agentId,
    agent_name: config.agentName,
    description: config.description,
    system_prompt: config.systemPrompt,
    user_prompt_template: config.userPromptTemplate,
    allowed_tools: config.agentId === BROWSER_AGENT_ID ? [] : config.allowedTools,
    browser_use_vision: config.browserUseVision ?? false,
    model_config: {
      inherit_from_parent: config.modelConfig.inheritFromParent,
      api_base: config.modelConfig.apiBase || null,
      api_key: config.modelConfig.apiKey || null,
      model_name: config.modelConfig.modelName || null,
    },
    max_iterations: config.maxIterations,
    max_output_tokens: config.maxOutputTokens,
    temperature: config.temperature,
    timeout_secs: config.timeoutSecs,
    enabled: config.enabled,
    created_at: config.createdAt,
    updated_at: config.updatedAt,
  });
}

export function fromBackendConfig(record: SubAgentRecord): SubAgentConfig | null {
  try {
    const raw = JSON.parse(record.configJson);
    return {
      agentId: raw.agent_id ?? record.id,
      agentName: raw.agent_name ?? record.name,
      description: raw.description ?? record.description,
      systemPrompt: raw.system_prompt ?? "",
      userPromptTemplate: raw.user_prompt_template ?? "{{task}}",
      allowedTools: Array.isArray(raw.allowed_tools)
        ? raw.allowed_tools
        : Array.isArray(raw.allowedTools)
          ? raw.allowedTools
          : [],
      browserUseVision: raw.browser_use_vision ?? raw.browserUseVision ?? false,
      modelConfig: {
        inheritFromParent:
          raw.model_config?.inherit_from_parent ?? raw.modelConfig?.inheritFromParent ?? true,
        apiBase: raw.model_config?.api_base ?? raw.modelConfig?.apiBase ?? undefined,
        apiKey: raw.model_config?.api_key ?? raw.modelConfig?.apiKey ?? undefined,
        modelName: raw.model_config?.model_name ?? raw.modelConfig?.modelName ?? undefined,
      },
      maxIterations: raw.max_iterations ?? raw.maxIterations ?? 60,
      maxOutputTokens: raw.max_output_tokens ?? raw.maxOutputTokens ?? 4096,
      temperature: raw.temperature ?? 0.7,
      timeoutSecs: raw.timeout_secs ?? raw.timeoutSecs ?? 3600,
      enabled: record.enabled,
      createdAt: record.createdAt,
      updatedAt: record.updatedAt,
    };
  } catch {
    return null;
  }
}
