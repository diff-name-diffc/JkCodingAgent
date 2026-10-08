import { describe, expect, it } from "vitest";
import {
  demoteActiveTextSegments,
  foldAssistantDrafts,
  type AssistantTurnSegment,
} from "./assistant-segments";

function text(text: string, superseded?: boolean): AssistantTurnSegment {
  return { kind: "assistant-text", text, superseded };
}

describe("foldAssistantDrafts", () => {
  it("keeps only non-superseded segments as visible 正文", () => {
    const { visible, drafts } = foldAssistantDrafts([text("最终答复"), text("工具前的草稿", true)]);
    expect(visible.map((s) => s.text)).toEqual(["最终答复"]);
    expect(drafts).toEqual(["工具前的草稿"]);
  });

  it("folds every superseded segment into the thinking collapse in order", () => {
    const { visible, drafts } = foldAssistantDrafts([
      text("草稿一", true),
      text("正文"),
      text("草稿二", true),
      text("草稿三", true),
    ]);
    expect(visible.map((s) => s.text)).toEqual(["正文"]);
    expect(drafts).toEqual(["草稿一", "草稿二", "草稿三"]);
  });

  it("drops blank segments from both sides", () => {
    const { visible, drafts } = foldAssistantDrafts([text("   "), text("\n", true), text("正文")]);
    expect(visible.map((s) => s.text)).toEqual(["正文"]);
    expect(drafts).toEqual([]);
  });

  it("demoted text becomes a draft when the next round starts", () => {
    const streamed = [text("第一轮中间推理")];
    const { visible, drafts } = foldAssistantDrafts(demoteActiveTextSegments(streamed));
    expect(visible).toEqual([]);
    expect(drafts).toEqual(["第一轮中间推理"]);
  });
});
