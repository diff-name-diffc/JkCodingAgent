import { describe, expect, it } from "vitest";
import {
  CODE_EDITOR_CONTAINER_SELECTOR,
  TERMINAL_CONTAINER_SELECTOR,
  isImeKeyEvent,
  isInsideContainer,
  isMacPlatform,
  isTypingTarget,
  matchesBinding,
  shouldSkipBinding,
  type EventTargetLike,
  type KeyEventLike,
  type ShortcutModifiers,
} from "./keyboard-bindings";

// ── 测试桩 ────────────────────────────────────────────────────────────────

/** 构造带 closest 祖先链的事件目标桩：containers 为命中的选择器集合。 */
function targetStub(opts: {
  tagName?: string;
  isContentEditable?: boolean;
  containers?: string[];
}): EventTargetLike {
  const containers = opts.containers ?? [];
  return {
    tagName: opts.tagName,
    isContentEditable: opts.isContentEditable,
    closest: (selector: string) => (containers.includes(selector) ? {} : null),
  };
}

function keyEvent(overrides: Partial<KeyEventLike> = {}): KeyEventLike {
  return { key: "k", ...overrides };
}

const MOD_K: ShortcutModifiers = { key: "k", mod: true };
const PLAIN_ESCAPE: ShortcutModifiers = { key: "Escape", mod: false };

// ── isMacPlatform ─────────────────────────────────────────────────────────

