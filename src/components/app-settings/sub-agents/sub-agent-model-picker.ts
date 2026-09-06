import type {
  ModelCategory,
  ModelLibraryEntry,
  SubAgentModelConfig,
} from "../../../types";
import {
  categoryDef,
  entriesForCategory,
} from "../../settings/providers/model-library";

/**
 * 子智能体「自定义配置」模型选择器的纯函数层。
 *
 * 只暴露对话模型（text）与视觉模型（vision）两个分类：先选分类，再从该分类的
 * 启用条目中选模型。选中后把库条目的 url/apiKey/model 回填进子智能体的
 * modelConfig（apiBase/apiKey/modelName），仍由对话框「保存」按钮统一落库。
 */

/** 可挑选的模型分类（顺序即分类下拉展示顺序）。 */
export const PICKABLE_CATEGORIES: ModelCategory[] = ["text", "vision"];

export interface PickableCategoryOption {
  category: ModelCategory;
  label: string;
}

/** 分类下拉选项（复用 model-library 的 CATEGORY_DEFS 中文 label）。 */
export function pickableCategoryOptions(): PickableCategoryOption[] {
  return PICKABLE_CATEGORIES.map((category) => ({
    category,
    label: categoryDef(category).label,
  }));
}

/** 某分类下可选的启用模型条目（按别名/模型名排序，见 entriesForCategory）。 */
export function pickableEntries(
  library: ModelLibraryEntry[],
  category: ModelCategory,
): ModelLibraryEntry[] {
  return entriesForCategory(library, category, { enabledOnly: true });
}

/**
 * 在可挑选分类的启用条目中，按 url+model（apiKey 作消歧）匹配当前
 * modelConfig 对应的库条目。命中用于回填下拉的「当前选中」；未命中（含手填
 * 或指向停用/已删条目）返回 undefined，交由调用方展示占位符。
 */
export function findMatchedLibraryEntry(
  library: ModelLibraryEntry[],
  modelConfig: Pick<SubAgentModelConfig, "apiBase" | "apiKey" | "modelName">,
): ModelLibraryEntry | undefined {
  const apiBase = modelConfig.apiBase?.trim();
  const modelName = modelConfig.modelName?.trim();
  if (!apiBase || !modelName) return undefined;
  const apiKey = modelConfig.apiKey?.trim();
  return PICKABLE_CATEGORIES.flatMap((category) =>
    pickableEntries(library, category),
  ).find((entry) => {
    if (entry.url.trim() !== apiBase || entry.model.trim() !== modelName) return false;
    // 同 url+model 可能有多个账号条目：apiKey 一致才算命中，否则下拉显示
    // 与已回填的 Key 可能来自不同条目。
    return !apiKey || !entry.apiKey.trim() || entry.apiKey.trim() === apiKey;
  });
}
