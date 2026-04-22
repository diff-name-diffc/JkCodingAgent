import { Dwg_File_Type } from "@mlightcad/libredwg-web";
import type { DwgDatabase } from "@mlightcad/libredwg-web";
import type { CadEntityEnvelope, DwgEntityPayloadRecord, DwgParseSummary } from "../types";
import { buildNormalizedDwgIndex } from "./dwgNormalize";
import { ensureLibreDwg, registerPreparedDwgViewerParseTask } from "./dwgSharedParse";

export type DwgParsedArtifacts = {
  model: DwgDatabase;
  stats: {
    unknownEntityCount: number;
  };
  summary: DwgParseSummary;
  envelopes: CadEntityEnvelope[];
  payloads: DwgEntityPayloadRecord[];
};

const inflightParses = new Map<string, Promise<DwgParsedArtifacts>>();

function cloneBytesForParser(bytes: Uint8Array): ArrayBuffer {
  if (
    bytes.buffer instanceof ArrayBuffer &&
    bytes.byteOffset === 0 &&
    bytes.byteLength === bytes.buffer.byteLength
  ) {
    return bytes.buffer.slice(0);
  }
  return bytes.slice().buffer;
}

export async function ensureParsedDwgArtifacts({
  cacheKey,
  filePath,
  parserVersion,
  bytes,
}: {
  cacheKey: string;
  filePath: string;
  parserVersion: string;
  bytes: Uint8Array;
}): Promise<DwgParsedArtifacts> {
  const existing = inflightParses.get(cacheKey);
  if (existing) {
    return existing;
  }

  const task = (async () => {
    const libredwg = await ensureLibreDwg();
    const handle = libredwg.dwg_read_data(cloneBytesForParser(bytes), Dwg_File_Type.DWG);
    if (typeof handle !== "number") {
      throw new Error("无法创建 DWG 数据句柄");
    }

    try {
      const converted = libredwg.convertEx(handle);
      const stats = {
        unknownEntityCount: converted.stats?.unknownEntityCount ?? 0,
      };
      const normalized = buildNormalizedDwgIndex(
        converted.database,
        filePath,
        parserVersion,
        stats.unknownEntityCount,
      );
      return {
        model: converted.database,
        stats,
        summary: normalized.summary,
        envelopes: normalized.envelopes,
        payloads: normalized.payloads,
      };
    } finally {
      libredwg.dwg_free(handle);
    }
  })();

  registerPreparedDwgViewerParseTask(
    bytes.buffer as ArrayBuffer,
    task.then(({ model, stats }) => ({
      model,
      stats,
    })),
  );
  inflightParses.set(cacheKey, task);
  try {
    return await task;
  } finally {
    inflightParses.delete(cacheKey);
  }
}
