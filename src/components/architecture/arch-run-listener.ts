/**
 * 画布执行监听器：architecture_run 工具 ↔ 前端画布解释器的往返桥梁。
 *
 * 后端工具登记 oneshot 后 emit `architecture-run-request`；本监听器在
 * editor 上执行画布程序，再经 `architecture_run_complete` 命令回传报告解除
 * 后端等待。工具侧超时/取消时槽位已清，迟到的回传返回 false，无副作用。
 */

import { useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";
import type { ArchRunRequestPayload } from "../../types/architecture";
import { canvasNotReadyReport, type CanvasBlockInfo } from "./canvas-block-info";
import { runArchProgram } from "./program/arch-executor";
import { truncateArchReport } from "./program/arch-report";

/** 等待画布 editor 挂载的总时限与轮询步长（毫秒）。 */
const EDITOR_WAIT_MS = 2000;
const EDITOR_WAIT_STEP_MS = 100;

export function useArchRunListener(
  getCanvasApi: () => ExcalidrawImperativeAPI | null,
  getBlockInfo?: () => CanvasBlockInfo | null,
): void {
  const getterRef = useRef(getCanvasApi);
  getterRef.current = getCanvasApi;
  const blockInfoRef = useRef(getBlockInfo);
  blockInfoRef.current = getBlockInfo;

  useEffect(() => {
    let disposed = false;
    const controllers = new Map<string, AbortController>();
    const cancelListener = listen<{ runId: string }>("architecture-run-cancel", (event) => {
      controllers.get(event.payload.runId)?.abort();
    });
    const unlistenPromise = cancelListener.then(() => listen<ArchRunRequestPayload>(
      "architecture-run-request",
      async (event) => {
        const { runId, workspaceId, program } = event.payload;
        const controller = new AbortController();
        controllers.set(runId, controller);
        // 画布 api 的挂载与视图切换存在瞬时空窗：先短轮询等待，避免把
        // 「正在挂载」误判为「视图未打开」而直接放弃执行。
        let canvasApi = getterRef.current();
        for (
          let waited = 0;
          !canvasApi && waited < EDITOR_WAIT_MS;
          waited += EDITOR_WAIT_STEP_MS
        ) {
          await new Promise((resolve) => setTimeout(resolve, EDITOR_WAIT_STEP_MS));
          if (disposed || controller.signal.aborted) break;
          canvasApi = getterRef.current();
        }
        let report: string;
        if (disposed || controller.signal.aborted) {
          report = "画布程序已取消，未提交修改。";
        } else if (!canvasApi) {
          // 附带阻断诊断：区分「视图未打开」与「画布此前渲染崩溃」，
          // 让 Agent 拿到可行动的失败原因而非笼统的未就绪。
          report = canvasNotReadyReport(blockInfoRef.current?.() ?? null);
        } else {
          try {
            const claimed = await invoke<boolean>("architecture_run_claim", { workspaceId, runId });
            if (!claimed) {
              report = "画布任务已取消或已结算，未执行。";
            } else {
              const outcome = await runArchProgram(canvasApi, workspaceId, program, controller.signal);
              report = outcome.reportText;
            }
          } catch (error) {
            console.error("架构画布程序执行异常:", error);
            // 手工拼接的异常报告同样受 ≤950 字符硬上限约束（超长会破坏
            // 工具报告契约）。草稿未提交即天然零副作用，等同整体回滚。
            report = truncateArchReport(
              `错误：画布程序执行异常：${error instanceof Error ? error.message : String(error)}。执行报告未生成，提交状态需核对。`,
            );
          }
        }
        try {
          await invoke<boolean>("architecture_run_complete", { workspaceId, runId, report });
        } catch (error) {
          console.error("回传架构画布执行报告失败:", error);
        } finally {
          controllers.delete(runId);
        }
      },
    ));

    return () => {
      disposed = true;
      for (const controller of controllers.values()) controller.abort();
      void cancelListener.then((unlisten) => unlisten()).catch(console.error);
      unlistenPromise
        .then((unlisten: UnlistenFn) => unlisten())
        .catch((error) => {
          // 注册/注销失败不得静默：注册失败意味着执行通道不可用，
          // 此后每次 architecture_run 都会走满后端 20s 超时。
          console.error("架构画布执行监听注册/注销失败:", error);
        });
    };
  }, []);
}
