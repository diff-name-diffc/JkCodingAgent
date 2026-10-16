/**
 * 画板文档状态机：当前画板身份、启动加载（含旧版 localStorage 场景一次性
 * 迁移）、onChange 防抖自动保存、打开/新建/重命名/删除/重载动作。
 *
 * 关键设计：
 * - 「槽位」（slot）对象标识一次画板会话：切换画板即更换槽位；在途保存
 *   完成时仅当槽位仍是当前槽位才回填 id，避免竞态串画板；
 * - 保存经 Promise 链串行：新建在途时后续保存排队，待拿到 id 后写同一
 *   文件，防止连击建出多份；
 * - 切换/删除前 flush 待保存内容，删除当前画板前还要等在途保存落盘，
 *   避免在途写在删除后重建文件。
 */

import { useCallback, useEffect, useRef, useState } from "react";
import type { ArchitectureCanvasMeta } from "../../types/architecture";
import { toast } from "../Toast";
import {
  canvasErrorMessage,
  clearLegacyScene,
  DEFAULT_CANVAS_NAME,
  deleteCanvasFile,
  EMPTY_CANVAS_SCENE,
  listCanvasFiles,
  loadCanvasFile,
  loadLegacySceneElements,
  readLastCanvasId,
  renameCanvasFile,
  saveCanvasFile,
  sceneHasVisibleElements,
  writeLastCanvasId,
  type CanvasScene,
} from "./canvas-files";
import { removeCanvasMeta, upsertCanvasMeta } from "./canvas-file-list";

const SAVE_DEBOUNCE_MS = 900;

/** 当前画板身份；id 为 null 表示尚未落盘的新画板（首个内容变更时自动建文件）。 */
export interface CanvasIdentity {
  id: string | null;
  name: string;
}

interface CanvasSlot extends CanvasIdentity {
  /** 单调递增世代：切换画板即换槽位对象，供在途保存识别自己是否已过期。 */
  key: number;
}

export interface CanvasDocumentController {
  /** 启动加载（列表 + 上次画板/迁移）完成。 */
  ready: boolean;
  current: CanvasIdentity;
  /** 当前画板挂载用初始场景（随 epoch 一起替换）。 */
  initialScene: CanvasScene;
  /** 场景世代号：变更即触发 Excalidraw 重挂载以完整回显。 */
  epoch: number;
  /** 已保存画板列表（更新时间倒序）。 */
  canvases: ArchitectureCanvasMeta[];
  /** Excalidraw onChange 喂入（elements/appState/files）。 */
  handleSceneChange: (scene: CanvasScene) => void;
  openCanvas: (canvasId: string) => Promise<void>;
  createCanvas: () => Promise<void>;
  renameCanvas: (name: string) => Promise<void>;
  deleteCanvas: (canvasId: string) => Promise<void>;
  /** 崩溃重试：从磁盘重载当前画板（未落盘新画板则重置为空）。 */
  reloadCurrent: () => Promise<void>;
  /**
   * 显式保存（Ctrl/Cmd+S）：以调用方快照的实时场景为准（吸收防抖待保存
   * 内容）；name 非空时先改名。未命名空画板也尊重用户意图建文件。
   */
  saveNow: (name: string | undefined, scene: CanvasScene) => Promise<void>;
}

