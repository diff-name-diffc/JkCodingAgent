import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { DwgParseCacheRecord, DwgParseSummary } from "../../../types";
import { buildDwgCacheFingerprint } from "../../../lib/dwgCache";

type FileMeta = {
  sizeBytes: number;
  lineCount: number;
  isText: boolean;
  modifiedAt: number;
};

type ParseWorkerPayload =
  | {
      kind: "parsed";
      filePath: string;
      parserVersion: string;
      summary: DwgParseSummary;
      entities: DwgParseCacheRecord["entities"];
    }
  | {
      kind: "error";
      filePath: string;
      error: string;
    };

const DWG_PARSER_VERSION = "dwg-worker-v1";

export function useDwgIndex({
  filePath,
  fileName,
  projectPath,
}: {
  filePath: string;
  fileName: string;
  projectPath: string;
}) {
  const worker = useMemo(
    () =>
      new Worker(new URL("../../../workers/dwgParseWorker.ts", import.meta.url), {
        type: "module",
      }),
    [],
  );
  const [loading, setLoading] = useState(true);
  const [parseStatus, setParseStatus] = useState<"idle" | "parsing" | "ready" | "error">("idle");
  const [error, setError] = useState<string | null>(null);
  const [summary, setSummary] = useState<DwgParseSummary | null>(null);
  const [docId, setDocId] = useState<string | null>(null);
  const [meta, setMeta] = useState<FileMeta | null>(null);
  const [bytes, setBytes] = useState<Uint8Array | null>(null);

  useEffect(() => {
    let cancelled = false;

    async function load() {
      setLoading(true);
      setError(null);
      setSummary(null);
      setDocId(null);
      setBytes(null);
      setMeta(null);
      setParseStatus("idle");

      try {
        const [nextMeta, rawBytes] = await Promise.all([
          invoke<FileMeta>("get_file_meta", { path: filePath, projectPath }),
          invoke<number[]>("read_binary_file", { path: filePath, projectPath }),
        ]);
        if (cancelled) return;

        const nextBytes = Uint8Array.from(rawBytes);
        setMeta(nextMeta);
        setBytes(nextBytes);

        const cached = await invoke<DwgParseCacheRecord | null>("dispatcher_get_dwg_parse_cache", {
          projectPath,
          filePath,
          fileSize: nextMeta.sizeBytes,
          fileMtime: nextMeta.modifiedAt,
          parserVersion: DWG_PARSER_VERSION,
        });
        if (cancelled) return;

        if (cached) {
          const nextFingerprint = buildDwgCacheFingerprint({
            projectPath,
            filePath,
            fileSize: nextMeta.sizeBytes,
            fileMtime: nextMeta.modifiedAt,
            parserVersion: DWG_PARSER_VERSION,
          });
          const cachedFingerprint = buildDwgCacheFingerprint({
            projectPath: cached.projectPath,
            filePath: cached.filePath,
            fileSize: cached.fileSize,
            fileMtime: cached.fileMtime,
            parserVersion: cached.parserVersion,
          });
          if (cachedFingerprint === nextFingerprint) {
            setSummary(cached.summary);
            setDocId(cached.documentId ?? null);
            setParseStatus("ready");
            setLoading(false);
            return;
          }
        }

        setParseStatus("parsing");
        const workerBytes = nextBytes.slice();
        worker.postMessage(
          {
            kind: "parse",
            filePath,
            fileName,
            parserVersion: DWG_PARSER_VERSION,
            bytes: workerBytes,
          },
          [workerBytes.buffer],
        );
      } catch (nextError) {
        if (!cancelled) {
          setError(nextError instanceof Error ? nextError.message : String(nextError));
          setParseStatus("error");
          setLoading(false);
        }
      }
    }

    void load();
    return () => {
      cancelled = true;
    };
  }, [fileName, filePath, projectPath, worker]);

  useEffect(() => {
    const handleMessage = async (event: MessageEvent<ParseWorkerPayload>) => {
      const payload = event.data;
      if (payload.filePath !== filePath) {
        return;
      }
      if (payload.kind === "error") {
        setParseStatus("error");
        setError(payload.error);
        setLoading(false);
        return;
      }

      try {
        const nextMeta = await invoke<FileMeta>("get_file_meta", { path: filePath, projectPath });
        const saved = await invoke<DwgParseCacheRecord>("dispatcher_save_dwg_parse_cache", {
          payload: {
            projectPath,
            filePath,
            fileSize: nextMeta.sizeBytes,
            fileMtime: nextMeta.modifiedAt,
            parserVersion: payload.parserVersion,
            summary: payload.summary,
            entities: payload.entities,
          },
        });
        setMeta(nextMeta);
        setSummary(saved.summary);
        setDocId(saved.documentId ?? null);
        setParseStatus("ready");
        setLoading(false);
      } catch (nextError) {
        setParseStatus("error");
        setError(nextError instanceof Error ? nextError.message : String(nextError));
        setLoading(false);
      }
    };

    const handleWorkerError = (event: ErrorEvent) => {
      setParseStatus("error");
      setError(event.message || "DWG 解析 worker 发生未知错误");
      setLoading(false);
    };

    const handleWorkerMessageError = () => {
      setParseStatus("error");
      setError("DWG 解析 worker 消息传输失败");
      setLoading(false);
    };

    worker.addEventListener("message", handleMessage as unknown as EventListener);
    worker.addEventListener("error", handleWorkerError);
    worker.addEventListener("messageerror", handleWorkerMessageError);
    return () => {
      worker.removeEventListener("message", handleMessage as unknown as EventListener);
      worker.removeEventListener("error", handleWorkerError);
      worker.removeEventListener("messageerror", handleWorkerMessageError);
    };
  }, [filePath, projectPath, worker]);

  useEffect(
    () => () => {
      worker.terminate();
    },
    [worker],
  );

  return {
    loading,
    parseStatus,
    error,
    summary,
    docId,
    meta,
    bytes,
  };
}
