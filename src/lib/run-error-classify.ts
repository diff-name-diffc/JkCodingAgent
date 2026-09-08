/**
 * 运行错误分类（UI-25 遗留 c：发送失败「模型未配置」深链；
 * 第四批遗留领取：工具级「模型未配置」错误深链 + 分类推断）。
 *
 * 后端在 run 入口（`run_loop/core.rs` 的 `validate_provider_completeness`
 * 预校验）与各 adapter 的 `provider_missing_message` 处，对「模型未配置 /
 * 配置不完整」返回稳定的显式错误串。前端据这些稳定子串判定：命中时在错误
 * 展示旁提供「配置模型」深链，直达设置中心对应模型分类，取代只读的死错误。
 *
 * run 级错误的分类刻意不从错误串推断具体 category（text/vision）——深链
 * 发起点自知分类（主聊天/项目 = text、架构助手 = vision），与 6fdee44
 * 「配置模型深链直达分类」同一原则。工具级错误（tool-call-card 的
 * errorText）则相反：展示点不知道凭据属于哪个槽位，由
 * `inferModelNotConfiguredCategory` 从后端稳定串推断分类（vision/image/
 * imageEdit），推断不出时回退发起点默认分类。
 *
 * 子串匹配对上层包装免疫：`useDispatcherActions` 会把命令级失败包成
 * 「聊天执行失败：…」「调度智能体执行失败：…」，仍含后端子串。
 */

import type { ModelCategory } from "../types/chat";

/**
 * 后端稳定错误串特征子串。任一命中即判定为「模型未配置」类：
 *
 * run 级（发起点自知分类）：
 * - 「模型服务缺少 …」：`validate_provider_completeness`（缺 API Key / Base URL / 模型名）
 * - 「LLM API Key 未配置」：plain_chat / project adapter 的 provider_missing_message
 * - 「未配置视觉模型」：architecture adapter 的 provider_missing_message
 * - 「URL 或模型名为空」：build_architecture_agent 显式条目不完整分支
 *
 * 工具级（经 tool-call-card errorText 透出）：
 * - 「视觉模型未配置」/「视觉模型缺少 API Key」：analyze_image
 * - 「图片生成 API Key 未配置」：generate_image（builtin 与 image_generator 同串）
 * - 「图片编辑 API Key 未配置」：edit_image（builtin 与 image_generator 同串）
 *
 * 刻意排除：「未配置安全审查」（shell/local_zsh/ssh 审查门禁串）——修复入口
 * 是审查模型槽位而非模型服务分类页，不走本深链。
 *
 * 与后端串同源（同一仓库），改动后端错误文案时须同步这里与测试。
 */
const MODEL_NOT_CONFIGURED_MARKERS = [
  "模型服务缺少",
  "LLM API Key 未配置",
  "未配置视觉模型",
  "URL 或模型名为空",
  "视觉模型未配置",
  "视觉模型缺少 API Key",
  "图片生成 API Key 未配置",
  "图片编辑 API Key 未配置",
] as const;

/**
 * 工具级错误串 → 模型服务分类的推断表（先命中先得）。
 * 「无法调用视觉模型」覆盖 browser 工具的「LLM API Key 未配置，无法调用
 * 视觉模型」包装串——run 级同名 marker「LLM API Key 未配置」不带分类，
 * 含视觉上下文的该串推断为 vision。
 */
const TOOL_CATEGORY_MARKERS: readonly [marker: string, category: ModelCategory][] = [
  ["视觉模型未配置", "vision"],
  ["视觉模型缺少 API Key", "vision"],
  ["无法调用视觉模型", "vision"],
  ["图片生成 API Key 未配置", "image"],
  ["图片编辑 API Key 未配置", "imageEdit"],
];

/**
 * 判定运行/发送/工具错误是否属于「模型未配置或配置不完整」类。
 * 命中即可在 UI 侧提供「配置模型」深链。空串/普通网络错误返回 false。
 */
export function isModelNotConfiguredError(text: string | null | undefined): boolean {
  if (!text) return false;
  return MODEL_NOT_CONFIGURED_MARKERS.some((marker) => text.includes(marker));
}

/**
 * 从「模型未配置」类错误串推断设置中心模型服务分类（工具级深链用）。
 * 仅工具级稳定串可推断；run 级串（发起点自知分类）与未命中串返回 null，
 * 调用方以自身默认分类兜底。
 */
export function inferModelNotConfiguredCategory(
  text: string | null | undefined,
): ModelCategory | null {
  if (!text) return null;
  for (const [marker, category] of TOOL_CATEGORY_MARKERS) {
    if (text.includes(marker)) return category;
  }
  return null;
}