describe("isMacPlatform", () => {
  it("Mac/iPhone/iPad 平台判为 mac", () => {
    expect(isMacPlatform({ platform: "MacIntel" })).toBe(true);
    expect(isMacPlatform({ platform: "iPhone" })).toBe(true);
    expect(isMacPlatform({ platform: "", userAgent: "Mozilla/5.0 (iPad)" })).toBe(true);
  });

  it("Windows/Linux 判为非 mac", () => {
    expect(isMacPlatform({ platform: "Win32" })).toBe(false);
    expect(isMacPlatform({ platform: "Linux x86_64" })).toBe(false);
  });

  it('Node 运行时 userAgent（"Node.js/22.x"）不被裸 Mac 子串误判', () => {
    expect(isMacPlatform({ userAgent: "Node.js/22.14.0" })).toBe(false);
  });

  it("userAgent 形态的 mac 判定（platform 为空时兜底）", () => {
    expect(
      isMacPlatform({ userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)" }),
    ).toBe(true);
  });

  it("平台信息缺失时按非 mac（保守回退，不读宿主 navigator）", () => {
    expect(isMacPlatform({})).toBe(false);
  });
});

// ── isImeKeyEvent ─────────────────────────────────────────────────────────

describe("isImeKeyEvent", () => {
  it("isComposing=true 判定组合期", () => {
    expect(isImeKeyEvent(keyEvent({ isComposing: true }))).toBe(true);
  });

  it("keyCode/which 229 判定组合期（macOS 中文输入法候选确认）", () => {
    expect(isImeKeyEvent(keyEvent({ keyCode: 229 }))).toBe(true);
    expect(isImeKeyEvent(keyEvent({ which: 229 }))).toBe(true);
  });

  it('key === "Process" 判定组合期', () => {
    expect(isImeKeyEvent(keyEvent({ key: "Process" }))).toBe(true);
  });

  it("普通按键不算组合期", () => {
    expect(isImeKeyEvent(keyEvent())).toBe(false);
  });
});

// ── matchesBinding ────────────────────────────────────────────────────────

describe("matchesBinding", () => {
  it("mac 下 Mod 映射到 metaKey", () => {
    expect(matchesBinding(keyEvent({ metaKey: true }), MOD_K, true)).toBe(true);
    expect(matchesBinding(keyEvent({ ctrlKey: true }), MOD_K, true)).toBe(false);
  });

  it("非 mac 下 Mod 映射到 ctrlKey", () => {
    expect(matchesBinding(keyEvent({ ctrlKey: true }), MOD_K, false)).toBe(true);
    expect(matchesBinding(keyEvent({ metaKey: true }), MOD_K, false)).toBe(false);
  });

  it("mod:false 要求修饰键未按下", () => {
    expect(matchesBinding(keyEvent({ key: "Escape" }), PLAIN_ESCAPE, true)).toBe(true);
    expect(
      matchesBinding(keyEvent({ key: "Escape", metaKey: true }), PLAIN_ESCAPE, true),
    ).toBe(false);
  });

  it("shift/alt 精确匹配", () => {
    const binding: ShortcutModifiers = { key: "a", mod: true, shift: true };
    expect(
      matchesBinding(keyEvent({ key: "A", metaKey: true, shiftKey: true }), binding, true),
    ).toBe(true);
    expect(matchesBinding(keyEvent({ key: "a", metaKey: true }), binding, true)).toBe(false);
  });

  it("单字符键大小写归一，命名键原样比较", () => {
    expect(matchesBinding(keyEvent({ key: "K", metaKey: true }), MOD_K, true)).toBe(true);
    expect(matchesBinding(keyEvent({ key: "escape" }), PLAIN_ESCAPE, true)).toBe(false);
    expect(matchesBinding(keyEvent({ key: "Escape" }), PLAIN_ESCAPE, true)).toBe(true);
  });
});

// ── isTypingTarget / isInsideContainer ────────────────────────────────────

describe("isTypingTarget", () => {
  it("input/textarea/contentEditable 判为输入目标", () => {
    expect(isTypingTarget(targetStub({ tagName: "INPUT" }))).toBe(true);
    expect(isTypingTarget(targetStub({ tagName: "TEXTAREA" }))).toBe(true);
    expect(isTypingTarget(targetStub({ tagName: "DIV", isContentEditable: true }))).toBe(true);
  });

  it("普通元素与 null 不是输入目标", () => {
    expect(isTypingTarget(targetStub({ tagName: "DIV" }))).toBe(false);
    expect(isTypingTarget(null)).toBe(false);
    expect(isTypingTarget(undefined)).toBe(false);
  });
});

describe("isInsideContainer", () => {
  it("closest 命中选择器时判定容器内", () => {
    expect(
      isInsideContainer(
        targetStub({ tagName: "TEXTAREA", containers: [TERMINAL_CONTAINER_SELECTOR] }),
        TERMINAL_CONTAINER_SELECTOR,
      ),
    ).toBe(true);
  });

  it("无 closest 能力（如 document）时安全返回 false", () => {
    expect(isInsideContainer({ tagName: "DIV" }, CODE_EDITOR_CONTAINER_SELECTOR)).toBe(false);
    expect(isInsideContainer(null, CODE_EDITOR_CONTAINER_SELECTOR)).toBe(false);
  });
});

// ── shouldSkipBinding（统一让路裁决）───────────────────────────────────────

describe("shouldSkipBinding", () => {
  it("IME 组合期全跳过（含 Mod 组合）", () => {
    expect(shouldSkipBinding(keyEvent({ isComposing: true }), MOD_K, true)).toBe(true);
    expect(shouldSkipBinding(keyEvent({ keyCode: 229 }), MOD_K, false)).toBe(true);
  });

  it("输入目标内：非 Mod 键跳过，Mod 键放行（既有豁免语义）", () => {
    const input = targetStub({ tagName: "TEXTAREA" });
    expect(shouldSkipBinding(keyEvent({ key: "Escape" }), PLAIN_ESCAPE, true)).toBe(false);
    expect(
      shouldSkipBinding(keyEvent({ key: "Escape", target: input }), PLAIN_ESCAPE, true),
    ).toBe(true);
    expect(
      shouldSkipBinding(keyEvent({ metaKey: true, target: input }), MOD_K, true),
    ).toBe(false);
  });

  it("非 Mac + 终端内 + Mod 键 → 放行给 shell（Ctrl+K/L/N/J 不被抢）", () => {
    const xtermTextarea = targetStub({
      tagName: "TEXTAREA",
      containers: [TERMINAL_CONTAINER_SELECTOR],
    });
    for (const key of ["k", "l", "n", "j"]) {
      expect(
        shouldSkipBinding(keyEvent({ key, ctrlKey: true, target: xtermTextarea }), { key, mod: true }, false),
      ).toBe(true);
    }
  });

  it("Mac + 终端内 + Cmd 键 → 应用级快捷键照常触发（Cmd 不进终端）", () => {
    const xtermTextarea = targetStub({
      tagName: "TEXTAREA",
      containers: [TERMINAL_CONTAINER_SELECTOR],
    });
    expect(
      shouldSkipBinding(keyEvent({ metaKey: true, target: xtermTextarea }), MOD_K, true),
    ).toBe(false);
  });

  it("终端内的裸键仍按输入目标豁免（xterm-helper-textarea）", () => {
    const xtermTextarea = targetStub({
      tagName: "TEXTAREA",
      containers: [TERMINAL_CONTAINER_SELECTOR],
    });
    expect(
      shouldSkipBinding(keyEvent({ key: "Escape", target: xtermTextarea }), PLAIN_ESCAPE, false),
    ).toBe(true);
  });

  it("Monaco 内：allowInEditor 默认 true 应用级键放行", () => {
    const monacoTextarea = targetStub({
      tagName: "TEXTAREA",
      containers: [CODE_EDITOR_CONTAINER_SELECTOR],
    });
    expect(
      shouldSkipBinding(keyEvent({ metaKey: true, target: monacoTextarea }), MOD_K, true),
    ).toBe(false);
  });

  it("Monaco 内：allowInEditor=false 的绑定让路给编辑器", () => {
    const monacoTextarea = targetStub({
      tagName: "TEXTAREA",
      containers: [CODE_EDITOR_CONTAINER_SELECTOR],
    });
    expect(
      shouldSkipBinding(
        keyEvent({ metaKey: true, target: monacoTextarea }),
        { ...MOD_K, allowInEditor: false },
        true,
      ),
    ).toBe(true);
  });

  it("普通焦点上下文不跳过", () => {
    expect(shouldSkipBinding(keyEvent({ metaKey: true, target: targetStub({ tagName: "DIV" }) }), MOD_K, true)).toBe(false);
    expect(shouldSkipBinding(keyEvent({ metaKey: true }), MOD_K, false)).toBe(false);
  });
});

// ── 键位注册表无歧义门禁（UI-23d）──────────────────────────────────────────
//
// 镜像 use-chat-shortcuts（Mod+K/N/B/L、Mod+Shift+A、Escape）与 ProjectPage
// 工作区键位（Mod+1..4、Mod+J）的应用级注册表。新增/删改键位必须同步此清单——
// 本用例穷举平台×修饰键组合，保证任一按键事件至多命中一个绑定（冲突审查的
// 自动化门禁）。

const APP_SHORTCUT_REGISTRY: ShortcutModifiers[] = [
  { key: "k", mod: true },
  { key: "n", mod: true },
  { key: "b", mod: true },
  { key: "l", mod: true },
  { key: "a", mod: true, shift: true },
  { key: "Escape", mod: false },
  { key: "1", mod: true },
  { key: "2", mod: true },
  { key: "3", mod: true },
  { key: "4", mod: true },
  { key: "j", mod: true },
];

describe("应用级键位注册表无歧义", () => {
  const KEYS = ["a", "b", "j", "k", "l", "n", "1", "2", "3", "4", "Escape"];

  it("穷举平台×修饰键组合：任一事件至多命中一个绑定", () => {
    for (const mac of [true, false]) {
      for (const key of KEYS) {
        for (const mod of [false, true]) {
          for (const shift of [false, true]) {
            for (const alt of [false, true]) {
              const event = keyEvent({
                key,
                metaKey: mac && mod,
                ctrlKey: !mac && mod,
                shiftKey: shift,
                altKey: alt,
              });
              const hits = APP_SHORTCUT_REGISTRY.filter((binding) =>
                matchesBinding(event, binding, mac),
              );
              expect(
                hits.length,
                `mac=${mac} key=${key} mod=${mod} shift=${shift} alt=${alt} 命中 ${hits.length} 个绑定`,
              ).toBeLessThanOrEqual(1);
            }
          }
        }
      }
    }
  });

  it("注册表内每个绑定都可达（存在唯一命中的事件）", () => {
    for (const mac of [true, false]) {
      for (const binding of APP_SHORTCUT_REGISTRY) {
        const event = keyEvent({
          key: binding.key,
          metaKey: mac && binding.mod === true,
          ctrlKey: !mac && binding.mod === true,
          shiftKey: binding.shift === true,
        });
        const hits = APP_SHORTCUT_REGISTRY.filter((other) =>
          matchesBinding(event, other, mac),
        );
        expect(hits).toEqual([binding]);
      }
    }
  });
});
