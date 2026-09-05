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
import type { Editor } from "tldraw";
import type { ArchRunRequestPayload } from "../../types/architecture";
import { canvasNotReadyReport, type CanvasBlockInfo } from "./canvas-block-info";
import { runArchProgram } from "./program/arch-executor";
import { truncateArchReport } from "./program/arch-report";

/** 等待画布 editor 挂载的总时限与轮询步长（毫秒）。 */
const EDITOR_WAIT_MS = 2000;
const EDITOR_WAIT_STEP_MS = 100;

export function useArchRunListener(
  getEditor: () => Editor | null,
  getBlockInfo?: () => CanvasBlockInfo | null,
): void {
  const getterRef = useRef(getEditor);
  getterRef.current = getEditor;
  const blockInfoRef = useRef(getBlockInfo);
  blockInfoRef.current = getBlockInfo;

  useEffect(() => {
    let disposed = false;
    const unlistenPromise = listen<ArchRunRequestPayload>(
      "architecture-run-request",
      async (event) => {
        const { runId, workspaceId, program } = event.payload;
        // 画布 editor 的挂载与视图切换存在瞬时空窗：先短轮询等待，避免把
        // 「正在挂载」误判为「视图未打开」而直接放弃执行。
        let editor = getterRef.current();
        for (
          let waited = 0;
          !editor && waited < EDITOR_WAIT_MS;
          waited += EDITOR_WAIT_STEP_MS
        ) {
          await new Promise((resolve) => setTimeout(resolve, EDITOR_WAIT_STEP_MS));
          if (disposed) return;
          editor = getterRef.current();
        }
        let report: string;
        if (!editor) {
          // 附带阻断诊断：区分「视图未打开」与「画布被许可门禁/崩溃关闭」，
          // 让 Agent 拿到可行动的失败原因而非笼统的未就绪。
          report = canvasNotReadyReport(blockInfoRef.current?.() ?? null);
        } else {
          try {
            const outcome = await runArchProgram(editor, workspaceId, program);
            report = outcome.reportText;
          } catch (error) {
            console.error("架构画布程序执行异常:", error);
            // 手工拼接的异常报告同样受 ≤950 字符硬上限约束（tldraw 抛出的
            // 错误可能携带形状数据，超长会破坏工具报告契约）。
            report = truncateArchReport(
              `错误：画布程序执行异常：${error instanceof Error ? error.message : String(error)}。已整体回滚。`,
            );
          }
        }
        if (disposed) return;
        try {
          await invoke<boolean>("architecture_run_complete", { runId, report });
        } catch (error) {
          console.error("回传架构画布执行报告失败:", error);
        }
      },
    );

    return () => {
      disposed = true;
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
