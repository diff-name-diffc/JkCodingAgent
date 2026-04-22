import { Dwg_File_Type, LibreDwg } from "@mlightcad/libredwg-web";
import { buildNormalizedDwgIndex } from "../lib/dwgNormalize";

type ParseRequest = {
  kind: "parse";
  filePath: string;
  fileName: string;
  parserVersion: string;
  bytes: Uint8Array;
};

type ParseResponse =
  | {
      kind: "parsed";
      filePath: string;
      parserVersion: string;
      summary: ReturnType<typeof buildNormalizedDwgIndex>["summary"];
      envelopes: ReturnType<typeof buildNormalizedDwgIndex>["envelopes"];
      payloads: ReturnType<typeof buildNormalizedDwgIndex>["payloads"];
    }
  | {
      kind: "error";
      filePath: string;
      error: string;
    };

const workerScope = self as typeof globalThis & {
  onmessage: ((event: MessageEvent<ParseRequest>) => void) | null;
  postMessage: (message: ParseResponse) => void;
};

let activeFilePath: string | null = null;

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

function postWorkerError(error: unknown, filePath = activeFilePath ?? "__unknown__") {
  const response: ParseResponse = {
    kind: "error",
    filePath,
    error: describeUnknownError(error),
  };
  workerScope.postMessage(response);
}

self.addEventListener("error", (event: ErrorEvent) => {
  postWorkerError(event.message || event.error || "DWG 解析 worker 全局错误");
});

self.addEventListener("unhandledrejection", (event: PromiseRejectionEvent) => {
  postWorkerError(event.reason || "DWG 解析 worker 未处理 Promise 异常");
});

workerScope.onmessage = async (event: MessageEvent<ParseRequest>) => {
  const payload = event.data;
  if (payload.kind !== "parse") {
    return;
  }
  activeFilePath = payload.filePath;

  try {
    const libredwg = await LibreDwg.create();
    const input = payload.bytes.slice().buffer;
    const handle = libredwg.dwg_read_data(input, Dwg_File_Type.DWG);
    if (typeof handle !== "number") {
      throw new Error("无法创建 DWG 数据句柄");
    }

    try {
      const converted = libredwg.convertEx(handle);
      const normalized = buildNormalizedDwgIndex(
        converted.database,
        payload.filePath,
        payload.parserVersion,
        converted.stats.unknownEntityCount,
      );
      const response: ParseResponse = {
        kind: "parsed",
        filePath: payload.filePath,
        parserVersion: payload.parserVersion,
        summary: normalized.summary,
        envelopes: normalized.envelopes,
        payloads: normalized.payloads,
      };
      workerScope.postMessage(response);
    } finally {
      libredwg.dwg_free(handle);
    }
  } catch (error) {
    postWorkerError(error, payload.filePath);
  }
};

export {};
