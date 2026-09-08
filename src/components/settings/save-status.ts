/**
 * 设置自动保存的持续状态派生（UI-21）。
 *
 * store 已暴露 loading / dirty(=pending||saving) / saveError，本函数把这三者
 * 收敛成头部指示器的单一语义状态，供 `SaveStatusIndicator` 消费。纯函数，无副作用。
 *
 * 优先级：loading > error > saving > saved。
 * - error 先于 saving：保存失败后用户再次编辑会清空 saveError 并置 dirty，
 *   届时自然回到 saving；二者实践中互斥，error 优先保证失败不被「保存中」掩盖。
 * - 非 dirty 且无错误即「已保存」：设置从磁盘加载即与存储同步，初次未编辑也成立。
 */
export type SaveStatus = "loading" | "saving" | "saved" | "error";

/**
 * 保存源的模式（UI-21 遗留领取，供 save-sources 注册表使用）：
 * - auto：debounce 自动保存——dirty 语义为「保存中」（等待中的 timer 或进行中的保存）；
 * - manual：手动保存（如 RAG 配置页）——dirty 语义为「未保存的修改」，
 *   指示器显示「未保存」而非谎报「保存中」。
 */
export type SaveSourceMode = "auto" | "manual";

export function deriveSaveStatus(input: {
  loading: boolean;
  dirty: boolean;
  hasError: boolean;
}): SaveStatus {
  if (input.loading) return "loading";
  if (input.hasError) return "error";
  if (input.dirty) return "saving";
  return "saved";
}
