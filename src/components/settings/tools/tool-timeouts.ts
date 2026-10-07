import type { AhaSettingsV2, ToolTimeoutSettings } from "../../../types";

/**
 * 工具超时默认（纯函数层）。
 *
 * 仅对「调用可声明超时」白名单工具生效（图片生成 / 编辑 / 下载）：未配置
 * 回退策略表默认；Agent 也可在调用参数中声明 timeout_secs（优先于本配置，
 * 上限一致）。生效值 = clamp(调用声明 ?? 此处默认 ?? 策略表默认)。
 */

/** 声明区间（与 src-tauri 的 rig_ext/tools/spec.rs 常量组镜像，改动两处同步）。 */
export const IMAGE_TIMEOUT_RANGE = { min: 30, max: 300 } as const;
export const FETCH_TIMEOUT_RANGE = { min: 10, max: 300 } as const;

export type ToolTimeoutFieldKey = keyof ToolTimeoutSettings;

export type ToolTimeoutFieldDef = {
  key: ToolTimeoutFieldKey;
  label: string;
  tip: string;
  min: number;
  max: number;
  /** 策略表默认（秒），仅用于 placeholder 展示（镜像 spec.rs 常量）。 */
  defaultSecs: number;
};

export const TOOL_TIMEOUT_FIELD_DEFS: ToolTimeoutFieldDef[] = [
  {
    key: "generateImageSecs",
    label: "图片生成",
    tip: "generate_image 工具的默认超时。复杂提示词 / 大尺寸生成常超 60 秒，默认 120；Agent 也可在调用时声明更长（上限 300）。",
    min: IMAGE_TIMEOUT_RANGE.min,
    max: IMAGE_TIMEOUT_RANGE.max,
    defaultSecs: 120,
  },
  {
    key: "editImageSecs",
    label: "图片编辑",
    tip: "edit_image 工具的默认超时。编辑大图或修改幅度大时偏慢，默认 120；Agent 也可在调用时声明更长（上限 300）。",
    min: IMAGE_TIMEOUT_RANGE.min,
    max: IMAGE_TIMEOUT_RANGE.max,
    defaultSecs: 120,
  },
  {
    key: "fetchImageSecs",
    label: "图片下载",
    tip: "fetch_image 工具的默认超时。慢速图源可调大，默认 60；Agent 也可在调用时声明更长（上限 300）。",
    min: FETCH_TIMEOUT_RANGE.min,
    max: FETCH_TIMEOUT_RANGE.max,
    defaultSecs: 60,
  },
];

export function timeoutFieldValue(
  settings: AhaSettingsV2,
  key: ToolTimeoutFieldKey,
): number | undefined {
  return settings.toolTimeouts?.[key];
}

/** 更新（或清除，value=undefined）某工具的超时默认，返回新 settings 不落盘。 */
export function patchToolTimeouts(
  settings: AhaSettingsV2,
  key: ToolTimeoutFieldKey,
  value: number | undefined,
): AhaSettingsV2 {
  const next: ToolTimeoutSettings = { ...(settings.toolTimeouts ?? {}) };
  if (value === undefined) {
    delete next[key];
  } else {
    next[key] = value;
  }
  return { ...settings, toolTimeouts: next };
}
