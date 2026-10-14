import type { JSX } from "react";
import type { Components } from "streamdown";

/**
 * 聊天表格自定义渲染（接管 streamdown 默认表格组件）。
 *
 * streamdown 的默认表格 DOM 注入了 Tailwind 工具类（px-4 py-2、bg-muted/80、
 * divide-y 等）。这些类落在 @layer utilities，层序压过 .ai-streamdown 的
 * components 层覆盖，表格外观实际是工具类与残留覆盖的混排（全网格竖线、
 * 气泡里再套一层卡片）。这里改为输出不带工具类的纯净 DOM，视觉全部交给
 * styles/tailwind.css 的 .ai-md-table 规则——书板式（booktabs）：只留
 * 顶线 / 表头下线 / 底线，列间靠留白分隔，表格融入正文流。
 *
 * table 级覆盖会整体替换默认实现的包装壳（含复制/下载控件与内层滚动容器），
 * 滚动容器因此由这里自带；控件本就被禁用（controls.table = false）。
 * GFM 列对齐经 style/align prop 传入，透传保留。
 */

type MdElementProps<T extends keyof JSX.IntrinsicElements> = JSX.IntrinsicElements[T] & {
  /** react-markdown 注入的 hast 节点，DOM 不需要，丢弃。 */
  node?: unknown;
};

function MarkdownTable({ node: _node, children, ...rest }: MdElementProps<"table">) {
  return (
    <div className="ai-md-table-scroll">
      <table className="ai-md-table" {...rest}>
        {children}
      </table>
    </div>
  );
}

function MarkdownTableHead({ node: _node, children, ...rest }: MdElementProps<"thead">) {
  return <thead {...rest}>{children}</thead>;
}

function MarkdownTableBody({ node: _node, children, ...rest }: MdElementProps<"tbody">) {
  return <tbody {...rest}>{children}</tbody>;
}

function MarkdownTableRow({ node: _node, children, ...rest }: MdElementProps<"tr">) {
  return <tr {...rest}>{children}</tr>;
}

function MarkdownTableHeaderCell({ node: _node, children, ...rest }: MdElementProps<"th">) {
  return <th {...rest}>{children}</th>;
}

function MarkdownTableCell({ node: _node, children, ...rest }: MdElementProps<"td">) {
  return <td {...rest}>{children}</td>;
}

export const markdownTableComponents = {
  table: MarkdownTable,
  thead: MarkdownTableHead,
  tbody: MarkdownTableBody,
  tr: MarkdownTableRow,
  th: MarkdownTableHeaderCell,
  td: MarkdownTableCell,
} satisfies Components;
