import { lazy, Suspense } from "react";
import type { PythonCodeRunRecord, PythonCodeRunTarget } from "../../types";
import { PythonRunDrawer } from "../dispatcher-chat/PythonRunDrawer";

const GraphPanel = lazy(() =>
  import("../graph/GraphPanel").then((module) => ({ default: module.GraphPanel })),
);

interface ChatPageOverlaysProps {
  graphPlanId: string | null;
  /**
   * 所属工作区当前是否可见（多项目保活挂载时隐藏项目为 false）。
   * portal 到 body 的覆盖层会脱离祖先 visibility:hidden，必须用此门控，
   * 否则隐藏项目的执行图会浮到可见界面之上。
   */
  workspaceVisible?: boolean;
  pythonDrawerOpen: boolean;
  pythonTarget: PythonCodeRunTarget | null;
  pythonRecord: PythonCodeRunRecord | null;
  pythonRunning: boolean;
  onCloseGraph: () => void;
  onClosePython: () => void;
  onRunPython: (target: PythonCodeRunTarget) => Promise<void>;
  onStopPython: (runId: string) => Promise<unknown>;
  onClearPython: (target: PythonCodeRunTarget) => Promise<void>;
}

export function ChatPageOverlays({
  graphPlanId,
  workspaceVisible = true,
  pythonDrawerOpen,
  pythonTarget,
  pythonRecord,
  pythonRunning,
  onCloseGraph,
  onClosePython,
  onRunPython,
  onStopPython,
  onClearPython,
}: ChatPageOverlaysProps) {
  return (
    <>
      {pythonDrawerOpen && (
        <PythonRunDrawer
          target={pythonTarget}
          record={pythonRecord}
          running={pythonRunning}
          onClose={onClosePython}
          onRun={onRunPython}
          onStop={onStopPython}
          onClear={onClearPython}
        />
      )}
      {graphPlanId && workspaceVisible && (
        <Suspense fallback={null}>
          <GraphPanel planId={graphPlanId} onClose={onCloseGraph} />
        </Suspense>
      )}
    </>
  );
}
