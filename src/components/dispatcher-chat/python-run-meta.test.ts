import { describe, expect, it } from "vitest";
import {
  formatPythonRunDuration,
  formatPythonRunStartedAt,
  pythonRunSourceLabel,
} from "./python-run-meta";

function record(patch: Partial<{ status: string; createdAt: string; updatedAt: string }>) {
  return {
    status: "done",
    createdAt: "2026-09-06T12:00:00.000Z",
    updatedAt: "2026-09-06T12:00:03.200Z",
    ...patch,
  };
}

describe("formatPythonRunDuration", () => {
  it("终态耗时 = updatedAt − createdAt", () => {
    expect(formatPythonRunDuration(record({}))).toBe("3.2s");
  });

  it("running 态用 nowMs，随时间推进", () => {
    const running = record({ status: "running" });
    const now = Date.parse(running.createdAt) + 1500;
    expect(formatPythonRunDuration(running, now)).toBe("1.5s");
  });

  it("stopped/failed 终态同样按 updatedAt 计算", () => {
    expect(formatPythonRunDuration(record({ status: "stopped" }))).toBe("3.2s");
    expect(formatPythonRunDuration(record({ status: "failed" }))).toBe("3.2s");
  });

  it("非法日期返回 null（不猜 0）", () => {
    expect(formatPythonRunDuration(record({ createdAt: "not-a-date" }))).toBeNull();
    expect(formatPythonRunDuration(record({ updatedAt: "" }))).toBeNull();
  });

  it("亚秒显示 ms，超分钟显示 m/s；负差夹为 0", () => {
    expect(
      formatPythonRunDuration(
        record({ createdAt: "2026-09-06T12:00:00.000Z", updatedAt: "2026-09-06T12:00:00.250Z" }),
      ),
    ).toBe("250ms");
    expect(
      formatPythonRunDuration(
        record({ createdAt: "2026-09-06T12:00:00.000Z", updatedAt: "2026-09-06T12:02:05.000Z" }),
      ),
    ).toBe("2m 5s");
    expect(
      formatPythonRunDuration(
        record({ createdAt: "2026-09-06T12:00:05.000Z", updatedAt: "2026-09-06T12:00:00.000Z" }),
      ),
    ).toBe("0ms");
  });
});

describe("pythonRunSourceLabel", () => {
  it("含消息 id 前缀与 1 起代码块序号", () => {
    expect(
      pythonRunSourceLabel({ messageId: "msg-1234567890abcdef", codeBlockIndex: 0 }),
    ).toBe("消息 msg-1234 · 代码块 #1");
    expect(pythonRunSourceLabel({ messageId: "ab", codeBlockIndex: 2 })).toBe(
      "消息 ab · 代码块 #3",
    );
  });
});

describe("formatPythonRunStartedAt", () => {
  it("本地时区 HH:MM:SS", () => {
    const local = new Date(2026, 8, 6, 13, 5, 9);
    expect(formatPythonRunStartedAt({ createdAt: local.toISOString() })).toBe("13:05:09");
  });

  it("非法日期返回 null", () => {
    expect(formatPythonRunStartedAt({ createdAt: "oops" })).toBeNull();
  });
});
