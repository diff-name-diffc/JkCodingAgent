import { useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { Check, ChevronDown, FilePlus2, FolderOpen, Pencil, Shapes, Trash2 } from "lucide-react";
import type { ArchitectureCanvasMeta } from "../../types/architecture";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "../ui/dropdown-menu";
import { ConfirmDialog } from "../settings/ConfirmDialog";
import { toast } from "../Toast";
import { canvasErrorMessage, revealCanvasFilePath } from "./canvas-files";
import { formatCanvasTime } from "./canvas-file-list";
import { CanvasNameDialog } from "./CanvasNameDialog";

/**
 * 画板文件栏：画布右上角悬浮条——画板列表（打开/定位/删除）、新建、重命名。
 * 画板文件统一保存在应用资源目录，此处提供管理入口。
 */
export function CanvasFileBar({
  current,
  canvases,
  onOpen,
  onCreate,
  onRename,
  onDelete,
}: {
  current: { id: string | null; name: string };
  canvases: ArchitectureCanvasMeta[];
  onOpen: (canvasId: string) => void;
  onCreate: () => void;
  onRename: (name: string) => void;
  onDelete: (canvasId: string) => void;
}) {
  const [menuOpen, setMenuOpen] = useState(false);
  const [renameOpen, setRenameOpen] = useState(false);
  const [deleting, setDeleting] = useState<ArchitectureCanvasMeta | null>(null);

  const handleReveal = async (canvasId: string) => {
    try {
      const path = await revealCanvasFilePath(canvasId);
      await revealItemInDir(path);
    } catch (error) {
      toast.error(`无法打开画板所在目录：${canvasErrorMessage(error)}`);
    }
  };

  return (
    <div className="ai-arch-filebar">
      <button
        type="button"
        className="ai-arch-filebar-icon-btn"
        title="新建画板"
        onClick={() => void onCreate()}
      >
        <FilePlus2 size={14} strokeWidth={1.9} />
      </button>
      <DropdownMenu open={menuOpen} onOpenChange={setMenuOpen}>
        <DropdownMenuTrigger asChild>
          <button type="button" className="ai-arch-filebar-name" title="画板文件（⌘/Ctrl+S 保存）">
            <Shapes size={13} strokeWidth={1.9} />
            <span className="ai-arch-filebar-name-text">{current.name}</span>
            <ChevronDown size={12} strokeWidth={1.9} />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="ai-arch-filebar-menu">
          <DropdownMenuLabel>画板文件（{canvases.length}）</DropdownMenuLabel>
          {canvases.length === 0 ? (
            <div className="ai-arch-filebar-empty">暂无已保存画板，开始绘制后自动保存</div>
          ) : (
            canvases.map((item) => (
              <DropdownMenuItem
                key={item.id}
                className="ai-arch-filebar-item"
                onSelect={() => onOpen(item.id)}
              >
                <Check
                  size={13}
                  strokeWidth={2}
                  className={item.id === current.id ? "" : "invisible"}
                />
                <span className="ai-arch-filebar-item-text">
                  <span className="ai-arch-filebar-item-name">{item.name}</span>
                  <span className="ai-arch-filebar-item-time">
                    {formatCanvasTime(item.updatedAt)}
                  </span>
                </span>
                <span className="ai-arch-filebar-item-actions">
                  <button
                    type="button"
                    className="ai-arch-filebar-icon-btn"
                    title="在文件夹中显示"
                    onClick={(event) => {
                      event.stopPropagation();
                      void handleReveal(item.id);
                    }}
                  >
                    <FolderOpen size={12} strokeWidth={1.9} />
                  </button>
                  <button
                    type="button"
                    className="ai-arch-filebar-icon-btn is-danger"
                    title="删除画板"
                    onClick={(event) => {
                      event.stopPropagation();
                      setMenuOpen(false);
                      setDeleting(item);
                    }}
                  >
                    <Trash2 size={12} strokeWidth={1.9} />
                  </button>
                </span>
              </DropdownMenuItem>
            ))
          )}
          <DropdownMenuSeparator />
          <DropdownMenuItem
            onSelect={() => {
              setMenuOpen(false);
              void onCreate();
            }}
          >
            <FilePlus2 size={13} strokeWidth={1.9} />
            新建画板
          </DropdownMenuItem>
          <DropdownMenuItem
            onSelect={() => {
              setMenuOpen(false);
              setRenameOpen(true);
            }}
          >
            <Pencil size={13} strokeWidth={1.9} />
            重命名当前画板
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <CanvasNameDialog
        open={renameOpen}
        title="重命名画板"
        initialName={current.name}
        onCancel={() => setRenameOpen(false)}
        onSubmit={(name) => {
          setRenameOpen(false);
          void onRename(name);
        }}
      />
      <ConfirmDialog
        open={deleting !== null}
        title="删除画板"
        description={`删除后「${deleting?.name ?? ""}」的画板文件将从应用目录移除，此操作不可撤销。`}
        onCancel={() => setDeleting(null)}
        onConfirm={() => {
          const target = deleting;
          setDeleting(null);
          if (target) void onDelete(target.id);
        }}
      />
    </div>
  );
}
