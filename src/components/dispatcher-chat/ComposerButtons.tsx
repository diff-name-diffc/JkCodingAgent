import { memo } from "react";
import { Brain } from "lucide-react";
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

export const ThinkingToggleButton = memo(function ThinkingToggleButton({
  active,
  disabled,
  onToggle,
}: {
  active: boolean;
  disabled?: boolean;
  onToggle: () => void;
}) {
  return (
    <button
      type="button"
      style={styles.thinkingToggleBtn(active)}
      onClick={onToggle}
      disabled={disabled}
      title={active ? "隐藏思考内容" : "显示思考内容"}
      aria-label={active ? "隐藏思考内容" : "显示思考内容"}
      aria-pressed={active}
    >
      <Brain size={15} />
      <span>思考</span>
    </button>
  );
});
