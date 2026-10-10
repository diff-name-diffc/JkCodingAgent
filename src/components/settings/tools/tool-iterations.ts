import type { AhaSettingsV2 } from "../../../types";

/**
 * 工具迭代轮数上限（纯函数层）。
 *
 * 主对话循环（普通聊天 / 项目编排 / 架构助手）单次 run 允许的最大工具调用
 * 轮数：未配置回退内置默认；生效值由后端 `effective_max_tool_iterations`
 * 统一解析（越界视同未配置，不做静默夹紧）。
 */

/** 声明区间与默认值（与 src-tauri 的 agent/config.rs 常量镜像，改动两处同步）。 */
export const TOOL_ITERATIONS_RANGE = { min: 1, max: 10000 } as const;
export const DEFAULT_TOOL_ITERATIONS = 1000;

export function maxToolIterationsValue(settings: AhaSettingsV2): number | undefined {
  return settings.maxToolIterations;
}

/** 更新（或清除，value=undefined）迭代轮数上限，返回新 settings 不落盘。 */
export function patchMaxToolIterations(
  settings: AhaSettingsV2,
  value: number | undefined,
): AhaSettingsV2 {
  const next = { ...settings };
  if (value === undefined) {
    delete next.maxToolIterations;
  } else {
    next.maxToolIterations = value;
  }
  return next;
}
