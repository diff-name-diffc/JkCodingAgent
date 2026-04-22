import { LibreDwg } from "@mlightcad/libredwg-web";
import type { DwgDatabase } from "@mlightcad/libredwg-web";

type LibreDwgInstance = Awaited<ReturnType<typeof LibreDwg.create>>;

export type PreparedDwgViewerParse = {
  model: DwgDatabase;
  stats: {
    unknownEntityCount: number;
  };
};

let libreDwgPromise: Promise<LibreDwgInstance> | null = null;
const preparedViewerParseTasks = new WeakMap<ArrayBuffer, Promise<PreparedDwgViewerParse>>();

function describeUnknownError(error: unknown): string {
  if (error instanceof Error) {
    return error.message || error.stack || "未知 Error";
  }
  if (typeof error === "string") {
    return error;
  }
  if (error && typeof error === "object") {
    const message =
      "message" in error && typeof error.message === "string"
        ? error.message
        : "reason" in error && typeof error.reason === "string"
          ? error.reason
          : null;
    if (message) {
      return message;
    }
    try {
      return JSON.stringify(error);
    } catch {
      return Object.prototype.toString.call(error);
    }
  }
  if (typeof error === "undefined") {
    return "未知错误（undefined）";
  }
  return String(error);
}

export async function ensureLibreDwg() {
  if (!libreDwgPromise) {
    libreDwgPromise = LibreDwg.create().catch((error) => {
      libreDwgPromise = null;
      throw new Error(`libredwg 初始化失败：${describeUnknownError(error)}`);
    });
  }
  return libreDwgPromise;
}

export function registerPreparedDwgViewerParse(
  content: ArrayBuffer,
  payload: PreparedDwgViewerParse,
) {
  preparedViewerParseTasks.set(content, Promise.resolve(payload));
}

export function registerPreparedDwgViewerParseTask(
  content: ArrayBuffer,
  task: Promise<PreparedDwgViewerParse>,
) {
  preparedViewerParseTasks.set(content, task);
}

export function consumePreparedDwgViewerParseTask(
  content: ArrayBuffer,
): Promise<PreparedDwgViewerParse> | null {
  const prepared = preparedViewerParseTasks.get(content) ?? null;
  if (prepared) {
    preparedViewerParseTasks.delete(content);
  }
  return prepared;
}
