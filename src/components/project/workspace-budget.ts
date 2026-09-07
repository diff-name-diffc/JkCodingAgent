/**
 * 工作区空间预算：纯函数模块（无 DOM、无副作用、可单测）。
 *
 * 输入视口尺寸与各开关/宽度偏好，输出每个区域的像素尺寸与有序降级记录。
 * 设计规格见 docs/ui-redesign-2026-09-06/02-design.md §3：
 *   - 协作编码：对话最低 460px、代码最低 520px，分隔条 8px；
 *   - 不满足两面板最低宽度时先收导航，再改单栏；
 *   - 主区剩余可用高不得低于 320px；
 *   - 窗口变小触发的是临时适配，不覆盖用户保存的偏好（偏好是冻结输入，
 *     自动适配只体现在输出与 degradations 里）。
 *
 * chrome 尺寸参数化：当前外壳为 rail 56 / 工具栏 48 / 会话栏 288，
 * UI-07 统一外壳后改为 rail 52 / 工具栏 0 / 导航 248，仅需换参数。
 */

export interface WorkspaceChromeSizes {
  /** 左侧项目 rail 宽（UI-07 后为全局 rail 52）。 */
  railWidth: number;
  /** 右侧工具栏宽（UI-07 移除后为 0）。 */
  toolbarWidth: number;
  /** 顶栏高（拖拽安全区）。 */
  titlebarHeight: number;
  /** 上下文导航（会话栏）允许范围。 */
  navMin: number;
  navMax: number;
  /** 右面板（文件/Git/浏览器）允许范围。 */
  rightPanelMin: number;
  rightPanelMax: number;
  /** 中央分屏分隔条宽。 */
  splitterWidth: number;
  /** 终端 dock 高度范围。 */
  terminalMinHeight: number;
  terminalMaxHeight: number;
  /** 主区剩余可用高下限。 */
  minMainHeight: number;
  /** 双栏时对话面板最小宽。 */
  minChatWidth: number;
  /** 双栏时编辑面板最小宽。 */
  minEditorWidth: number;
}

/**
 * 终端 dock 高度范围（UI-19 单一出处）：workspace-prefs 的 sanitize 与
 * useProjectPanels 的拖拽钳制共用，避免三处字面量漂移。
 */
export const TERMINAL_HEIGHT_LIMITS = { min: 100, max: 600 } as const;

export const DEFAULT_CHROME: WorkspaceChromeSizes = {
  railWidth: 56,
  toolbarWidth: 48,
  titlebarHeight: 38,
  navMin: 216,
  navMax: 320,
  rightPanelMin: 180,
  rightPanelMax: 600,
  splitterWidth: 8,
  terminalMinHeight: TERMINAL_HEIGHT_LIMITS.min,
  terminalMaxHeight: TERMINAL_HEIGHT_LIMITS.max,
  minMainHeight: 320,
  minChatWidth: 460,
  minEditorWidth: 520,
};

export interface WorkspaceBudgetInput {
  viewportWidth: number;
  viewportHeight: number;
  chrome?: Partial<WorkspaceChromeSizes>;
  /** 用户偏好：导航展开。 */
  navOpen: boolean;
  navWidthPref: number;
  /** 用户偏好：右面板打开与其宽度。 */
  rightPanelOpen: boolean;
  rightPanelWidthPref: number;
  /** 用户偏好：终端 dock 打开与其高度。 */
  terminalOpen: boolean;
  terminalHeightPref: number;
  /** 用户请求双栏（会话 + 编辑器同时可见）。 */
  dualPaneRequested: boolean;
  /** 双栏时编辑器占比偏好（0–1）。 */
  editorRatioPref: number;
}

export type WorkspaceDegradation =
  | { kind: "terminal-height-clamped"; from: number; to: number }
  | { kind: "main-height-below-min"; height: number }
  | { kind: "right-panel-clamped"; from: number; to: number }
  | { kind: "right-panel-closed" }
  | { kind: "nav-collapsed" }
  | { kind: "single-column"; reason: string };

