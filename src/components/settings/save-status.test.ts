import { describe, expect, it } from "vitest";
import { aggregateSaveStatuses, deriveSaveStatus } from "./save-status";

describe("deriveSaveStatus", () => {
  it("loading 优先于其它状态", () => {
    expect(deriveSaveStatus({ loading: true, dirty: false, hasError: false })).toBe("loading");
    expect(deriveSaveStatus({ loading: true, dirty: true, hasError: true })).toBe("loading");
  });

  it("无错误、非 dirty 时为已保存（含初次加载完成未编辑）", () => {
    expect(deriveSaveStatus({ loading: false, dirty: false, hasError: false })).toBe("saved");
  });

  it("dirty（pending 或 saving）且无错误时为保存中", () => {
    expect(deriveSaveStatus({ loading: false, dirty: true, hasError: false })).toBe("saving");
  });

  it("有错误时为保存失败，优先于 saving", () => {
    expect(deriveSaveStatus({ loading: false, dirty: false, hasError: true })).toBe("error");
    // 失败后未再编辑：dirty=false，仍显示 error 而非 saved
    expect(deriveSaveStatus({ loading: false, dirty: true, hasError: true })).toBe("error");
  });
});

describe("aggregateSaveStatuses", () => {
  const idle = { loading: false, dirty: false, hasError: false };
  const autoIdle = { mode: "auto" as const, dirty: false, saving: false, hasError: false };

  it("空 sources 时与单源派生逐位一致", () => {
    expect(aggregateSaveStatuses(idle, [])).toBe("saved");
    expect(aggregateSaveStatuses({ ...idle, dirty: true }, [])).toBe("saving");
    expect(aggregateSaveStatuses({ ...idle, hasError: true }, [])).toBe("error");
    expect(aggregateSaveStatuses({ ...idle, loading: true }, [])).toBe("loading");
  });

  it("loading（全局管线加载中）门控一切", () => {
    expect(
      aggregateSaveStatuses(
        { loading: true, dirty: true, hasError: true },
        [{ ...autoIdle, hasError: true }],
      ),
    ).toBe("loading");
  });

  it("任一源失败即 error，优先于 saving/unsaved", () => {
    expect(
      aggregateSaveStatuses(idle, [
        autoIdle,
        { ...autoIdle, dirty: true },
        { ...autoIdle, hasError: true },
      ]),
    ).toBe("error");
    expect(aggregateSaveStatuses({ ...idle, dirty: true }, [{ ...autoIdle, hasError: true }])).toBe(
      "error",
    );
  });

  it("auto 源 dirty 或任一源保存进行中即 saving", () => {
    expect(aggregateSaveStatuses(idle, [{ ...autoIdle, dirty: true }])).toBe("saving");
    expect(
      aggregateSaveStatuses(idle, [{ mode: "manual" as const, dirty: true, saving: true, hasError: false }]),
    ).toBe("saving");
    expect(aggregateSaveStatuses({ ...idle, dirty: true }, [autoIdle])).toBe("saving");
  });

  it("manual 源 dirty 显示 unsaved，不谎报保存中", () => {
    expect(
      aggregateSaveStatuses(idle, [{ mode: "manual" as const, dirty: true, saving: false, hasError: false }]),
    ).toBe("unsaved");
  });

  it("saving 优先于 unsaved（auto 在保存、manual 有未保存修改并存时）", () => {
    expect(
      aggregateSaveStatuses(idle, [
        { ...autoIdle, dirty: true },
        { mode: "manual" as const, dirty: true, saving: false, hasError: false },
      ]),
    ).toBe("saving");
  });

  it("全部空闲即 saved", () => {
    expect(
      aggregateSaveStatuses(idle, [
        autoIdle,
        { mode: "manual" as const, dirty: false, saving: false, hasError: false },
      ]),
    ).toBe("saved");
  });
});
