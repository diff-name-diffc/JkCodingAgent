import { describe, expect, it } from "vitest";
import {
  DEFAULT_CHROME,
  editorRatioFromWidths,
  resolveWorkspaceBudget,
  splitDualPaneWidths,
  type WorkspaceBudgetInput,
} from "./workspace-budget";

const WIDTHS = [1000, 1280, 1440, 1600, 1920] as const;
const HEIGHTS = [680, 800, 900, 1000, 1080] as const;

function base(overrides: Partial<WorkspaceBudgetInput> = {}): WorkspaceBudgetInput {
  return {
    viewportWidth: 1440,
    viewportHeight: 900,
    navOpen: true,
    navWidthPref: 288,
    rightPanelOpen: false,
    rightPanelWidthPref: 280,
    terminalOpen: false,
    terminalHeightPref: 240,
    dualPaneRequested: false,
    editorRatioPref: 0.5,
    ...overrides,
  };
}

describe("resolveWorkspaceBudget 不变式", () => {
  it.each(WIDTHS.flatMap((w) => HEIGHTS.map((h) => [w, h] as const)))(
    "%i×%i 全开关组合无负宽且水平预算闭合",
    (w, h) => {
      for (const navOpen of [true, false]) {
        for (const rightPanelOpen of [true, false]) {
          for (const terminalOpen of [true, false]) {
            for (const dual of [true, false]) {
              const b = resolveWorkspaceBudget(
                base({
                  viewportWidth: w,
                  viewportHeight: h,
                  navOpen,
                  rightPanelOpen,
                  terminalOpen,
                  dualPaneRequested: dual,
                }),
              );
              expect(b.navWidth).toBeGreaterThanOrEqual(0);
              expect(b.rightPanelWidth).toBeGreaterThanOrEqual(0);
              expect(b.terminalHeight).toBeGreaterThanOrEqual(0);
              expect(b.mainWidth).toBeGreaterThanOrEqual(0);
              expect(b.chatWidth).toBeGreaterThanOrEqual(0);
              expect(b.editorWidth).toBeGreaterThanOrEqual(0);
              // 水平闭合：rail + nav + main + right + toolbar = viewport
              expect(
                b.railWidth + b.navWidth + b.mainWidth + b.rightPanelWidth + b.toolbarWidth,
              ).toBe(w);
              // 双栏时 chat + splitter + editor = main
              if (b.dualPane) {
                expect(b.chatWidth + b.splitterWidth + b.editorWidth).toBe(b.mainWidth);
                expect(b.chatWidth).toBeGreaterThanOrEqual(DEFAULT_CHROME.minChatWidth);
                expect(b.editorWidth).toBeGreaterThanOrEqual(DEFAULT_CHROME.minEditorWidth);
              } else {
                expect(b.chatWidth).toBe(b.mainWidth);
                expect(b.editorWidth).toBe(0);
              }
              // 垂直：终端开启时主区高 + 终端 + 顶栏 = viewport
              if (terminalOpen) {
                expect(b.mainHeight + b.terminalHeight + DEFAULT_CHROME.titlebarHeight).toBe(h);
              }
            }
          }
        }
      }
    },
  );

  it("偏好是冻结输入：窄窗临时适配不修改偏好值", () => {
    const input = base({
      viewportWidth: 1000,
      rightPanelOpen: true,
      rightPanelWidthPref: 600,
      navWidthPref: 320,
    });
    const frozen = { ...input };
    resolveWorkspaceBudget(input);
    expect(input).toEqual(frozen);
  });
});

