export interface SubAgentModelConfig {
  inheritFromParent: boolean;
  apiBase?: string;
  apiKey?: string;
  modelName?: string;
}

export interface SubAgentConfig {
  agentId: string;
  agentName: string;
  description: string;
  systemPrompt: string;
  userPromptTemplate: string;
  allowedTools: string[];
  modelConfig: SubAgentModelConfig;
  maxIterations: number;
  maxOutputTokens: number;
  temperature: number;
  timeoutSecs: number;
  enabled: boolean;
  createdAt: number;
  updatedAt: number;
}

export interface SubAgentRecord {
  id: string;
  name: string;
  description: string;
  configJson: string;
  enabled: boolean;
  createdAt: number;
  updatedAt: number;
}

export interface SubAgentToolInfo {
  name: string;
  description: string;
}

export interface SubAgentUsage {
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
}

export type SubAgentEventType =
  | "Started"
  | "ToolStarted"
  | "ToolFinished"
  | "Progress"
  | "llmDelta"
  | "UsageUpdated"
  | "Finished"
  | "Failed";

export interface SubAgentEvent {
  event: SubAgentEventType;
  data: {
    agentId?: string;
    agentName?: string;
    task?: string;
    toolName?: string;
    arguments?: Record<string, unknown>;
    resultPreview?: string;
    message?: string;
    delta?: string;
    result?: string;
    iterations?: number;
    elapsedMs?: number;
    tokenUsage?: SubAgentUsage;
    error?: string;
  };
}

export interface SubAgentEventPayload {
  sessionId: string;
  toolCallId: string;
  timestampMs: number;
  event: SubAgentEventType;
  data: SubAgentEvent["data"];
}

export interface SubAgentRunTrace {
  workspaceId: string;
  toolCallId: string;
  agentId: string;
  status: "completed" | "failed";
  eventsJson: string;
  createdAt: string;
  updatedAt: string;
}
