import { describe, expect, it } from "vitest";
import type { RagKbConfig } from "../../../types";
import {
  deriveRagRuntimeState,
  normalizeLogLevel,
  normalizeSparseConfig,
  parseBoundedNumberInput,
  ragFileName,
} from "./rag-config";

describe("rag config normalization", () => {
  it("规范化日志等级并拒绝未知值", () => {
    expect(normalizeLogLevel(" warning ")).toBe("WARNING");
    expect(normalizeLogLevel("verbose")).toBe("INFO");
    expect(normalizeLogLevel("")).toBe("INFO");
  });

  it("合法供应商下的空白或未知模型回退到默认模型", () => {
    for (const model of ["", "missing/model"]) {
      const config = {
        sparseEmbedding: { provider: "fastembed", model },
      } as RagKbConfig;
      expect(normalizeSparseConfig(config).sparseEmbedding.model).toBe("Qdrant/bm25");
    }
  });

  it("拒绝空白、非有限值和越界数字输入", () => {
    expect(parseBoundedNumberInput("", 1)).toBeNull();
    expect(parseBoundedNumberInput("Infinity", 1)).toBeNull();
    expect(parseBoundedNumberInput("0", 1)).toBeNull();
    expect(parseBoundedNumberInput("0.6", 0, 1)).toBe(0.6);
  });

  it("未知供应商与未收录模型名一并回落到默认选项", () => {
    const config = {
      sparseEmbedding: { provider: "unknown", model: "Qdrant/BM25" },
    } as RagKbConfig;

    expect(normalizeSparseConfig(config).sparseEmbedding).toEqual({
      provider: "fastembed",
      model: "Qdrant/bm25",
    });
  });

  it("从不同平台路径提取文件名", () => {
    expect(ragFileName("/tmp/docs/a.pdf")).toBe("a.pdf");
    expect(ragFileName("C:\\docs\\b.docx")).toBe("b.docx");
  });
});

describe("deriveRagRuntimeState（UI-22c）", () => {
  it("运行中优先，带端口", () => {
    expect(deriveRagRuntimeState({ running: true, restarting: false, probing: false, port: 8200 })).toEqual({
      state: "running",
      text: "已运行 · 端口 8200",
    });
    // 端口缺失回退占位符
    expect(deriveRagRuntimeState({ running: true, restarting: false, probing: false }).text).toBe(
      "已运行 · 端口 -",
    );
  });

  it("未运行但重启中 → 启动中态「重启中…」", () => {
    expect(deriveRagRuntimeState({ running: false, restarting: true, probing: false })).toEqual({
      state: "starting",
      text: "重启中…",
    });
  });

  it("未运行且探活窗口内 → 启动中态「启动中…」", () => {
    expect(deriveRagRuntimeState({ running: false, restarting: false, probing: true })).toEqual({
      state: "starting",
      text: "启动中…",
    });
  });

  it("探活窗口耗尽仍未运行 → stopped「未运行」（不被吞成启动中）", () => {
    expect(deriveRagRuntimeState({ running: false, restarting: false, probing: false })).toEqual({
      state: "stopped",
      text: "未运行",
    });
  });
});
