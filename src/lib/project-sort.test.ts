import { describe, expect, it } from "vitest";
import type { Project } from "../types";
import { sortProjectsByRecency } from "./project-sort";

const project = (id: string, name: string, lastOpenedAt: number): Project =>
  ({ id, name, path: `/tmp/${id}`, lastOpenedAt } as Project);

describe("sortProjectsByRecency", () => {
  it("按 lastOpenedAt 降序，与 id 形态无关（UUID 亦正确）", () => {
    const input = [
      project("f47ac10b-58cc-4372-a567-0e02b2c3d479", "乙", 100),
      project("1", "甲", 300),
      project("e457c10b-58cc-4372-a567-0e02b2c3d478", "丙", 200),
    ];
    expect(sortProjectsByRecency(input).map((p) => p.name)).toEqual(["甲", "丙", "乙"]);
  });

  it("同时间戳回退名称序，且不修改入参", () => {
    const input = [project("b", "b", 5), project("a", "a", 5)];
    const out = sortProjectsByRecency(input);
    expect(out.map((p) => p.name)).toEqual(["a", "b"]);
    expect(input.map((p) => p.name)).toEqual(["b", "a"]);
  });
});
