import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebglAddon } from "@xterm/addon-webgl";
import { isDarkActive } from "../lib/theme";

// ── Theme ───────────────────────────────────────────────────────────────────

export const LIGHT_THEME = {
  background: "#ffffff",
  foreground: "#24292f",
  cursor: "#24292f",
  selectionBackground: "#b3d7ff",
  black: "#24292f",
  red: "#cf222e",
  green: "#116329",
  yellow: "#9a6700",
  blue: "#0550ae",
  magenta: "#8250df",
  cyan: "#1b7c83",
  white: "#6e7781",
  brightBlack: "#57606a",
  brightRed: "#a40e26",
  brightGreen: "#1a7f37",
  brightYellow: "#633c01",
  brightBlue: "#0969da",
  brightMagenta: "#6639ba",
  brightCyan: "#3192aa",
  brightWhite: "#8c959f",
};

export const DARK_THEME = {
  background: "#111513",
  foreground: "#e7ece9",
  cursor: "#55c7ad",
  selectionBackground: "#183c34",
  black: "#1f2622",
  red: "#f87171",
  green: "#7ee0a8",
  yellow: "#f5a97f",
  blue: "#79c0ff",
  magenta: "#d2a8ff",
  cyan: "#70d6be",
  white: "#a2aaa5",
  brightBlack: "#7f8983",
  brightRed: "#ffa198",
  brightGreen: "#96e6b8",
  brightYellow: "#ffd8a8",
  brightBlue: "#a5d6ff",
  brightMagenta: "#e2c5ff",
  brightCyan: "#9fdccb",
  brightWhite: "#e7ece9",
};

export function currentTerminalTheme() {
  return isDarkActive() ? DARK_THEME : LIGHT_THEME;
}

// ── Watermark flow control ───────────────────────────────────────────────────

const HIGH_WATER = 128 * 1024; // 128 KB：超过时停止写入
const LOW_WATER = 16 * 1024; // 16 KB：恢复写入
const MAX_PENDING_BYTES = 2 * 1024 * 1024; // 2 MB：暂停期间最多缓存的数据
const MAX_WRITE_CHUNK = 32 * 1024; // xterm 大块写入会卡 UI，分片让恢复和直播都能喘气
const INPUT_FLUSH_DELAY_MS = 8;

interface SmartWriter {
  write: (data: string, callback?: () => void) => void;
  drainPending: () => void;
  setSelectionPaused: (paused: boolean) => void;
}

interface InputBatcher {
  push: (data: string) => void;
  flush: () => void;
  dispose: () => void;
}

interface ResizeScheduler {
  schedule: () => void;
  flush: () => void;
  dispose: () => void;
}

/**
 * 创建基于水位线的流控写入器。
 *
 * - 当 xterm write queue 积累超过 HIGH_WATER 时暂停写入
 * - 低于 LOW_WATER 时恢复
 * - selectionPaused 在鼠标选择期间暂停写入（可选使用）
 */
export function createSmartWriter(term: Terminal): SmartWriter {
  const state = {
    pendingChunks: [] as Array<{ data: string; callback?: () => void }>,
    pendingHead: 0,
    pendingBytes: 0,
    watermark: 0,
    paused: false,
    selectionPaused: false,
  };

  function compactPendingQueue() {
    if (state.pendingHead === 0) return;
    if (state.pendingHead < 64 && state.pendingHead * 2 < state.pendingChunks.length) return;
    state.pendingChunks = state.pendingChunks.slice(state.pendingHead);
    state.pendingHead = 0;
  }

  function dropOldestPendingChunk() {
    if (state.pendingHead >= state.pendingChunks.length) return;
    const dropped = state.pendingChunks[state.pendingHead++];
    state.pendingBytes -= dropped.data.length;
    compactPendingQueue();
  }

  function enqueuePending(data: string, callback?: () => void, limitBytes = true) {
    state.pendingChunks.push({ data, callback });
    state.pendingBytes += data.length;
    while (limitBytes && state.pendingBytes > MAX_PENDING_BYTES && state.pendingHead < state.pendingChunks.length) {
      dropOldestPendingChunk();
    }
  }

  function flushOne(data: string, callback?: () => void) {
    state.watermark += data.length;
    term.write(data, () => {
      state.watermark -= data.length;
      callback?.();
      if (state.paused && state.watermark < LOW_WATER) {
        state.paused = false;
        drainPending();
      }
    });
  }

  function drainPending() {
    while (
      state.pendingHead < state.pendingChunks.length &&
      !state.paused &&
      !state.selectionPaused
    ) {
      const next = state.pendingChunks[state.pendingHead++];
      state.pendingBytes -= next.data.length;
      if (state.watermark >= HIGH_WATER) {
        state.pendingHead--;
        state.pendingBytes += next.data.length;
        state.paused = true;
        break;
      }
      flushOne(next.data, next.callback);
    }
    compactPendingQueue();
  }

  function write(data: string, callback?: () => void) {
    if (data.length > MAX_WRITE_CHUNK) {
      for (let offset = 0; offset < data.length; offset += MAX_WRITE_CHUNK) {
        const end = offset + MAX_WRITE_CHUNK;
        enqueuePending(data.slice(offset, end), end >= data.length ? callback : undefined, false);
      }
      drainPending();
      return;
    }
    if (state.paused || state.selectionPaused || state.watermark >= HIGH_WATER) {
      if (state.watermark >= HIGH_WATER) state.paused = true;
      enqueuePending(data, callback);
      return;
    }
    flushOne(data, callback);
  }

  function setSelectionPaused(paused: boolean) {
    state.selectionPaused = paused;
    if (!paused) drainPending();
  }

  return { write, drainPending, setSelectionPaused };
}

