/**
 * 各聊天入口的领域化空态文案（UI-25 A06）。
 *
 * 三个主入口分别有贴合语境的起步提示，不再共用同一套通用空态：
 *   - plain（普通聊天）：独立聊天工作区，不绑定项目——通用问答/写作/贴代码分析。
 *   - project（项目）：绑定项目仓库——可读写项目文件、看 Git 变更、产出执行图。
 *   - architecture（架构）：绘图任务示例，见 ArchitectureChatPanel 的 ARCH_EMPTY_STATE。
 *
 * 纯数据 + 纯函数，无组件依赖，便于单测与复用。
 */

/** 空态文案形状（标题 / 副文案 / 起步提示）。与 MessageList 的 emptyState 契约一致。 */
export interface ChatEmptyStateContent {
  title?: string;
  copy?: string;
  prompts?: string[];
}

/** 走 ChatShell → MessageList 的两个聊天入口；架构入口自带空态，不经此函数。 */
export type ChatEntryKind = "plain" | "project";

/** 普通聊天：不绑定项目，起步提示偏向通用问答 / 写作 / 代码解释。 */
export const PLAIN_CHAT_EMPTY_STATE: Required<ChatEmptyStateContent> = {
  title: "有什么可以帮你的？",
  copy: "独立聊天工作区，不绑定项目。提问、写作、贴代码分析 —— 模型会在这里推理并给出结果。",
  prompts: [
    "解释一下 React 19 的 use() hook 和 Suspense 的关系",
    "把这段 SQL 优化一下，并解释为什么更快",
    "写一个 Python 脚本批量处理文件，并解释每一步",
    "帮我润色一段中文文案，让它更简洁专业",
  ],
};

/** 项目会话：绑定项目仓库，起步提示偏向项目文件 / Git 变更 / 执行图。 */
export const PROJECT_CHAT_EMPTY_STATE: Required<ChatEmptyStateContent> = {
  title: "在这个项目里开始一个任务",
  copy: "Agent 可以读取与修改项目文件、运行命令、产出执行图，并在这里汇总改了什么、如何验证。",
  prompts: [
    "梳理这个项目的目录结构，说明主要模块与职责",
    "审查当前 Git 变更，总结改了什么、有哪些风险",
    "为最近改动的代码补充单元测试并运行验证",
    "把这个需求拆成执行图，分步实现并自检",
  ],
};

/**
 * 按入口类型返回领域化空态文案。
 *
 * 未知/缺省类型回退到普通聊天文案（保守默认，绝不返回空对象）。
 */
export function resolveChatEmptyState(kind: ChatEntryKind): ChatEmptyStateContent {
  return kind === "project" ? PROJECT_CHAT_EMPTY_STATE : PLAIN_CHAT_EMPTY_STATE;
}
