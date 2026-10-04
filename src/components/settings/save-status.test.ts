import { describe, expect, it } from "vitest";
import { aggregateSaveStatuses } from "./save-status";

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
      aggregateSaveStatuses({ loading: true, dirty: true, hasError: true }, [
        { ...autoIdle, hasError: true },
      ]),
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
      aggregateSaveStatuses(idle, [
        { mode: "manual" as const, dirty: true, saving: true, hasError: false },
      ]),
    ).toBe("saving");
    expect(aggregateSaveStatuses({ ...idle, dirty: true }, [autoIdle])).toBe("saving");
  });

  it("manual 源 dirty 显示 unsaved，不谎报保存中", () => {
    expect(
      aggregateSaveStatuses(idle, [
        { mode: "manual" as const, dirty: true, saving: false, hasError: false },
      ]),
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