export function createInputBatcher(send: (data: string) => void): InputBatcher {
  let timer: number | null = null;
  let buffer = "";

  function flush() {
    if (!buffer) return;
    const data = buffer;
    buffer = "";
    if (timer !== null) {
      window.clearTimeout(timer);
      timer = null;
    }
    send(data);
  }

  function scheduleFlush() {
    if (timer !== null) return;
    timer = window.setTimeout(() => {
      timer = null;
      flush();
    }, INPUT_FLUSH_DELAY_MS);
  }

  function push(data: string) {
    buffer += data;
    const hasImmediateControl =
      data.includes("\r") ||
      data.includes("\n") ||
      data.includes(String.fromCharCode(3)) ||
      data.includes(String.fromCharCode(4)) ||
      data.includes(String.fromCharCode(27));
    if (buffer.length >= 4096 || hasImmediateControl) {
      flush();
      return;
    }
    scheduleFlush();
  }

  function dispose() {
    flush();
  }

  return { push, flush, dispose };
}

export function createResizeScheduler(run: () => void, delayMs = 50): ResizeScheduler {
  let timer: number | null = null;

  function flush() {
    if (timer !== null) {
      window.clearTimeout(timer);
      timer = null;
    }
    run();
  }

  function schedule() {
    if (timer !== null) {
      window.clearTimeout(timer);
    }
    timer = window.setTimeout(() => {
      timer = null;
      run();
    }, delayMs);
  }

  function dispose() {
    if (timer !== null) {
      window.clearTimeout(timer);
      timer = null;
    }
  }

  return { schedule, flush, dispose };
}

// ── xterm initialization ─────────────────────────────────────────────────────

interface InitTerminalResult {
  term: Terminal;
  fitAddon: FitAddon;
}

/**
 * 创建 xterm Terminal 实例并加载通用 addon（FitAddon, Unicode11, WebGL）。
 * 调用方负责 term.open(container)。
 */
export function initTerminal(
  scrollback = 1000,
  theme: typeof LIGHT_THEME = currentTerminalTheme(),
): InitTerminalResult {
  const term = new Terminal({
    convertEol: false,
    scrollback,
    cursorBlink: true,
    fontFamily: "monospace",
    fontSize: 12,
    theme,
    allowProposedApi: true,
  });

  const fitAddon = new FitAddon();
  const unicode11Addon = new Unicode11Addon();
  term.loadAddon(fitAddon);
  term.loadAddon(unicode11Addon);
  term.unicode.activeVersion = "11";

  return { term, fitAddon };
}

/**
 * 尝试加载 WebGL addon，失败时静默降级。
 * 必须在 term.open() 之后调用。
 */
export function loadWebglAddon(term: Terminal): void {
  try {
    const webglAddon = new WebglAddon();
    webglAddon.onContextLoss(() => {
      webglAddon.dispose();
    });
    term.loadAddon(webglAddon);
  } catch {
    /* 不支持 WebGL 时降级，不影响功能 */
  }
}

/**
 * 安全地执行 fitAddon.fit() 并返回 { cols, rows }，失败时返回 null。
 */
export function safeFit(fitAddon: FitAddon, term: Terminal): { cols: number; rows: number } | null {
  try {
    fitAddon.fit();
    return { cols: term.cols, rows: term.rows };
  } catch {
    return null;
  }
}
