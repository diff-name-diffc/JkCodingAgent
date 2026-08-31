import { lazy, Suspense } from "react";
import type { PythonCodeRunRecord, PythonCodeRunTarget } from "../../types";
import { PythonRunDrawer } from "../dispatcher-chat/PythonRunDrawer";

const GraphPanel = lazy(() =>
  import("../graph/GraphPanel").then((module) => ({ default: module.GraphPanel })),
);

interface ChatPageOverlaysProps {
  graphPlanId: string | null;
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
      {graphPlanId && (
        <Suspense fallback={null}>
          <GraphPanel planId={graphPlanId} onClose={onCloseGraph} />
        </Suspense>
      )}
    </>
  );
}
