import { describe, expect, it } from "vitest";
import { pickCurrentBranchName, type GitBranchInfo } from "./git-branch";

function branch(name: string, current: boolean, remote: string | null = null): GitBranchInfo {
  return { name, current, remote };
}

describe("pickCurrentBranchName", () => {
  it("返回 current 分支名", () => {
    expect(pickCurrentBranchName([branch("main", false), branch("dev", true)])).toBe("dev");
  });

  it("无 current 分支时返回 null", () => {
    expect(pickCurrentBranchName([branch("main", false), branch("dev", false)])).toBeNull();
  });

  it("空列表返回 null", () => {
    expect(pickCurrentBranchName([])).toBeNull();
  });

  it("多 remote 分支只认 current 标记", () => {
    expect(
      pickCurrentBranchName([
        branch("origin/main", false, "origin"),
        branch("upstream/dev", false, "upstream"),
        branch("feature/ui-11", true),
      ]),
    ).toBe("feature/ui-11");
  });
});
