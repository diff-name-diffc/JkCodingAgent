import * as ContextMenu from "@radix-ui/react-context-menu";
import { useEffect } from "react";
import type React from "react";
import { FileGlyph } from "../../file-icons";
import type { TreeNode } from "./tree";
import type { FileContextActionGroup } from "./fileContextActions";

/* 单一受控菜单：行不再各自挂载 Radix ContextMenu（每行一个 Root 的首装/稳态
   开销大），由 FileExplorer 在列表上做事件委托，右键时设定目标节点。 */

function FileExplorerContextMenuContent({
  node,
  relativePath,
  groups,
}: {
  node: TreeNode;
  relativePath: string;
  groups: FileContextActionGroup[];
}) {
  return (
    <ContextMenu.Content className="file-explorer-context-menu">
      <div className="file-explorer-context-menu-header">
        <div className="file-explorer-context-menu-icon">
          <FileGlyph
            name={node.name}
            path={node.path}
            extension={node.extension}
            isDir={node.is_dir}
            size={18}
          />
        </div>
        <div className="file-explorer-context-menu-meta">
          <div className="file-explorer-context-menu-title">{node.name}</div>
          <div className="file-explorer-context-menu-path">{relativePath}</div>
        </div>
      </div>

      {groups.map((group, groupIndex) => (
        <div key={group.id}>
          {groupIndex > 0 && (
            <ContextMenu.Separator className="file-explorer-context-menu-separator" />
          )}
          {group.label && (
            <ContextMenu.Label className="file-explorer-context-menu-group-label">
              {group.label}
            </ContextMenu.Label>
          )}
          {group.items.map((item) => {
            const Icon = item.icon;

            return (
              <ContextMenu.Item
                key={item.id}
                className={[
                  "file-explorer-context-menu-item",
                  item.tone === "danger" ? "file-explorer-context-menu-item-danger" : "",
                ]
                  .filter(Boolean)
                  .join(" ")}
                onSelect={() => {
                  void item.onSelect();
                }}
              >
                <span className="file-explorer-context-menu-item-icon">
                  <Icon size={15} strokeWidth={2} />
                </span>
                <span className="file-explorer-context-menu-item-copy">
                  <span className="file-explorer-context-menu-item-label">{item.label}</span>
                  {item.caption && (
                    <span className="file-explorer-context-menu-item-caption">
                      {item.caption}
                    </span>
                  )}
                </span>
              </ContextMenu.Item>
            );
          })}
        </div>
      ))}
    </ContextMenu.Content>
  );
}

export function FileExplorerContextMenu({
  open,
  onOpenChange,
  node,
  relativePath,
  groups,
  children,
}: {
  open: boolean;
  /** 需为稳定引用（state setter 或 useCallback）：下方收敛 effect 依赖它，
   * 内联箭头函数会导致 effect 每次渲染都执行（当前守卫下无副作用，但脆弱）。 */
  onOpenChange: (open: boolean) => void;
  node: TreeNode | null;
  relativePath: string;
  groups: FileContextActionGroup[];
  children: React.ReactNode;
}) {
  // 受控契约：open 与 node 必须由调用方同步更新。open=true 而 node=null
  // 属于非法状态（Radix Root 打开但无 Content，菜单静默不渲染），主动收敛。
  // 正常路径下调用方在同一事件内同步设置两者，不应走到这里；一旦触发说明
  // 受控契约被破坏（如 Radix 回调顺序变化后某处仍依赖内部 onOpenChange），
  // 打印告警便于定位，否则菜单会静默失效且无任何报错。
  useEffect(() => {
    if (open && !node) {
      console.warn(
        "[FileExplorerContextMenu] open=true 但 node 为空：受控契约被破坏，菜单被主动收敛关闭",
      );
      onOpenChange(false);
    }
  }, [open, node, onOpenChange]);

  return (
    <ContextMenu.Root open={open && Boolean(node)} onOpenChange={onOpenChange}>
      <ContextMenu.Trigger asChild>{children}</ContextMenu.Trigger>
      {node && (
        <ContextMenu.Portal>
          <FileExplorerContextMenuContent
            node={node}
            relativePath={relativePath}
            groups={groups}
          />
        </ContextMenu.Portal>
      )}
    </ContextMenu.Root>
  );
}
