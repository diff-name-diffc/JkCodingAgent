/**
 * 命令审查「需用户确认」弹窗的前端队列（模块级单例，仿 Toast 的无 Context 模式）。
 *
 * 后端命令类工具在审查不通过、需人工放行时登记一次性请求并 emit
 * `tool-confirm-request`；本 store 接收入队，由 `ToolConfirmDialog` 展示；
 * 用户点击「允许执行 / 拒绝」后经 `invoke("tool_confirm_resolve")` 回传裁决。
 * 多个待确认请求按到达顺序排队展示（一次只弹一个，避免遮挡）。
 */

export interface ToolConfirmRequest {
  /** 后端登记的一次性请求 id，回传时的凭据。 */
  requestId: string;
  /** 发起调用的会话（回传时后端做 workspace 域校验）。 */
  workspaceId: string;
  sessionId: string;
  /** 工具名：ssh_exec / local_zsh / sync_directory / MCP 工具名 / 通用工具名。 */
  tool: string;
  /** 目标的简短人类可读标签（服务器 id / 本地 zsh / 工具名等）。 */
  target: string;
  /** 待执行的命令或参数文本。 */
  command: string;
  /** 审查给出的拦截原因。 */
  reason: string;
  /** 是否提权（sudo）命令。 */
  elevated: boolean;
}

let pending: ToolConfirmRequest[] = [];
const listeners = new Set<() => void>();

function emit() {
  listeners.forEach((listener) => listener());
}

export function subscribeToolConfirms(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function getPendingToolConfirms(): ToolConfirmRequest[] {
  return pending;
}

/** 入队一个确认请求；同 requestId 重复到达时覆盖（后端重发语义）。 */
export function enqueueToolConfirm(request: ToolConfirmRequest): void {
  pending = [...pending.filter((item) => item.requestId !== request.requestId), request];
  emit();
}

/** 出队指定请求（用户裁决或后端侧已超时清槽后清理）。 */
export function removeToolConfirm(requestId: string): void {
  const next = pending.filter((item) => item.requestId !== requestId);
  if (next.length === pending.length) return;
  pending = next;
  emit();
}

/** 测试用：重置队列。 */
export function resetToolConfirms(): void {
  pending = [];
  emit();
}
