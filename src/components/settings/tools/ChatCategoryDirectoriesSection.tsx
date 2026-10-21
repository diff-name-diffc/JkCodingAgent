import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { FolderOpen, FolderPlus, X } from "lucide-react";
import { Button } from "../../ui/button";
import { toast } from "../../Toast";
import { Section } from "../Section";

export function ChatCategoryDirectoriesSection({
  categoryName,
  directories,
  onAdd,
  onRemove,
}: {
  categoryName: string;
  directories: string[];
  onAdd: (directories: string[]) => void;
  onRemove: (directory: string) => void;
}) {
  const [selecting, setSelecting] = useState(false);

  async function addDirectories() {
    setSelecting(true);
    try {
      const selected = await open({
        directory: true,
        multiple: true,
        title: `选择「${categoryName}」的共享工程目录`,
      });
      if (selected) onAdd(Array.isArray(selected) ? selected : [selected]);
    } catch (error) {
      toast.error(`选择工程目录失败：${String(error)}`);
    } finally {
      setSelecting(false);
    }
  }

  return (
    <Section
      title={`${categoryName} · 工程目录`}
      description="分类内所有普通聊天会话共享这些目录，用于保存工程文件、正式产物与成果。"
      aside={<span className="ai-set-section-count">{directories.length}</span>}
    >
      {directories.length > 0 ? (
        <ul className="m-0 flex list-none flex-col gap-2 p-0" aria-label="共享工程目录">
          {directories.map((directory) => (
            <li
              key={directory}
              className="flex min-w-0 items-center gap-2 rounded-md border border-border bg-card px-3 py-2"
            >
              <FolderOpen size={16} className="shrink-0 text-muted-foreground" aria-hidden="true" />
              <span className="min-w-0 flex-1 break-all font-mono text-xs">{directory}</span>
              <Button
                type="button"
                variant="ghost"
                size="icon-sm"
                aria-label={`移除工程目录绑定：${directory}`}
                title="移除绑定（不会删除文件）"
                onClick={() => onRemove(directory)}
              >
                <X size={14} aria-hidden="true" />
              </Button>
            </li>
          ))}
        </ul>
      ) : (
        <p className="ai-set-section-description">
          尚未绑定工程目录，文件继续保存在各会话临时目录。
        </p>
      )}
      <div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={selecting}
          onClick={() => void addDirectories()}
        >
          <FolderPlus size={14} aria-hidden="true" />
          {selecting ? "正在选择…" : "添加工程目录"}
        </Button>
      </div>
      <p className="ai-set-section-description">
        临时脚本、临时测试文件和中间生成物仍放入各会话独立的临时目录。
        保存成功后，下次运行生效；移除绑定不会删除目录或文件。
      </p>
    </Section>
  );
}