export function useCanvasDocument(): CanvasDocumentController {
  const [ready, setReady] = useState(false);
  const [current, setCurrent] = useState<CanvasIdentity>({
    id: null,
    name: DEFAULT_CANVAS_NAME,
  });
  const [initialScene, setInitialScene] = useState<CanvasScene>(EMPTY_CANVAS_SCENE);
  const [epoch, setEpoch] = useState(0);
  const [canvases, setCanvases] = useState<ArchitectureCanvasMeta[]>([]);

  const slotRef = useRef<CanvasSlot>({ key: 0, id: null, name: DEFAULT_CANVAS_NAME });
  const canvasesRef = useRef<ArchitectureCanvasMeta[]>([]);
  const pendingSceneRef = useRef<CanvasScene | null>(null);
  const saveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const saveChainRef = useRef<Promise<void>>(Promise.resolve());

  useEffect(() => {
    canvasesRef.current = canvases;
  }, [canvases]);

  const applySlot = useCallback((slot: CanvasSlot) => {
    slotRef.current = slot;
    setCurrent({ id: slot.id, name: slot.name });
  }, []);

  /** 场景落盘（经保存链串行）；新建保存完成且槽位未易主时转正当前画板。 */
  const enqueuePersist = useCallback(
    (slot: CanvasSlot, scene: CanvasScene, notify: boolean): Promise<void> => {
      const run = saveChainRef.current.then(async () => {
        try {
          const meta = await saveCanvasFile(slot.id, slot.name, scene);
          // 新建保存完成：仅当槽位未易主才转正为当前画板（过期槽位的文件
          // 已落盘，仅刷新列表，不劫持用户已切换到的新画板）。
          if (slotRef.current === slot && slot.id === null) {
            slot.id = meta.id;
            slot.name = meta.name;
            setCurrent({ id: meta.id, name: meta.name });
            writeLastCanvasId(meta.id);
          }
          setCanvases((prev) => upsertCanvasMeta(prev, meta));
          if (notify) toast.success("画板已保存");
        } catch (error) {
          console.error("画板保存失败:", error);
          toast.error(`画板保存失败：${canvasErrorMessage(error)}`);
        }
      });
      saveChainRef.current = run;
      return run;
    },
    [],
  );

  /** 立即落盘待保存内容（清防抖计时器）；返回保存链 Promise 供动作排队等待。 */
  const flushPendingSave = useCallback((): Promise<void> => {
    if (saveTimerRef.current) {
      clearTimeout(saveTimerRef.current);
      saveTimerRef.current = null;
    }
    const scene = pendingSceneRef.current;
    pendingSceneRef.current = null;
    if (!scene) return saveChainRef.current;
    const slot = slotRef.current;
    // 空新画板不建文件——用户打开即走的空白会话不产生垃圾文件。
    if (!slot.id && !sceneHasVisibleElements(scene)) return saveChainRef.current;
    return enqueuePersist(slot, scene, false);
  }, [enqueuePersist]);

  const saveNow = useCallback(
    async (name: string | undefined, scene: CanvasScene): Promise<void> => {
      const slot = slotRef.current;
      const trimmed = name?.trim();
      if (trimmed) {
        slot.name = trimmed;
        setCurrent({ id: slot.id, name: trimmed });
      }
      // 实时快照即最新内容，清掉防抖待保存避免重复写。
      pendingSceneRef.current = null;
      if (saveTimerRef.current) {
        clearTimeout(saveTimerRef.current);
        saveTimerRef.current = null;
      }
      await enqueuePersist(slot, scene, true);
    },
    [enqueuePersist],
  );

  const handleSceneChange = useCallback(
    (scene: CanvasScene) => {
      pendingSceneRef.current = scene;
      if (saveTimerRef.current) clearTimeout(saveTimerRef.current);
      saveTimerRef.current = setTimeout(() => {
        saveTimerRef.current = null;
        void flushPendingSave();
      }, SAVE_DEBOUNCE_MS);
    },
    [flushPendingSave],
  );

  // 启动：加载列表 → 回显上次画板（缺省最新）→ 空库时迁移旧版场景 → 全新空画板。
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const list = await listCanvasFiles();
        if (cancelled) return;
        setCanvases(list);
        const lastId = readLastCanvasId();
        const pick = (lastId && list.find((item) => item.id === lastId)) || list[0];
        if (pick) {
          const { meta, scene } = await loadCanvasFile(pick.id);
          if (cancelled) return;
          applySlot({ key: 1, id: meta.id, name: meta.name });
          setInitialScene(scene);
          setEpoch(1);
          writeLastCanvasId(meta.id);
          setCanvases((prev) => upsertCanvasMeta(prev, meta));
        } else {
          const legacy = loadLegacySceneElements();
          if (legacy.length > 0) {
            try {
              const meta = await saveCanvasFile(null, DEFAULT_CANVAS_NAME, {
                elements: legacy,
                appState: {},
                files: {},
              });
              if (cancelled) return;
              clearLegacyScene();
              applySlot({ key: 1, id: meta.id, name: meta.name });
              setInitialScene({ elements: legacy, appState: {}, files: {} });
              setEpoch(1);
              writeLastCanvasId(meta.id);
              setCanvases((prev) => upsertCanvasMeta(prev, meta));
            } catch (error) {
              // 迁移失败保留旧数据下次重试，本次按空画板启动。
              console.error("旧画布场景迁移失败:", error);
            }
          }
        }
      } catch (error) {
        console.error("画板列表加载失败:", error);
        toast.error(`画板加载失败：${canvasErrorMessage(error)}`);
      } finally {
        if (!cancelled) setReady(true);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [applySlot]);

  // 卸载前尽力落盘（保存链内的 setState 对卸载组件为无害 no-op）。
  useEffect(
    () => () => {
      void flushPendingSave();
    },
    [flushPendingSave],
  );

  const openCanvas = useCallback(
    async (canvasId: string) => {
      if (slotRef.current.id === canvasId) return;
      await flushPendingSave();
      try {
        const { meta, scene } = await loadCanvasFile(canvasId);
        applySlot({ key: slotRef.current.key + 1, id: meta.id, name: meta.name });
        setInitialScene(scene);
        setEpoch((value) => value + 1);
        writeLastCanvasId(meta.id);
        setCanvases((prev) => upsertCanvasMeta(prev, meta));
      } catch (error) {
        console.error("画板打开失败:", error);
        toast.error(`画板打开失败：${canvasErrorMessage(error)}`);
      }
    },
    [applySlot, flushPendingSave],
  );

  const createCanvas = useCallback(async () => {
    await flushPendingSave();
    applySlot({ key: slotRef.current.key + 1, id: null, name: DEFAULT_CANVAS_NAME });
    setInitialScene(EMPTY_CANVAS_SCENE);
    setEpoch((value) => value + 1);
    writeLastCanvasId(null);
  }, [applySlot, flushPendingSave]);

  const renameCanvas = useCallback(
    async (name: string) => {
      const trimmed = name.trim();
      if (!trimmed) return;
      const slot = slotRef.current;
      if (slot.name === trimmed) return;
      if (slot.id === null) {
        // 未落盘新画板：仅更新内存名，随后自动保存用它。
        slot.name = trimmed;
        setCurrent({ id: null, name: trimmed });
        return;
      }
      // 内容先落盘，rename 再以磁盘为准重写 appState.name。
      await flushPendingSave();
      try {
        const meta = await renameCanvasFile(slot.id, trimmed);
        if (slotRef.current === slot) {
          slot.name = meta.name;
          setCurrent({ id: meta.id, name: meta.name });
        }
        setCanvases((prev) => upsertCanvasMeta(prev, meta));
      } catch (error) {
        console.error("画板重命名失败:", error);
        toast.error(`画板重命名失败：${canvasErrorMessage(error)}`);
      }
    },
    [flushPendingSave],
  );

  const deleteCanvas = useCallback(
    async (canvasId: string) => {
      try {
        const deletingCurrent = slotRef.current.id === canvasId;
        if (deletingCurrent) {
          // 丢弃针对该画板的待保存内容，并等在途保存落盘，避免在途写重建文件。
          pendingSceneRef.current = null;
          if (saveTimerRef.current) {
            clearTimeout(saveTimerRef.current);
            saveTimerRef.current = null;
          }
          await saveChainRef.current;
        }
        await deleteCanvasFile(canvasId);
        const remaining = removeCanvasMeta(canvasesRef.current, canvasId);
        setCanvases(remaining);
        toast.success("画板已删除");
        if (deletingCurrent) {
          if (remaining.length > 0) {
            writeLastCanvasId(null);
            await openCanvas(remaining[0].id);
          } else {
            await createCanvas();
          }
        }
      } catch (error) {
        console.error("画板删除失败:", error);
        toast.error(`画板删除失败：${canvasErrorMessage(error)}`);
      }
    },
    [createCanvas, openCanvas],
  );

  const reloadCurrent = useCallback(async () => {
    const slot = slotRef.current;
    if (slot.id === null) {
      setInitialScene(EMPTY_CANVAS_SCENE);
      setEpoch((value) => value + 1);
      return;
    }
    try {
      const { meta, scene } = await loadCanvasFile(slot.id);
      if (slotRef.current !== slot) return;
      slot.name = meta.name;
      setCurrent({ id: meta.id, name: meta.name });
      setInitialScene(scene);
      setEpoch((value) => value + 1);
      setCanvases((prev) => upsertCanvasMeta(prev, meta));
    } catch (error) {
      console.error("画板重载失败:", error);
      // 崩溃重试路径必须保证重挂载：回退空画板，避免同一崩溃场景反复渲染。
      if (slotRef.current === slot) {
        setInitialScene(EMPTY_CANVAS_SCENE);
        setEpoch((value) => value + 1);
      }
      toast.error(`画板重载失败：${canvasErrorMessage(error)}`);
    }
  }, []);

  return {
    ready,
    current,
    initialScene,
    epoch,
    canvases,
    handleSceneChange,
    openCanvas,
    createCanvas,
    renameCanvas,
    deleteCanvas,
    reloadCurrent,
    saveNow,
  };
}
