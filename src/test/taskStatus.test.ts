import { describe, expect, it } from "vitest";

import { isActiveTaskStatus } from "../types";

describe("isActiveTaskStatus", () => {
  it("将 stopped 视为非活跃状态，避免误判为仍在执行", () => {
    expect(isActiveTaskStatus("stopped")).toBe(false);
  });

  it("保留 pending/running/input_required 的活跃语义", () => {
    expect(isActiveTaskStatus("pending")).toBe(true);
    expect(isActiveTaskStatus("running")).toBe(true);
    expect(isActiveTaskStatus("input_required")).toBe(true);
  });
});
