/**
 * 运行错误分类（UI-25 遗留 c：发送失败「模型未配置」深链）。
 *
 * 后端在 run 入口（`run_loop/core.rs` 的 `validate_provider_completeness`
 * 预校验）与各 adapter 的 `provider_missing_message` 处，对「模型未配置 /
 * 配置不完整」返回稳定的显式错误串。前端据这些稳定子串判定：命中时在错误
 * 展示旁提供「配置模型」深链，直达设置中心对应模型分类，取代只读的死错误。
 *
 * 分类刻意不从错误串推断具体 category（text/vision）——深链发起点自知分类
 * （主聊天/项目 = text、架构助手 = vision），与 6fdee44「配置模型深链直达
 * 分类」同一原则；本函数只回答「这是不是模型未配置类错误」。
 *
 * 子串匹配对上层包装免疫：`useDispatcherActions` 会把命令级失败包成
 * 「聊天执行失败：…」「调度智能体执行失败：…」，仍含后端子串。
 */

/**
 * 后端稳定错误串特征子串。任一命中即判定为「模型未配置」类：
 * - 「模型服务缺少 …」：`validate_provider_completeness`（缺 API Key / Base URL / 模型名）
 * - 「LLM API Key 未配置」：plain_chat / project adapter 的 provider_missing_message
 * - 「未配置视觉模型」：architecture adapter 的 provider_missing_message
 * - 「URL 或模型名为空」：build_architecture_agent 显式条目不完整分支
 *
 * 与后端串同源（同一仓库），改动后端错误文案时须同步这里与测试。
 */
const MODEL_NOT_CONFIGURED_MARKERS = [
  "模型服务缺少",
  "LLM API Key 未配置",
  "未配置视觉模型",
  "URL 或模型名为空",
] as const;

/**
 * 判定运行/发送错误是否属于「模型未配置或配置不完整」类。
 * 命中即可在 UI 侧提供「配置模型」深链。空串/普通网络错误返回 false。
 */
export function isModelNotConfiguredError(text: string | null | undefined): boolean {
  if (!text) return false;
  return MODEL_NOT_CONFIGURED_MARKERS.some((marker) => text.includes(marker));
}
