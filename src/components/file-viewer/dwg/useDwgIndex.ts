import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { DwgDocumentRecord, DwgFileSnapshot, DwgParseSummary } from "../../../types";
import { buildDwgCacheFingerprint } from "../../../lib/dwgCache";
import { ensureParsedDwgArtifacts } from "../../../lib/dwgParseService";

type FileMeta = {
  sizeBytes: number;
  lineCount: number;
  isText: boolean;
  modifiedAt: number;
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
        const snapshot = await invoke<DwgFileSnapshot>("read_dwg_file_snapshot", {
          path: filePath,
          projectPath,
        });
        if (cancelled) return;

        const nextMeta: FileMeta = {
          sizeBytes: snapshot.sizeBytes,
          lineCount: 0,
          isText: false,
          modifiedAt: snapshot.modifiedAt,
        };
        const nextBytes = Uint8Array.from(snapshot.bytes);
        const fingerprint = buildDwgCacheFingerprint({
          projectPath,
          filePath,
          fileSize: nextMeta.sizeBytes,
          fileMtime: nextMeta.modifiedAt,
          parserVersion: DWG_PARSER_VERSION,
        });

        const cached = await invoke<DwgDocumentRecord | null>(
          "dispatcher_get_dwg_document_record",
          {
            projectPath,
            filePath,
            fileSize: nextMeta.sizeBytes,
            fileMtime: nextMeta.modifiedAt,
            parserVersion: DWG_PARSER_VERSION,
          },
        );
        if (cancelled) return;

        if (cached) {
          setMeta(nextMeta);
          setBytes(nextBytes);
          setSummary(cached.summary);
          setDocId(cached.id);
          setParseStatus("ready");
          setLoading(false);
          return;
        }

        setParseStatus("parsing");
        const parsedPromise = ensureParsedDwgArtifacts({
          cacheKey: fingerprint,
          filePath,
          parserVersion: DWG_PARSER_VERSION,
          bytes: nextBytes,
        });
        setMeta(nextMeta);
        setBytes(nextBytes);
        const parsed = await parsedPromise;
        if (cancelled) return;

        const saved = await invoke<DwgDocumentRecord>("dispatcher_upsert_dwg_document_index", {
          payload: {
            projectPath,
            filePath,
            fileSize: nextMeta.sizeBytes,
            fileMtime: nextMeta.modifiedAt,
            parserVersion: DWG_PARSER_VERSION,
            summary: parsed.summary,
            envelopes: parsed.envelopes,
          },
        });
        if (cancelled) return;

        setSummary(saved.summary);
        setDocId(saved.id);
        setParseStatus("ready");
        setLoading(false);

        void invoke("dispatcher_upsert_dwg_entity_payloads", {
          payload: {
            docId: saved.id,
            payloads: parsed.payloads,
          },
        }).catch((payloadError) => {
          console.error("后台写入 DWG payload 失败:", payloadError);
        });
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
  }, [fileName, filePath, projectPath]);

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
