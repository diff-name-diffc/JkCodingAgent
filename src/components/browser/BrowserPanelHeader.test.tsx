import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { BrowserPanelHeader } from "./BrowserPanelHeader";

function buttons(automating: boolean, busy: boolean, stopping = false) {
  const noop = () => {};
  const html = renderToStaticMarkup(
    <BrowserPanelHeader
      statusText={automating ? "自动化运行中（仅观察）" : "ready"}
      connected
      busy={busy}
      automating={automating}
      stopping={stopping}
      hasSession
      canOpenCurrentUrl
      onGoBack={noop}
      onStart={noop}
      onStop={noop}
      onImportProfile={noop}
      onOpenExternal={noop}
    />,
  );
  return (title: string) => {
    const tag = [...html.matchAll(/<button\b[^>]*>/g)]
      .map((match) => match[0])
      .find((button) => button.includes(`title="${title}"`));
    expect(tag, `应存在按钮：${title}`).toBeDefined();
    return tag!;
  };
}

describe("浏览器自动化期间的面板控制", () => {
  it("禁止手动控制，保留停止与外部打开", () => {
    // busy=false 才能真正隔离 automating 的禁用作用（busy=true 时该守卫
    // 先于 automating 生效，删掉 automating 条件用例也会通过）。
    const button = buttons(true, false);
    for (const title of ["返回上一页", "启动浏览器", "导入 Chrome 登录态"]) {
      expect(button(title)).toContain("disabled");
    }
    expect(button("停止自动化并关闭浏览器")).not.toContain("disabled");
    expect(button("外部浏览器打开")).not.toContain("disabled");
  });

  it("停止请求期间禁止重复停止", () => {
    expect(buttons(true, false, true)("停止自动化并关闭浏览器")).toContain("disabled");
  });

  it("自动化结束后恢复手动控制", () => {
    const button = buttons(false, false);
    for (const title of ["返回上一页", "启动浏览器", "导入 Chrome 登录态", "关闭浏览器"]) {
      expect(button(title)).not.toContain("disabled");
    }
  });
});
