import type {
  AhaContextConfig,
  AhaSettingsV2,
  AhaSharedModels,
  DispatcherModelConfig,
} from "../../../types";

/**
 * 模型用途读写层（纯函数）。
 *
 * 存储结构中每个模型用途各持 `DispatcherModelConfig[]`。「模型服务」页按调用方式
 * 分类维护模型库（见 model-library.ts），「模型用途」页从库中选择条目后，由本模块
 * 写入携带 libraryId 引用的用途槽位——落库时后端剥离 url/apiKey/model 只保留引用，
 * 读取时再从模型库条目回填凭据，库更新后用途自动跟随。
 */

// ── 用途定义 ──────────────────────────────────────────────────────────────────

export type SharedPurposeKind = "vision" | "image" | "imageEdit" | "asr" | "tts" | "embedding";

export type PurposeKind =
  | SharedPurposeKind
  | "projectChat"
  | "projectSummary"
  | "chatChat"
  | "chatSummary"
  | "review";

export type PurposeDef = {
  kind: PurposeKind;
  title: string;
  description: string;
  /** dispatcher_test_model 的 kind 参数。 */
  testKind: string;
  /** 「获取模型」对该用途无意义（非 OpenAI 兼容 /v1/models）时隐藏。 */
  isModelListFetchable: boolean;
};

export const PURPOSE_DEFS: PurposeDef[] = [
  { kind: "vision", title: "视觉模型", description: "用户上传图片时使用的多模态模型。", testKind: "vision", isModelListFetchable: true },
  { kind: "image", title: "图片生成模型", description: "generate_image 工具使用的图片生成模型。", testKind: "image", isModelListFetchable: true },
  { kind: "imageEdit", title: "图片编辑模型", description: "edit_image 工具使用的图片编辑模型。", testKind: "imageEdit", isModelListFetchable: true },
  { kind: "asr", title: "语音识别（ASR）模型", description: "实时语音识别配置，URL 为 WebSocket 地址。", testKind: "asr", isModelListFetchable: false },
  { kind: "tts", title: "语音合成（TTS）模型", description: "预留的文本转语音模型配置。", testKind: "tts", isModelListFetchable: false },
  { kind: "embedding", title: "文本向量模型", description: "预留的向量模型配置。", testKind: "embedding", isModelListFetchable: true },
  { kind: "projectChat", title: "项目主模型", description: "项目对话和工具调用的主模型。", testKind: "chat", isModelListFetchable: true },
  { kind: "projectSummary", title: "项目摘要模型", description: "项目会话中工具结果、子任务输出和会话标题的摘要；留空时使用默认模型。", testKind: "summary", isModelListFetchable: true },
  { kind: "chatChat", title: "聊天主模型", description: "聊天对话和工具调用的主模型。", testKind: "chat", isModelListFetchable: true },
  { kind: "chatSummary", title: "聊天摘要模型", description: "聊天会话中工具结果、子任务输出和会话标题的摘要；留空时使用默认模型。", testKind: "summary", isModelListFetchable: true },
  { kind: "review", title: "SSH 审查模型", description: "SSH 命令执行前的安全审查模型。", testKind: "review", isModelListFetchable: true },
];

const SHARED_FIELD_MAP: Record<SharedPurposeKind, keyof AhaSharedModels> = {
  vision: "visionModelConfigs",
  image: "imageModelConfigs",
  imageEdit: "imageEditModelConfigs",
  asr: "asrModelConfigs",
  tts: "ttsModelConfigs",
  embedding: "embeddingModelConfigs",
};

// ── 用途配置读写 ──────────────────────────────────────────────────────────────

export function getPurposeConfigs(settings: AhaSettingsV2, kind: PurposeKind): DispatcherModelConfig[] {
  switch (kind) {
    case "projectChat":
      return settings.project.chatModelConfigs;
    case "projectSummary":
      return settings.project.summaryModelConfigs;
    case "chatChat":
      return settings.chat.chatModelConfigs;
    case "chatSummary":
      return settings.chat.summaryModelConfigs;
    case "review":
      return settings.review?.modelConfig?.url ? [settings.review.modelConfig] : [];
    default:
      return settings.shared[SHARED_FIELD_MAP[kind]];
  }
}

