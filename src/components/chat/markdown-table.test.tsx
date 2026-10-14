import { createElement, type ComponentType, type ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { markdownTableComponents } from "./markdown-table";
import { MarkdownRenderer } from "./markdown-renderer";

type AnyComponent = ComponentType<Record<string, unknown> & { children?: ReactNode }>;

function render(component: ComponentType | undefined, props: Record<string, unknown>): string {
  if (!component || typeof component === "string") {
    throw new Error("expected a custom component override");
  }
  return renderToStaticMarkup(createElement(component as AnyComponent, props));
}

describe("markdownTableComponents", () => {
  it("覆盖表格全族元素，确保 streamdown 默认工具类 DOM 不进场", () => {
    for (const tag of ["table", "thead", "tbody", "tr", "th", "td"] as const) {
      expect(typeof markdownTableComponents[tag]).toBe("function");
    }
  });

  it("table 输出书板式滚动容器，剥离 hast node 属性", () => {
    const html = render(markdownTableComponents.table, {
      node: { type: "element" },
      children: createElement(
        "tbody",
        null,
        createElement("tr", null, createElement("td", null, "x")),
      ),
    });
    expect(html).toContain('<div class="ai-md-table-scroll"><table class="ai-md-table">');
    expect(html).toContain("<td>x</td>");
    expect(html).not.toContain("node=");
    expect(html).not.toContain("data-streamdown");
  });

  it("单元格透传 GFM 对齐 style 且不注入 className", () => {
    const html = render(markdownTableComponents.td, {
      node: { type: "element" },
      style: { textAlign: "right" },
      children: "42",
    });
    expect(html).toContain('style="text-align:right"');
    expect(html).not.toContain("class=");
  });

  it("thead/tbody/tr/th 为纯净透传", () => {
    expect(render(markdownTableComponents.thead, { node: {}, children: "h" })).toBe(
      "<thead>h</thead>",
    );
    expect(render(markdownTableComponents.tbody, { node: {}, children: "b" })).toBe(
      "<tbody>b</tbody>",
    );
    expect(render(markdownTableComponents.tr, { node: {}, children: "r" })).toBe("<tr>r</tr>");
    expect(render(markdownTableComponents.th, { node: {}, children: "c" })).toBe("<th>c</th>");
  });
});

describe("MarkdownRenderer 表格渲染", () => {
  it("GFM 表格走自定义书板式渲染，不含 streamdown 默认包装与工具类", () => {
    const html = renderToStaticMarkup(
      createElement(MarkdownRenderer, {
        content: "| 引擎 | 负责的输入 |\n| --- | --- |\n| Chromium | HTML、Markdown |",
      }),
    );
    expect(html).toContain("ai-md-table-scroll");
    expect(html).toContain("ai-md-table");
    expect(html).toContain("Chromium");
    expect(html).not.toContain('data-streamdown="table-wrapper"');
    expect(html).not.toContain("bg-muted");
    expect(html).not.toContain("px-4");
  });
});
