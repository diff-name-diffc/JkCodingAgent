import { Maximize2, Minimize2 } from "lucide-react";
import { Button } from "../ui/button";

/** 「扩大占满主区 ↔ 还原双栏布局」切换（GraphPanelHeader 与 GraphPlanListView 共用）。 */
export function ExpandMainAreaButton({
  expanded,
  onClick,
}: {
  expanded: boolean;
  onClick: () => void;
}) {
  const label = expanded ? "还原双栏布局" : "扩大占满主区";
  return (
    <Button variant="ghost" size="icon-sm" aria-label={label} title={label} onClick={onClick}>
      {expanded ? <Minimize2 className="h-4 w-4" /> : <Maximize2 className="h-4 w-4" />}
    </Button>
  );
}
