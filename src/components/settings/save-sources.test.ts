import { describe, expect, it, vi } from "vitest";
import {
  flushAllSaveSources,
  getSaveSourcesSnapshot,
  hasDirtySaveSources,
  publishSaveSource,
  registerSaveSource,
  subscribeSaveSources,
} from "./save-sources";

// 注册表为模块级单例：各用例注册的项在用例内自行注销，用例间快照归零。

describe("save-sources 注册表", () => {
  it("注册即出现在快照，注销后移除", () => {
    const unregister = registerSaveSource("a", async () => {});
    expect(getSaveSourcesSnapshot()).toHaveLength(1);
    const flush = vi.fn(async () => {});
    const unregisterB = registerSaveSource("b", flush);
    expect(getSaveSourcesSnapshot()).toHaveLength(2);
    unregisterB();
    expect(getSaveSourcesSnapshot()).toHaveLength(1);
    unregister();
    expect(getSaveSourcesSnapshot()).toHaveLength(0);
  });

  it("publish 推送快照且同值不通知订阅者", () => {
    const flush = async () => {};
    const unregister = registerSaveSource("a", flush);
    const subscriber = vi.fn();
    const unsubscribe = subscribeSaveSources(subscriber);

    publishSaveSource("a", { mode: "auto", dirty: true, saving: false, hasError: false });
    expect(subscriber).toHaveBeenCalledTimes(1);
    expect(hasDirtySaveSources()).toBe(true);
    const afterChange = getSaveSourcesSnapshot();

    // 同值重复发布：不通知、快照引用不变。
    publishSaveSource("a", { mode: "auto", dirty: true, saving: false, hasError: false });
    expect(subscriber).toHaveBeenCalledTimes(1);
    expect(getSaveSourcesSnapshot()).toBe(afterChange);

    // 未注册 id 为 no-op。
    publishSaveSource("missing", { mode: "manual", dirty: true, saving: true, hasError: true });
    expect(subscriber).toHaveBeenCalledTimes(1);

    unsubscribe();
    unregister();
  });

  it("flush 引用保留：发布新状态后 flush 不变", () => {
    const flush = vi.fn(async () => {});
    const unregister = registerSaveSource("a", flush);
    publishSaveSource("a", { mode: "manual", dirty: true, saving: false, hasError: false });
    const source = getSaveSourcesSnapshot().find(() => true)!;
    expect(source.mode).toBe("manual");
    expect(source.flush).toBe(flush);
    unregister();
  });

  it("StrictMode 双挂载：旧清理函数不误删新注册", () => {
    const flush1 = async () => {};
    const flush2 = async () => {};
    const unregister1 = registerSaveSource("a", flush1);
    const unregister2 = registerSaveSource("a", flush2); // 覆盖注册
    unregister1(); // 旧清理：源已换 flush2，不应删除
    expect(getSaveSourcesSnapshot()).toHaveLength(1);
    unregister2();
    expect(getSaveSourcesSnapshot()).toHaveLength(0);
  });

  it("flushAllSaveSources 只调 dirty 源", async () => {
    const dirtyFlush = vi.fn(async () => {});
    const cleanFlush = vi.fn(async () => {});
    const un1 = registerSaveSource("dirty", dirtyFlush);
    const un2 = registerSaveSource("clean", cleanFlush);
    publishSaveSource("dirty", { mode: "auto", dirty: true, saving: false, hasError: false });
    // clean 源保持注册缺省（idle）。
    await flushAllSaveSources();
    expect(dirtyFlush).toHaveBeenCalledTimes(1);
    expect(cleanFlush).not.toHaveBeenCalled();
    un1();
    un2();
  });

  it("订阅退订后不再收通知", () => {
    const unregister = registerSaveSource("a", async () => {});
    const subscriber = vi.fn();
    const unsubscribe = subscribeSaveSources(subscriber);
    publishSaveSource("a", { mode: "auto", dirty: false, saving: true, hasError: false });
    expect(subscriber).toHaveBeenCalledTimes(1);
    unsubscribe();
    publishSaveSource("a", { mode: "auto", dirty: true, saving: false, hasError: false });
    expect(subscriber).toHaveBeenCalledTimes(1);
    unregister();
  });
});
