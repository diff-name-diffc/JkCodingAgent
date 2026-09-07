import { describe, expect, it } from "vitest";
import {
  hasOpenOverlay,
  isTopOverlay,
  peekOverlay,
  popOverlay,
  pushOverlay,
  shouldHandleEscape,
} from "./overlay-stack";

// 模块级栈是单例：每个用例先清空，保证隔离。
function reset() {
  for (const id of ["a", "b", "c", "command-palette", "graph-node-drawer"]) {
    popOverlay(id);
  }
}

describe("overlay-stack 栈语义", () => {
  it("空栈：peek=null / hasOpen=false / isTop=false", () => {
    reset();
    expect(peekOverlay()).toBeNull();
    expect(hasOpenOverlay()).toBe(false);
    expect(isTopOverlay("a")).toBe(false);
  });

  it("push 后成为栈顶；pop 后回退到下层", () => {
    reset();
    pushOverlay("a");
    pushOverlay("b");
    expect(peekOverlay()).toBe("b");
    expect(isTopOverlay("a")).toBe(false);
    popOverlay("b");
    expect(peekOverlay()).toBe("a");
    expect(hasOpenOverlay()).toBe(true);
    popOverlay("a");
    expect(hasOpenOverlay()).toBe(false);
  });

  it("重复 push 同 id：去重并置顶", () => {
    reset();
    pushOverlay("a");
    pushOverlay("b");
    pushOverlay("a");
    expect(peekOverlay()).toBe("a");
    popOverlay("a");
    expect(peekOverlay()).toBe("b");
    reset();
  });

  it("pop 非栈顶 id：只移除该 id，不影响其余顺序", () => {
    reset();
    pushOverlay("a");
    pushOverlay("b");
    pushOverlay("c");
    popOverlay("b");
    expect(peekOverlay()).toBe("c");
    popOverlay("c");
    expect(peekOverlay()).toBe("a");
    reset();
  });

  it("pop 不存在的 id：无副作用", () => {
    reset();
    pushOverlay("a");
    popOverlay("not-there");
    expect(peekOverlay()).toBe("a");
    reset();
  });
});

describe("shouldHandleEscape（Escape 统一裁决）", () => {
  it("栈顶且未被处理 → 响应", () => {
    expect(shouldHandleEscape("a", "a", false)).toBe(true);
  });

  it("非栈顶 → 让路", () => {
    expect(shouldHandleEscape("b", "a", false)).toBe(false);
  });

  it("已被内层 preventDefault → 让路", () => {
    expect(shouldHandleEscape("a", "a", true)).toBe(false);
  });

  it("空栈 → 不响应（无覆盖层可关）", () => {
    expect(shouldHandleEscape(null, "a", false)).toBe(false);
  });
});

describe("「一键双关」回归场景（UI-23b）", () => {
  it("命令面板入栈后：面板响应 Escape，底层快捷键（hasOpenOverlay）让路", () => {
    reset();
    // 场景：Artifact 面板开着（底层，不 push——用 hasOpenOverlay 让路），
    // 命令面板打开并 push 自己。
    pushOverlay("command-palette");
    // 底层 use-chat-shortcuts 的 Escape 分支：hasOpenOverlay() 为真 → 让路，
    // 不再出现「一次按键同时关面板与 Artifact」的双关。
    expect(hasOpenOverlay()).toBe(true);
    // 命令面板自身：栈顶且未被处理 → 响应。
    expect(shouldHandleEscape(peekOverlay(), "command-palette", false)).toBe(true);
    popOverlay("command-palette");
    // 面板关闭后：栈空，下一次 Escape 归还底层（关 Artifact）。
    expect(hasOpenOverlay()).toBe(false);
    expect(shouldHandleEscape(peekOverlay(), "command-palette", false)).toBe(false);
  });

  it("执行图节点抽屉在 Artifact 之上：只有抽屉响应 Escape", () => {
    reset();
    pushOverlay("graph-node-drawer");
    expect(shouldHandleEscape(peekOverlay(), "graph-node-drawer", false)).toBe(true);
    expect(hasOpenOverlay()).toBe(true);
    reset();
  });
});
