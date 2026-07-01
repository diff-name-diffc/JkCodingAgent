import { memo } from "react";
import type { DispatcherMode } from "../../types";
import { dispatcherChatStyles as styles } from "./dispatcherChatStyles";

export const PlanModeToggleButton = memo(function PlanModeToggleButton({
  mode,
  onToggleMode,
}: {
  mode: DispatcherMode;
  onToggleMode: (mode: DispatcherMode) => void;
}) {
  const active = mode === "plan";

  return (
    <button
      type="button"
      style={styles.modeToggleBtn(active)}
      onClick={() => onToggleMode("plan")}
      title={active ? "取消 Plan 模式" : "启用 Plan 模式"}
      aria-pressed={active}
    >
      Plan
    </button>
  );
});