export interface WorkspaceBudget {
  railWidth: number;
  toolbarWidth: number;
  /** 临时适配后的导航宽（0 = 收起）。 */
  navWidth: number;
  /** 临时适配后的右面板宽（0 = 关闭）。 */
  rightPanelWidth: number;
  /** 临时适配后的终端高度（0 = 关闭）。 */
  terminalHeight: number;
  /** 中央主区可用宽。 */
  mainWidth: number;
  /** 主区（workbench）可用高。 */
  mainHeight: number;
  dualPane: boolean;
  chatWidth: number;
  editorWidth: number;
  splitterWidth: number;
  /** 有序降级记录（先发生的在前）。 */
  degradations: WorkspaceDegradation[];
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

/**
 * 计算工作区空间预算。五步降级树：
 *   1 终端高度压缩 → 2 右面板压缩 → 3 右面板临时关闭 → 4 导航临时收起 → 5 双栏转单栏。
 * 任何一步都不修改输入偏好；输出即「本帧应渲染的尺寸」。
 */
export function resolveWorkspaceBudget(input: WorkspaceBudgetInput): WorkspaceBudget {
  const chrome: WorkspaceChromeSizes = { ...DEFAULT_CHROME, ...input.chrome };
  const degradations: WorkspaceDegradation[] = [];

  const availableWidth = Math.max(
    0,
    input.viewportWidth - chrome.railWidth - chrome.toolbarWidth,
  );

  // ── 导航 ────────────────────────────────────────────────
  let navWidth = 0;
  if (input.navOpen) {
    const preferred = clamp(input.navWidthPref, chrome.navMin, chrome.navMax);
    // 收起导航前至少保证单栏对话最小宽
    if (availableWidth - preferred >= chrome.minChatWidth) {
      navWidth = preferred;
    } else {
      degradations.push({ kind: "nav-collapsed" });
    }
  }

  // ── 右面板 ──────────────────────────────────────────────
  let rightPanelWidth = 0;
  if (input.rightPanelOpen) {
    const remaining = availableWidth - navWidth;
    let width = clamp(input.rightPanelWidthPref, chrome.rightPanelMin, chrome.rightPanelMax);
    if (remaining - width < chrome.minChatWidth) {
      const clamped = chrome.rightPanelMin;
      if (remaining - clamped >= chrome.minChatWidth) {
        degradations.push({ kind: "right-panel-clamped", from: width, to: clamped });
        width = clamped;
      } else {
        degradations.push({ kind: "right-panel-closed" });
        width = 0;
      }
    }
    rightPanelWidth = width;
  }

  const mainWidth = Math.max(0, availableWidth - navWidth - rightPanelWidth);

  // ── 双栏 / 单栏 ─────────────────────────────────────────
  let dualPane = false;
  let chatWidth = mainWidth;
  let editorWidth = 0;
  let splitterWidth = 0;
  if (input.dualPaneRequested) {
    const required = chrome.minChatWidth + chrome.splitterWidth + chrome.minEditorWidth;
    if (mainWidth >= required) {
      const inner = mainWidth - chrome.splitterWidth;
      const ratio = clamp(input.editorRatioPref, 0, 1);
      editorWidth = clamp(
        Math.round(inner * ratio),
        chrome.minEditorWidth,
        inner - chrome.minChatWidth,
      );
      chatWidth = inner - editorWidth;
      splitterWidth = chrome.splitterWidth;
      dualPane = true;
    } else {
      degradations.push({
        kind: "single-column",
        reason: `主区 ${mainWidth}px 不足双栏最低 ${required}px`,
      });
    }
  }

  // ── 垂直预算 ────────────────────────────────────────────
  let terminalHeight = 0;
  if (input.terminalOpen) {
    const maxByMain = Math.max(
      chrome.terminalMinHeight,
      input.viewportHeight - chrome.titlebarHeight - chrome.minMainHeight,
    );
    terminalHeight = clamp(
      input.terminalHeightPref,
      chrome.terminalMinHeight,
      Math.min(chrome.terminalMaxHeight, maxByMain),
    );
    if (terminalHeight !== input.terminalHeightPref) {
      degradations.push({
        kind: "terminal-height-clamped",
        from: input.terminalHeightPref,
        to: terminalHeight,
      });
    }
  }
  const mainHeight = Math.max(
    0,
    input.viewportHeight - chrome.titlebarHeight - terminalHeight,
  );
  if (mainHeight < chrome.minMainHeight) {
    degradations.push({ kind: "main-height-below-min", height: mainHeight });
  }

  return {
    railWidth: chrome.railWidth,
    toolbarWidth: chrome.toolbarWidth,
    navWidth,
    rightPanelWidth,
    terminalHeight,
    mainWidth,
    mainHeight,
    dualPane,
    chatWidth,
    editorWidth,
    splitterWidth,
    degradations,
  };
}

/** 拖拽编辑器分隔条时由像素反推占比偏好（供调用方写回偏好）。 */
export function editorRatioFromWidths(
  chatWidth: number,
  editorWidth: number,
  splitterWidth: number,
): number {
  const inner = chatWidth + editorWidth + splitterWidth;
  if (inner <= 0) return 0.5;
  return clamp(editorWidth / inner, 0, 1);
}

/**
 * 拖拽期双栏瞬时宽度（UI-24a-4）：与 resolveWorkspaceBudget 的双栏钳制
 * 同口径（minEditorWidth ≤ editor ≤ inner - minChatWidth）。拖拽中在
 * ProjectWorkbenchContent 本地重算（不写持久化偏好、不触发 ProjectPage
 * 全树重渲染），mouseup 才经 onEditorPaneRatioChange 一次性写回。
 * 入参为预算输出的双栏宽度（已不含 splitter），inner = chat + editor。
 */
export function splitDualPaneWidths(
  chatWidth: number,
  editorWidth: number,
  ratio: number,
  chrome: Pick<WorkspaceChromeSizes, "minChatWidth" | "minEditorWidth"> = DEFAULT_CHROME,
): { chatWidth: number; editorWidth: number } {
  const inner = Math.max(0, chatWidth + editorWidth);
  const editor = clamp(
    Math.round(inner * clamp(ratio, 0, 1)),
    Math.min(chrome.minEditorWidth, inner),
    Math.max(Math.min(chrome.minEditorWidth, inner), inner - chrome.minChatWidth),
  );
  return { chatWidth: inner - editor, editorWidth: editor };
}
