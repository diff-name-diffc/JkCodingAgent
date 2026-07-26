import type {
  AhaSettingsV2,
  ModelCategory,
  ModelLibraryEntry,
} from "../../../types";
import {
  PURPOSE_DEFS,
  getPurposeConfigs,
  type PurposeKind,
} from "./provider-registry";

/**
 * 分类模型库（纯函数层）。
 *
 * 模型库按模型调用方式分类（text/vision/image/...），每个条目独立持有
 * url/apiKey/model，存于 `AhaSettingsV2.modelLibrary`。「模型服务」页按分类
 * 分标签管理条目；「模型用途」页从对应分类的条目中选择，选中后把
 * url/apiKey/model 拷贝进用途槽位（见 provider-registry.ts 的 bindPurpose）。
 */

export type ModelCategoryDef = {
  category: ModelCategory;
  label: string;
  description: string;
  /** dispatcher_test_model 的 kind 参数。 */
  testKind: string;
  /** 「获取模型」对该分类无意义（非 OpenAI 兼容 /v1/models）时隐藏。 */
  isModelListFetchable: boolean;
};

export const CATEGORY_DEFS: ModelCategoryDef[] = [
  { category: "text", label: "对话模型", description: "项目/聊天主模型、摘要模型与 SSH 审查等文本对话模型。", testKind: "chat", isModelListFetchable: true },
  { category: "vision", label: "视觉模型", description: "用户上传图片时使用的多模态模型。", testKind: "vision", isModelListFetchable: true },
  { category: "image", label: "图片生成", description: "generate_image 工具使用的图片生成模型。", testKind: "image", isModelListFetchable: true },
  { category: "imageEdit", label: "图片编辑", description: "edit_image 工具使用的图片编辑模型。", testKind: "imageEdit", isModelListFetchable: true },
  { category: "asr", label: "语音识别", description: "实时语音识别配置，URL 为 WebSocket 地址。", testKind: "asr", isModelListFetchable: false },
  { category: "tts", label: "语音合成", description: "预留的文本转语音模型配置。", testKind: "tts", isModelListFetchable: false },
  { category: "embedding", label: "向量模型", description: "预留的向量模型配置。", testKind: "embedding", isModelListFetchable: true },
];

export function categoryDef(category: ModelCategory): ModelCategoryDef {
  return CATEGORY_DEFS.find((def) => def.category === category)!;
}

const PURPOSE_CATEGORY_MAP: Record<PurposeKind, ModelCategory> = {
  projectChat: "text",
  projectSummary: "text",
  chatChat: "text",
  chatSummary: "text",
  review: "text",
  vision: "vision",
  image: "image",
  imageEdit: "imageEdit",
  asr: "asr",
  tts: "tts",
  embedding: "embedding",
};

/** 用途 → 模型库分类。用途下拉只列出对应分类的条目。 */
export function purposeCategory(kind: PurposeKind): ModelCategory {
  return PURPOSE_CATEGORY_MAP[kind];
}

export function createEntry(category: ModelCategory): ModelLibraryEntry {
  return { id: crypto.randomUUID(), category, url: "", apiKey: "", model: "", enabled: true };
}

/** 某分类下的条目（可选只取启用中的），按别名/模型名排序。 */
export function entriesForCategory(
  library: ModelLibraryEntry[],
  category: ModelCategory,
  options?: { enabledOnly?: boolean },
): ModelLibraryEntry[] {
  return library
    .filter(
      (entry) =>
        entry.category === category && (!options?.enabledOnly || entry.enabled !== false),
    )
    .sort((a, b) => entryLabel(a).localeCompare(entryLabel(b)));
}

export function entryLabel(entry: ModelLibraryEntry): string {
  return entry.alias?.trim() || entry.model.trim() || "未命名模型";
}

// ── 首次迁移播种 ──────────────────────────────────────────────────────────────

/** 是否有任何用途已配置模型（决定是否需要从旧配置播种模型库）。 */
export function hasAnyPurposeConfigs(settings: AhaSettingsV2): boolean {
  return PURPOSE_DEFS.some((def) =>
    getPurposeConfigs(settings, def.kind).some((config) => config.url.trim() || config.model.trim()),
  );
}

/**
 * 首次迁移：把现有各用途配置按分类去重（url+apiKey+model）转成库条目。
 * 旧用途配置保持原样，绑定关系不受影响。
 */
export function seedModelLibrary(settings: AhaSettingsV2): ModelLibraryEntry[] {
  const seen = new Map<string, ModelLibraryEntry>();
  for (const def of PURPOSE_DEFS) {
    const category = purposeCategory(def.kind);
    for (const config of getPurposeConfigs(settings, def.kind)) {
      const url = config.url.trim();
      const model = config.model.trim();
      if (!url && !model) continue;
      const key = `${category}::${url}::${config.apiKey.trim()}::${model}`;
      if (seen.has(key)) continue;
      seen.set(key, {
        id: crypto.randomUUID(),
        category,
        url,
        apiKey: config.apiKey.trim(),
        model,
        enabled: true,
      });
    }
  }
  return [...seen.values()];
}

// ── 条目 CRUD（返回新 settings，不落盘） ──────────────────────────────────────

export function upsertLibraryEntry(
  settings: AhaSettingsV2,
  entry: ModelLibraryEntry,
): AhaSettingsV2 {
  const library = settings.modelLibrary ?? [];
  const index = library.findIndex((item) => item.id === entry.id);
  const next =
    index >= 0
      ? library.map((item) => (item.id === entry.id ? entry : item))
      : [...library, entry];
  return { ...settings, modelLibrary: next };
}

export function patchLibraryEntry(
  settings: AhaSettingsV2,
  id: string,
  patch: Partial<Omit<ModelLibraryEntry, "id" | "category">>,
): AhaSettingsV2 {
  return {
    ...settings,
    modelLibrary: (settings.modelLibrary ?? []).map((item) =>
      item.id === id ? { ...item, ...patch } : item,
    ),
  };
}

export function removeLibraryEntry(settings: AhaSettingsV2, id: string): AhaSettingsV2 {
  return {
    ...settings,
    modelLibrary: (settings.modelLibrary ?? []).filter((item) => item.id !== id),
  };
}

// ── 引用统计 ──────────────────────────────────────────────────────────────────

function configMatchesEntry(
  config: { url: string; apiKey: string; model: string },
  entry: ModelLibraryEntry,
): boolean {
  return (
    config.url.trim() === entry.url.trim() &&
    config.apiKey.trim() === entry.apiKey.trim() &&
    config.model.trim() === entry.model.trim()
  );
}

/**
 * 在模型库中查找与某用途绑定配置（url/apiKey/model）匹配的启用条目。
 * 设置页与聊天输入框的模型下拉用它确定「当前生效」的库条目，保证两处一致。
 */
export function findEnabledEntryForConfig(
  library: ModelLibraryEntry[],
  config: { url: string; apiKey: string; model: string } | null | undefined,
): ModelLibraryEntry | undefined {
  if (!config?.url.trim()) return undefined;
  return library.find((entry) => entry.enabled !== false && configMatchesEntry(config, entry));
}

/** 引用该条目的用途标题列表（删除确认时展示）。 */
export function entryUsageTitles(settings: AhaSettingsV2, entry: ModelLibraryEntry): string[] {
  return PURPOSE_DEFS.filter((def) => {
    const binding = getPurposeConfigs(settings, def.kind).find((config) => config.active);
    return binding ? configMatchesEntry(binding, entry) : false;
  }).map((def) => def.title);
}
