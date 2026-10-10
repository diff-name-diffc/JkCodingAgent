import type { ToolActivityItem } from "../dispatcher-chat/tool-activity";
import { parseWorkflowPlanId } from "../workflow/workflow-utils";

const RECENT_ACTIVITY_COUNT = 3;

type ToolPresentation = { action: string; target?: string };
type ToolDescriptor = readonly [action: string, ...targetKeys: string[]];

const TOOL_DESCRIPTORS: Record<string, ToolDescriptor> = {
  read_file: ["读取文件", "path", "paths", "file_path"],
  list_dir: ["浏览目录", "path", "paths"],
  write_file: ["写入文件", "path", "file_path"],
  edit_file: ["编辑文件", "path", "file_path"],
  grep: ["搜索内容", "pattern", "patterns"],
  glob: ["查找文件", "pattern", "patterns"],
  exec: ["执行命令", "command"],
  local_zsh: ["执行命令", "command"],
  ssh_exec: ["执行远程命令", "command"],
  sync_directory: ["同步目录", "source"],
  call_sub_agent: ["委托子智能体", "task", "prompt", "agent_id"],
  list_sub_agents: ["查看可用智能体"],
  notify_user_progress: ["更新进展", "message"],
  run_tool_program: ["执行工具程序", "description"],
  submit_workflow: ["生成工作流", "title"],
  workflow_get: ["查看工作流", "planId"],
  workflow_plan_report: ["汇报工作流", "planId"],
  workflow_result_read: ["读取工作流结果", "planId"],
  workflow_node_update: ["更新工作流步骤", "nodeId"],
  workflow_node_add: ["添加工作流步骤", "nodeId"],
  workflow_node_delete: ["删除工作流步骤", "nodeId"],
  generate_image: ["生成图片", "prompt"],
  edit_image: ["编辑图片", "prompt", "image_path"],
  analyze_image: ["分析图片", "question", "image_path"],
  fetch_image: ["下载图片", "url"],
  browser_open_url: ["打开网页", "url"],
  browser_click: ["点击页面元素", "selector", "ref"],
  browser_type: ["填写页面内容", "selector", "ref"],
  browser_press: ["操作页面", "key"],
  browser_wait_for: ["等待页面", "selector", "text"],
  browser_close: ["关闭网页"],
  browser_read_text: ["读取网页", "selector"],
  browser_visual_analyze: ["查看页面画面", "question"],
  ssh_list_servers: ["查看服务器"],
  ssh_tmux_install: ["安装 tmux", "server_id"],
  ssh_term_open: ["打开远程终端", "command", "server_id"],
  ssh_term_send: ["操作远程终端", "term_id"],
  ssh_term_read: ["读取终端输出", "term_id"],
  ssh_term_resize: ["调整终端尺寸", "term_id"],
  ssh_term_close: ["关闭远程终端", "term_id"],
  ssh_term_list: ["查看远程终端", "server_id"],
  ssh_memo_read: ["读取服务器备忘", "server_id"],
  ssh_memo_upsert: ["更新服务器备忘", "server_id"],
  ssh_memo_delete: ["删除服务器备忘", "server_id"],
  wait_for_tools: ["等待工具任务"],
  message: ["发送回复"],
};

/** 只用工具协议中的动作和输入生成摘要，不从输出推测工作成果。 */
export function presentToolActivity(
  item: Pick<ToolActivityItem, "name" | "input">,
): ToolPresentation {
  const descriptor = Object.prototype.hasOwnProperty.call(TOOL_DESCRIPTORS, item.name)
    ? TOOL_DESCRIPTORS[item.name]
    : undefined;
  const [action, ...keys] = descriptor ?? [
    item.name.startsWith("mcp__") ? "访问外部工具" : "调用工具",
    "path",
    "query",
    "pattern",
    "url",
    "description",
  ];
  const input = parseInput(item.input);
  const definition = parseInput(input.definition);
  const target = keys.reduce<string | undefined>(
    (found, key) => found ?? summarizeInput(input[key], key),
    undefined,
  );
  return {
    action,
    target:
      target ?? (item.name === "submit_workflow" ? summarizeInput(definition.title) : undefined),
  };
}

function parseInput(value: unknown): Record<string, unknown> {
  if (typeof value === "string") {
    try {
      value = JSON.parse(value) as unknown;
    } catch {
      // 流式参数可能尚未形成完整 JSON，保留动作即可，完整原文仍在详情中。
      return {};
    }
  }
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
}

function summarizeInput(value: unknown, key?: string): string | undefined {
  if (Array.isArray(value)) {
    const strings = value.filter(
      (entry): entry is string => typeof entry === "string" && !!entry.trim(),
    );
    if (!strings.length) return undefined;
    return `${compactText(strings[0], 90)}${strings.length > 1 ? ` 等 ${strings.length} 项` : ""}`;
  }
  if (typeof value !== "string" || !value.trim()) return undefined;
  let target = value;
  if (key === "url") {
    try {
      const url = new URL(value);
      // 概览只展示定位信息，查询串、片段和 URL 凭据留在用户主动打开的输入详情中。
      target =
        url.protocol === "http:" || url.protocol === "https:"
          ? `${url.host}${url.pathname === "/" ? "" : url.pathname}`
          : `${url.protocol}${url.pathname}`;
    } catch {
      return compactText(value, 100);
    }
  }
  return compactText(target, 100);
}

function compactText(value: string, limit: number): string {
  const singleLine = value.trim().replace(/\s+/g, " ");
  return singleLine.length > limit ? `${singleLine.slice(0, limit - 1)}…` : singleLine;
}

export function toolActivityErrorSummary(item: ToolActivityItem): string | undefined {
  if (item.status !== "error") return undefined;
  const error = item.errorText ?? (typeof item.output === "string" ? item.output : undefined);
  return error?.trim() ? compactText(error, 180) : "这一步未能完成，展开查看详情";
}

export function toolWorkflowPlanId(item: ToolActivityItem): string | null {
  return item.name === "submit_workflow" && typeof item.output === "string"
    ? parseWorkflowPlanId(item.output)
    : null;
}

/** 最近活动保持连续可见；异常、待处理、工作流入口与用户正阅读的详情不会被挤走。 */
export function selectVisibleToolActivities(
  items: readonly ToolActivityItem[],
  showAll: boolean,
  expandedIds: ReadonlySet<string> = new Set(),
): readonly ToolActivityItem[] {
  if (showAll) return items;
  const recentStart = Math.max(0, items.length - RECENT_ACTIVITY_COUNT);
  return items.filter(
    (item, index) =>
      index >= recentStart ||
      item.planned ||
      item.status !== "success" ||
      expandedIds.has(item.id) ||
      toolWorkflowPlanId(item) !== null,
  );
}

export function formatToolDuration(durationMs: number): string {
  if (durationMs < 1000) return `${Math.round(durationMs)}ms`;
  return `${(durationMs / 1000).toFixed(1)}s`;
}
