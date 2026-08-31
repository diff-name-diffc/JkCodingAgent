import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useState } from "react";
import type {
  RagIngestJobStartResult,
  RagIngestJobStatus,
  RagKbConfig,
  RagKbSaveResult,
  RagRuntimeStatus,
} from "../../../types";
import { RAG_FILE_EXTENSIONS, normalizeSparseConfig } from "./rag-config";
import { useMountedDelay } from "./useMountedDelay";

type RagConfigSectionKey = "qdrant" | "embedding" | "sparseEmbedding" | "chunking" | "ocr";

export interface RagTestFeedback {
  status: "success" | "error";
  message: string;
}

interface RagVectorTestResult {
  ok: boolean;
  message?: string;
  status?: number;
  dimension?: number;
}

interface UseRagKbConfigOptions {
  projectId?: string;
  projectPath?: string;
  showToast: (message: string, type?: "error" | "warning") => void;
}

export function useRagKbConfig({ projectId, projectPath, showToast }: UseRagKbConfigOptions) {
  const { isMounted, waitWhileMounted } = useMountedDelay();
  const [config, setConfig] = useState<RagKbConfig | null>(null);
  const [original, setOriginal] = useState<RagKbConfig | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [runtimeStatus, setRuntimeStatus] = useState<RagRuntimeStatus>({ running: false });
  const [actionInProgress, setActionInProgress] = useState<string | null>(null);
  const [qdrantTest, setQdrantTest] = useState<RagTestFeedback | null>(null);
  const [embeddingTest, setEmbeddingTest] = useState<RagTestFeedback | null>(null);
  const [selectedFiles, setSelectedFiles] = useState<string[]>([]);
  const [ingesting, setIngesting] = useState(false);
  const [ingestError, setIngestError] = useState<string | null>(null);
  const [ingestJob, setIngestJob] = useState<RagIngestJobStatus | null>(null);

  useEffect(() => {
    invoke<RagKbConfig>("rag_get_kb_config")
      .then((loaded) => {
        if (!isMounted()) return;
        const normalized = normalizeSparseConfig(loaded);
        setConfig(normalized);
        setOriginal(normalized);
      })
      .catch((error) => {
        if (!isMounted()) return;
        setSaveError(String(error));
        showToast(`加载 RAG 配置失败：${String(error)}`);
      })
      .finally(() => {
        if (isMounted()) setLoading(false);
      });

    const pollStatus = (retries: number) => {
      void invoke<RagRuntimeStatus>("rag_status")
        .then((status) => {
          if (!isMounted()) return;
          setRuntimeStatus(status);
          if (!status.running && retries > 0) {
            void waitWhileMounted(1500).then((mounted) => {
              if (mounted) pollStatus(retries - 1);
            });
          }
        })
        .catch(() => {});
    };
    pollStatus(5);
  }, [isMounted, showToast, waitWhileMounted]);

  const patchConfig = useCallback(
    <K extends RagConfigSectionKey>(key: K, patch: Partial<RagKbConfig[K]>) => {
      setConfig((previous) =>
        previous
          ? {
              ...previous,
              [key]: { ...(previous[key] as object), ...(patch as object) },
            }
          : previous,
      );
    },
    [],
  );

  const persistConfig = useCallback(
    async (next: RagKbConfig) => {
      const result = await invoke<RagKbSaveResult>("rag_save_kb_config", { config: next });
      if (!isMounted()) return result;
      setOriginal(next);
      setSaved(true);
      void waitWhileMounted(2000).then((mounted) => {
        if (mounted) setSaved(false);
      });
      if (result.reloadError) {
        showToast("配置已保存，但知识库服务热更新失败，重启应用后生效", "warning");
      } else if (result.reloaded) {
        showToast("配置已保存并热更新到运行中的服务", "warning");
      }
      return result;
    },
    [isMounted, showToast, waitWhileMounted],
  );

  const save = useCallback(async () => {
    if (!config) return;
    setSaving(true);
    setSaved(false);
    setSaveError(null);
    try {
      await persistConfig(config);
    } catch (error) {
      setSaveError(String(error));
    } finally {
      if (isMounted()) setSaving(false);
    }
  }, [config, isMounted, persistConfig]);

  const restart = useCallback(async () => {
    setActionInProgress("restart");
    try {
      const result = await invoke<RagRuntimeStatus>("rag_restart");
      if (!isMounted()) return;
      setRuntimeStatus({ running: true, port: result.port });
    } catch (error) {
      if (!isMounted()) return;
      showToast(`重启 RAG 服务失败：${String(error)}`);
      void invoke<RagRuntimeStatus>("rag_status")
        .then(setRuntimeStatus)
        .catch(() => {});
    } finally {
      if (isMounted()) setActionInProgress(null);
    }
  }, [isMounted, showToast]);

  const runTest = useCallback(
    async (target: "qdrant" | "embedding") => {
      if (!config) return;
      const setFeedback = target === "qdrant" ? setQdrantTest : setEmbeddingTest;
      setFeedback({ status: "success", message: "测试中..." });
      setActionInProgress(`test-${target}`);
      setSaveError(null);
      try {
        await persistConfig(config);
        const result = await invoke<RagVectorTestResult>(
          target === "qdrant" ? "rag_test_qdrant" : "rag_test_embedding",
          { config },
        );
        if (!isMounted()) return;
        setFeedback({ status: "success", message: result.message ?? "连接正常" });
        void waitWhileMounted(3000).then((mounted) => {
          if (mounted) setFeedback(null);
        });
        void invoke<RagRuntimeStatus>("rag_status")
          .then(setRuntimeStatus)
          .catch(() => {});
      } catch (error) {
        if (isMounted()) setFeedback({ status: "error", message: String(error) });
      } finally {
        if (isMounted()) setActionInProgress(null);
      }
    },
    [config, isMounted, persistConfig, waitWhileMounted],
  );

  const pickFiles = useCallback(async () => {
    const selected = await openDialog({
      directory: false,
      multiple: true,
      defaultPath: projectPath,
      title: "选择要导入 RAG 知识库的文件",
      filters: [{ name: "RAG 文档", extensions: RAG_FILE_EXTENSIONS }],
    });
    if (!isMounted()) return;
    const files = Array.isArray(selected)
      ? selected
      : typeof selected === "string"
        ? [selected]
        : [];
    if (files.length > 0) {
      setSelectedFiles((previous) => [...new Set([...previous, ...files])]);
      setIngestError(null);
    }
  }, [isMounted, projectPath]);

  const removeFile = useCallback((file: string) => {
    setSelectedFiles((previous) => previous.filter((item) => item !== file));
  }, []);

  const ingest = useCallback(async () => {
    if (!config || !projectId || !projectPath || selectedFiles.length === 0) return;
    setIngesting(true);
    setIngestError(null);
    setIngestJob(null);
    try {
      await persistConfig(config);
      const started = await invoke<RagIngestJobStartResult>("rag_ingest_files", {
        projectId,
        projectPath,
        files: selectedFiles,
      });
      let done = false;
      while (isMounted() && !done) {
        const status = await invoke<RagIngestJobStatus>("rag_ingest_job_status", {
          jobId: started.jobId,
        });
        if (!isMounted()) return;
        setIngestJob(status);
        done = ["done", "partial", "failed"].includes(status.status);
        if (!done && !(await waitWhileMounted(1200))) return;
      }
    } catch (error) {
      if (isMounted()) setIngestError(String(error));
    } finally {
      if (isMounted()) setIngesting(false);
    }
  }, [config, isMounted, persistConfig, projectId, projectPath, selectedFiles, waitWhileMounted]);

  return {
    config,
    setConfig,
    patchConfig,
    loading,
    saving,
    saved,
    saveError,
    dirty: Boolean(config && original && JSON.stringify(config) !== JSON.stringify(original)),
    runtimeStatus,
    actionInProgress,
    qdrantTest,
    embeddingTest,
    selectedFiles,
    ingesting,
    ingestError,
    ingestJob,
    importReady: Boolean(projectId && projectPath),
    save,
    restart,
    runTest,
    pickFiles,
    removeFile,
    ingest,
  };
}

export type RagKbConfigController = ReturnType<typeof useRagKbConfig>;
