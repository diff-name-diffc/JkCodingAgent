import { describe, expect, it } from "vitest";
import { diffFileDisplayPath, parseUnifiedDiff } from "./git-diff";

const MODIFIED_DIFF = [
  "diff --git a/src/a.ts b/src/a.ts",
  "index 1111111..2222222 100644",
  "--- a/src/a.ts",
  "+++ b/src/a.ts",
  "@@ -1,4 +1,5 @@",
  " import x;",
  "-const a = 1;",
  "+const a = 2;",
  "+const b = 3;",
  " ",
  "\\ No newline at end of file",
].join("\n");

describe("parseUnifiedDiff", () => {
  it("parses a regular modification with structured paths and line numbers", () => {
    const files = parseUnifiedDiff(MODIFIED_DIFF);
    expect(files).toHaveLength(1);
    const file = files[0];
    expect(file.header).toBe("src/a.ts");
    expect(file.oldPath).toBe("src/a.ts");
    expect(file.newPath).toBe("src/a.ts");
    expect(file.isNew).toBe(false);
    expect(file.isDeleted).toBe(false);
    expect(file.isBinary).toBe(false);
    expect(file.hunks).toHaveLength(1);

    const lines = file.hunks[0].lines;
    expect(lines.map((l) => l.type)).toEqual(["ctx", "del", "add", "add", "ctx"]);
    // 行号递进：ctx 双侧、del 仅旧侧、add 仅新侧；No-newline 行被跳过。
    expect(lines[0]).toMatchObject({ oldLn: 1, newLn: 1 });
    expect(lines[1]).toMatchObject({ oldLn: 2, newLn: null });
    expect(lines[2]).toMatchObject({ oldLn: null, newLn: 2 });
    expect(lines[3]).toMatchObject({ oldLn: null, newLn: 3 });
    expect(lines[4]).toMatchObject({ oldLn: 3, newLn: 4, content: "" });
  });

  it("marks new files with null oldPath", () => {
    const files = parseUnifiedDiff(
      [
        "diff --git a/created.txt b/created.txt",
        "new file mode 100644",
        "index 0000000..abcdef0",
        "--- /dev/null",
        "+++ b/created.txt",
        "@@ -0,0 +1,1 @@",
        "+hello",
      ].join("\n"),
    );
    expect(files[0].isNew).toBe(true);
    expect(files[0].oldPath).toBeNull();
    expect(files[0].newPath).toBe("created.txt");
    expect(files[0].hunks[0].lines[0]).toMatchObject({ type: "add", newLn: 1 });
  });

  it("marks deleted files with null newPath", () => {
    const files = parseUnifiedDiff(
      [
        "diff --git a/gone.txt b/gone.txt",
        "deleted file mode 100644",
        "index abcdef0..0000000",
        "--- a/gone.txt",
        "+++ /dev/null",
        "@@ -1,1 +0,0 @@",
        "-bye",
      ].join("\n"),
    );
    expect(files[0].isDeleted).toBe(true);
    expect(files[0].newPath).toBeNull();
    expect(files[0].hunks[0].lines[0]).toMatchObject({ type: "del", oldLn: 1 });
  });

  it("captures rename metadata with similarity and no hunks", () => {
    const files = parseUnifiedDiff(
      [
        "diff --git a/old-name.ts b/new-name.ts",
        "similarity index 92%",
        "rename from old-name.ts",
        "rename to new-name.ts",
        "index 1111111..2222222 100644",
      ].join("\n"),
    );
    const file = files[0];
    expect(file.renameFrom).toBe("old-name.ts");
    expect(file.renameTo).toBe("new-name.ts");
    expect(file.similarity).toBe(92);
    expect(file.hunks).toHaveLength(0);
    expect(diffFileDisplayPath(file)).toBe("new-name.ts");
  });

  it("marks binary files", () => {
    const differ = parseUnifiedDiff(
      ["diff --git a/logo.png b/logo.png", "index 111..222 100644", "Binary files a/logo.png and b/logo.png differ"].join("\n"),
    );
    expect(differ[0].isBinary).toBe(true);

    const gitBinaryPatch = parseUnifiedDiff(
      ["diff --git a/logo.png b/logo.png", "index 111..222 100644", "GIT binary patch", "literal 100"].join("\n"),
    );
    expect(gitBinaryPatch[0].isBinary).toBe(true);
  });

  it("splits multiple files", () => {
    const files = parseUnifiedDiff(
      [
        "diff --git a/one.ts b/one.ts",
        "--- a/one.ts",
        "+++ b/one.ts",
        "@@ -1 +1 @@",
        "-a",
        "+b",
        "diff --git a/two.ts b/two.ts",
        "--- a/two.ts",
        "+++ b/two.ts",
        "@@ -2 +2 @@",
        "-c",
        "+d",
      ].join("\n"),
    );
    expect(files.map((f) => f.header)).toEqual(["one.ts", "two.ts"]);
    expect(files[1].hunks[0].lines[0]).toMatchObject({ type: "del", oldLn: 2 });
  });

  it("falls back to an empty-header file for flat hunk-only diffs", () => {
    const files = parseUnifiedDiff(["@@ -1 +1 @@", "-old", "+new"].join("\n"));
    expect(files).toHaveLength(1);
    expect(files[0].header).toBe("");
    expect(files[0].hunks[0].lines.map((l) => l.type)).toEqual(["del", "add"]);
  });

  it("returns an empty array for empty input", () => {
    expect(parseUnifiedDiff("")).toEqual([]);
    expect(parseUnifiedDiff("\n\n")).toEqual([]);
  });

  it("drops hunk context after unknown non-meta lines, matching legacy behavior", () => {
    const files = parseUnifiedDiff(
      ["diff --git a/x.ts b/x.ts", "--- a/x.ts", "+++ b/x.ts", "@@ -1 +1 @@", "+new", "garbage line", " +still context?"].join("\n"),
    );
    // "garbage line" 终止当前 hunk；后续以空格开头的行不再并入任何 hunk。
    expect(files[0].hunks[0].lines).toHaveLength(1);
  });
});

describe("diffFileDisplayPath", () => {
  it("prefers renameTo, then newPath, then header", () => {
    expect(diffFileDisplayPath({ ...emptyFile(), renameTo: "r.ts" })).toBe("r.ts");
    expect(diffFileDisplayPath({ ...emptyFile(), newPath: "n.ts" })).toBe("n.ts");
    expect(diffFileDisplayPath({ ...emptyFile(), header: "h.ts" })).toBe("h.ts");
  });
});

function emptyFile() {
  return {
    header: "",
    oldPath: null,
    newPath: null,
    isNew: false,
    isDeleted: false,
    isBinary: false,
    renameFrom: null,
    renameTo: null,
    similarity: null,
    hunks: [],
  };
}