function setContextConfigs(
  context: AhaContextConfig,
  field: "chatModelConfigs" | "summaryModelConfigs",
  configs: DispatcherModelConfig[],
): AhaContextConfig {
  return { ...context, [field]: configs };
}

export function setPurposeConfigs(
  settings: AhaSettingsV2,
  kind: PurposeKind,
  configs: DispatcherModelConfig[],
): AhaSettingsV2 {
  switch (kind) {
    case "projectChat":
      return { ...settings, project: setContextConfigs(settings.project, "chatModelConfigs", configs) };
    case "projectSummary":
      return { ...settings, project: setContextConfigs(settings.project, "summaryModelConfigs", configs) };
    case "chatChat":
      return { ...settings, chat: setContextConfigs(settings.chat, "chatModelConfigs", configs) };
    case "chatSummary":
      return { ...settings, chat: setContextConfigs(settings.chat, "summaryModelConfigs", configs) };
    case "review": {
      const base = settings.review ?? { modelConfig: emptyModelConfig(), systemPrompt: "" };
      return {
        ...settings,
        review: { ...base, modelConfig: configs[0] ?? emptyModelConfig() },
      };
    }
    default:
      return { ...settings, shared: { ...settings.shared, [SHARED_FIELD_MAP[kind]]: configs } };
  }
}

function emptyModelConfig(): DispatcherModelConfig {
  return { url: "", apiKey: "", model: "", active: true, systemPrompt: "" };
}

/** 某用途当前生效的绑定（active 的那条；无则未配置）。 */
export function getPurposeBinding(
  settings: AhaSettingsV2,
  kind: PurposeKind,
): DispatcherModelConfig | null {
  const configs = getPurposeConfigs(settings, kind);
  if (kind === "review") return configs[0] ?? null;
  return configs.find((config) => config.active && config.url) ?? null;
}

/**
 * 用途绑定：把该用途设为指向目标模型（来自模型库条目）的单条配置。
 * 绑定携带 libraryId 引用（落盘只保留引用，凭据由库解析）；
 * 传入空 url 表示解除绑定（回到「未配置」）。
 */
export function bindPurpose(
  settings: AhaSettingsV2,
  kind: PurposeKind,
  target: { id?: string; url: string; apiKey: string; model: string },
): AhaSettingsV2 {
  return setPurposeConfigs(settings, kind, [
    {
      url: target.url,
      apiKey: target.apiKey,
      model: target.model,
      active: true,
      systemPrompt: "",
      ...(target.id ? { libraryId: target.id } : {}),
    },
  ]);
}

// ── 模型能力标签（尽力而为的静态启发式） ──────────────────────────────────────

const VISION_PATTERN = /(vision|vl|gpt-4o|gpt-5|claude|gemini|qwen-vl|4v|llava|minicpm-v)/i;
const LONG_CONTEXT_PATTERN = /(128k|200k|256k|512k|1m|long|kimi|claude|gemini|qwen-long)/i;

export function modelCapabilityTags(model: string): string[] {
  const tags: string[] = [];
  if (VISION_PATTERN.test(model)) tags.push("视觉");
  if (LONG_CONTEXT_PATTERN.test(model)) tags.push("长上下文");
  return tags;
}

// ── 从旧 provider-editor 迁入的工具函数（仍被保存管线使用） ────────────────────

export function withoutModelSystemPrompts(
  providers: DispatcherModelConfig[],
): DispatcherModelConfig[] {
  return providers.map((provider) => ({ ...provider, systemPrompt: "" }));
}

export function withoutChatModelSystemPrompts(config: AhaContextConfig): AhaContextConfig {
  return { ...config, chatModelConfigs: withoutModelSystemPrompts(config.chatModelConfigs) };
}
