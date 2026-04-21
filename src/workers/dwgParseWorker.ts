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
      entities: ReturnType<typeof buildNormalizedDwgIndex>["entities"];
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

workerScope.onmessage = async (event: MessageEvent<ParseRequest>) => {
  const payload = event.data;
  if (payload.kind !== "parse") {
    return;
  }

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
        entities: normalized.entities,
      };
      workerScope.postMessage(response);
    } finally {
      libredwg.dwg_free(handle);
    }
  } catch (error) {
    const response: ParseResponse = {
      kind: "error",
      filePath: payload.filePath,
      error: error instanceof Error ? error.message : String(error),
    };
    workerScope.postMessage(response);
  }
};

export {};