describe("resolveWorkspaceBudget 降级树", () => {
  it("1000px 开右面板：右面板临时关闭、双栏转单栏", () => {
    const b = resolveWorkspaceBudget(
      base({ viewportWidth: 1000, rightPanelOpen: true, dualPaneRequested: true }),
    );
    expect(b.rightPanelWidth).toBe(0);
    expect(b.navWidth).toBe(288);
    expect(b.mainWidth).toBe(1000 - 56 - 48 - 288);
    expect(b.dualPane).toBe(false);
    expect(b.degradations.map((d) => d.kind)).toEqual([
      "right-panel-closed",
      "single-column",
    ]);
  });

  it("760px：先收导航，右面板压缩到下限", () => {
    const b = resolveWorkspaceBudget(base({ viewportWidth: 760, rightPanelOpen: true }));
    expect(b.navWidth).toBe(0);
    expect(b.rightPanelWidth).toBe(DEFAULT_CHROME.rightPanelMin);
    expect(b.degradations.map((d) => d.kind)).toEqual([
      "nav-collapsed",
      "right-panel-clamped",
    ]);
  });

  it("620px：导航与右面板均临时关闭，主区独占且不产生负宽", () => {
    const b = resolveWorkspaceBudget(base({ viewportWidth: 620, rightPanelOpen: true }));
    expect(b.navWidth).toBe(0);
    expect(b.rightPanelWidth).toBe(0);
    expect(b.mainWidth).toBe(620 - DEFAULT_CHROME.railWidth - DEFAULT_CHROME.toolbarWidth);
    expect(b.degradations.map((d) => d.kind)).toEqual([
      "nav-collapsed",
      "right-panel-closed",
    ]);
  });

  it("矮窗口：终端高度被压缩以保住主区 320px 下限", () => {
    const b = resolveWorkspaceBudget(
      base({ viewportHeight: 680, terminalOpen: true, terminalHeightPref: 600 }),
    );
    expect(b.terminalHeight).toBeLessThan(600);
    expect(b.mainHeight).toBeGreaterThanOrEqual(DEFAULT_CHROME.minMainHeight);
    expect(b.degradations.map((d) => d.kind)).toContain("terminal-height-clamped");
  });

  it("宽屏完整双栏锚点（对齐 02-design §3.2 样例）", () => {
    const b = resolveWorkspaceBudget(
      base({
        viewportWidth: 1600,
        navWidthPref: 248,
        dualPaneRequested: true,
        editorRatioPref: 0.5,
        chrome: { railWidth: 52, toolbarWidth: 0 },
      }),
    );
    // 52 + 248 + main(1300) = 1600；双栏内 460≤chat/editor 且和为 1292
    expect(b.mainWidth).toBe(1600 - 52 - 248);
    expect(b.dualPane).toBe(true);
    expect(b.chatWidth + b.splitterWidth + b.editorWidth).toBe(b.mainWidth);
  });
});

describe("editorRatioFromWidths", () => {
  it("由像素反推占比并夹取 0–1", () => {
    expect(editorRatioFromWidths(460, 520, 8)).toBeCloseTo(520 / 988, 5);
    expect(editorRatioFromWidths(0, 0, 0)).toBe(0.5);
  });
});

describe("splitDualPaneWidths（UI-24a-4 拖拽瞬时宽度）", () => {
  it("中间占比：按 inner 分配，和不变", () => {
    const result = splitDualPaneWidths(700, 700, 0.5);
    expect(result.editorWidth).toBe(700);
    expect(result.chatWidth).toBe(700);
  });

  it("钳制到 minEditorWidth=520（占比过小）", () => {
    const result = splitDualPaneWidths(700, 700, 0.1);
    expect(result.editorWidth).toBe(520);
    expect(result.chatWidth).toBe(1400 - 520);
  });

  it("钳制到 inner - minChatWidth=460（占比过大）", () => {
    const result = splitDualPaneWidths(700, 700, 0.95);
    expect(result.editorWidth).toBe(1400 - 460);
    expect(result.chatWidth).toBe(460);
  });

  it("ratio 越界先夹取 0..1；宽度和恒守恒", () => {
    for (const ratio of [-0.5, 0, 0.3, 0.7, 1, 1.5]) {
      const result = splitDualPaneWidths(600, 800, ratio);
      expect(result.chatWidth + result.editorWidth).toBe(1400);
      expect(result.editorWidth).toBeGreaterThanOrEqual(0);
    }
  });

  it("退化 inner=0：不产生负值/NaN", () => {
    const result = splitDualPaneWidths(0, 0, 0.5);
    expect(result).toEqual({ chatWidth: 0, editorWidth: 0 });
  });

  it("自定义 chrome 钳制口径生效", () => {
    const result = splitDualPaneWidths(500, 500, 0.2, {
      minChatWidth: 200,
      minEditorWidth: 300,
    });
    expect(result.editorWidth).toBe(300);
    expect(result.chatWidth).toBe(700);
  });
});
