import { memo } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import { FileGlyph } from "../../file-icons";
import { cn } from "../../lib/cn";
import { isSystemGroupNode, type TreeNode } from "./tree";

const ROW_HEIGHT = 28;

function FileIcon({
  name,
  path,
  ext,
  isDir,
  iconName,
}: {
  name: string;
  path?: string;
  ext?: string;
  isDir: boolean;
  iconName?: string;
}) {
  if (isDir) {
    return <FileGlyph name={iconName ?? name} path={iconName ? undefined : path} isDir size={20} />;
  }

  return <FileGlyph name={name} path={path} extension={ext} size={20} />;
}

export const FileExplorerTreeItem = memo(function FileExplorerTreeItem({
  node,
  depth,
  selected,
  onNodeSelect,
  onNodeToggle,
}: {
  node: TreeNode;
  depth: number;
  selected: boolean;
  onNodeSelect: (node: TreeNode) => void;
  onNodeToggle: (path: string) => void;
}) {
  const isSystemGroup = isSystemGroupNode(node);

  return (
    <div
      onClick={() => (node.is_dir ? onNodeToggle(node.path) : onNodeSelect(node))}
      className={cn(
        "ai-file-explorer-row",
        selected && "is-selected",
        isSystemGroup && "is-system-group",
      )}
      style={{
        paddingLeft: 8 + depth * 14,
      }}
      title={isSystemGroup ? node.name : node.path}
    >
      <span className="ai-file-explorer-chevron">
        {node.is_dir && (node.expanded ? <ChevronDown size={11} /> : <ChevronRight size={11} />)}
      </span>
      <FileIcon
        name={node.name}
        path={node.path}
        ext={node.extension}
        isDir={node.is_dir}
        iconName={node.iconName}
      />
      <span className="ai-file-explorer-row-label">{node.name}</span>
    </div>
  );
});

export { ROW_HEIGHT };
