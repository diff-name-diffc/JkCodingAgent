import { forwardRef, type HTMLAttributes } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import { FileGlyph } from "../../file-icons";
import s from "../../styles";
import { isSystemGroupNode, type TreeNode } from "./tree";

const ROW_HEIGHT = 30;

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

export const FileExplorerTreeItem = forwardRef<
  HTMLDivElement,
  HTMLAttributes<HTMLDivElement> & {
    node: TreeNode;
    depth: number;
    selectedPath: string | null;
    onNodeSelect: (node: TreeNode) => void;
    onNodeToggle: (path: string) => void;
  }
>(function FileExplorerTreeItem(
  {
    node,
    depth,
    selectedPath,
    onNodeSelect,
    onNodeToggle,
    ...rest
  },
  ref,
) {
  const isSelected = selectedPath === node.path;
  const isSystemGroup = isSystemGroupNode(node);

  return (
    <div
      {...rest}
      ref={ref}
      onClick={() => (node.is_dir ? onNodeToggle(node.path) : onNodeSelect(node))}
      style={{
        ...s.fileExplorerRow,
        paddingLeft: 8 + depth * 14,
        background: isSelected ? "var(--bg-selected)" : "transparent",
      }}
      onMouseEnter={(event) => {
        if (!isSelected) {
          event.currentTarget.style.background = "var(--bg-hover)";
        }
      }}
      onMouseLeave={(event) => {
        if (!isSelected) {
          event.currentTarget.style.background = "transparent";
        }
      }}
      title={isSystemGroup ? node.name : node.path}
    >
      <span style={s.fileExplorerRowChevron}>
        {node.is_dir && (node.expanded ? <ChevronDown size={11} /> : <ChevronRight size={11} />)}
      </span>
      <FileIcon
        name={node.name}
        path={node.path}
        ext={node.extension}
        isDir={node.is_dir}
        iconName={node.iconName}
      />
      <span style={s.fileExplorerRowLabel}>{node.name}</span>
    </div>
  );
});

export { ROW_HEIGHT };
