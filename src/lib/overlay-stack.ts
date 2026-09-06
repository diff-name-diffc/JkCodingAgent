/**
 * 全局覆盖层层级栈：协调多个并存自研覆盖层（执行图、详情面板等）的 Escape
 * 与焦点归属。覆盖层 mount 时 push、unmount 时 pop；Escape 处理者仅在自己是
 * 栈顶时响应，避免一次按键同时关掉多层。
 *
 * Radix 内部弹层（Dialog/Select/Popover）走 DismissableLayer 自己的分支栈，
 * 不进本栈；它们与自研覆盖层共存时由 defaultPrevented 检查兜底让路。
 */
const stack: string[] = [];

export function pushOverlay(id: string): void {
  const index = stack.indexOf(id);
  if (index !== -1) stack.splice(index, 1);
  stack.push(id);
}

export function popOverlay(id: string): void {
  const index = stack.indexOf(id);
  if (index !== -1) stack.splice(index, 1);
}

export function isTopOverlay(id: string): boolean {
  return stack.length > 0 && stack[stack.length - 1] === id;
}

/** 是否有自研覆盖层打开（底层快捷键据此让路）。 */
export function hasOpenOverlay(): boolean {
  return stack.length > 0;
}
