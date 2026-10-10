import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { ToolActivityItem } from "../dispatcher-chat/tool-activity";
import { createRowUiStateStore, RowUiStateProvider } from "./row-ui-state";
import { ToolCallList } from "./tool-call-card";

describe("ToolCallList 展开态恢复", () => {
  const items: ToolActivityItem[] = [
    { id: "first", name: "local_zsh", status: "success" },
    ...Array.from({ length: 4 }, (_, index): ToolActivityItem => ({
      id: `read-${index}`,
      name: "read_file",
      status: "success",
    })),
  ];

  it("收起全览后重挂，较早且已展开的工具仍可见并保留详情", () => {
    const store = createRowUiStateStore();
    store.set("tools:turn-1", false);
    store.set("card:first", true);
    const render = () =>
      renderToStaticMarkup(
        <RowUiStateProvider value={store}>
          <ToolCallList items={items} rowId="turn-1" />
        </RowUiStateProvider>,
      );
    const firstMount = render();
    const remount = render();
    expect(firstMount).toContain("执行命令");
    expect(remount).toContain("local_zsh");
    expect(remount).toContain("另有 1 项");

    store.set("card:first", false);
    expect(render()).not.toContain("执行命令");
  });

  it("总览遵循用户选择，较早的成功活动不会在重挂时再次被折叠", () => {
    const store = createRowUiStateStore();
    store.set("tools:turn-1", true);
    const html = renderToStaticMarkup(
      <RowUiStateProvider value={store}>
        <ToolCallList items={items} rowId="turn-1" />
      </RowUiStateProvider>,
    );
    expect(html).toContain("执行命令");
    expect(html).toContain("收起较早活动");
  });
});
