import type { PythonCodeRunRecord, PythonCodeRunTarget } from "../../types";
import { PythonRunDrawer } from "../dispatcher-chat/PythonRunDrawer";
import { Sheet, SheetContent } from "../ui/sheet";

interface ChatPageOverlaysProps {
  pythonDrawerOpen: boolean;
  pythonTarget: PythonCodeRunTarget | null;
  pythonRecord: PythonCodeRunRecord | null;
  pythonRunning: boolean;
  onClosePython: () => void;
  onRunPython: (target: PythonCodeRunTarget) => Promise<void>;
  onStopPython: (runId: string) => Promise<unknown>;
  onClearPython: (target: PythonCodeRunTarget) => Promise<void>;
}

/**
 * 聊天页覆盖层。执行图已迁入主区标签（UI-13），不再从这里以 portal
 * 模态渲染；当前仅承载 Python 运行详情 Sheet（Radix，UI-09）。
 */
export function ChatPageOverlays({
  pythonDrawerOpen,
  pythonTarget,
  pythonRecord,
  pythonRunning,
  onClosePython,
  onRunPython,
  onStopPython,
  onClearPython,
}: ChatPageOverlaysProps) {
  return (
    <Sheet open={pythonDrawerOpen} onOpenChange={(open) => !open && onClosePython()}>
      <SheetContent aria-label="Python 运行详情">
        <PythonRunDrawer
          target={pythonTarget}
          record={pythonRecord}
          running={pythonRunning}
          onClose={onClosePython}
          onRun={onRunPython}
          onStop={onStopPython}
          onClear={onClearPython}
        />
      </SheetContent>
    </Sheet>
  );
}
