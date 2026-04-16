import * as ContextMenu from "@radix-ui/react-context-menu";
import type React from "react";
import { FileGlyph } from "../../file-icons";
import type { TreeNode } from "./tree";
import type { FileContextActionGroup } from "./fileContextActions";

export function FileExplorerContextMenu({
  node,
  relativePath,
  groups,
  children,
}: {
  node: TreeNode;
  relativePath: string;
  groups: FileContextActionGroup[];
  children: React.ReactNode;
}) {
  return (
    <ContextMenu.Root>
      <ContextMenu.Trigger asChild>{children}</ContextMenu.Trigger>
      <ContextMenu.Portal>
        <ContextMenu.Content
          className="file-explorer-context-menu"
        >
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
      </ContextMenu.Portal>
    </ContextMenu.Root>
  );
}
