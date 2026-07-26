import { CircleHelp } from "lucide-react";
import { Tooltip, TooltipContent, TooltipTrigger } from "../ui/tooltip";

/**
 * 表单标签：文本 + 可选的「?」tooltip（术语首次出现时解释）。
 * 与输入框间距 6px 由 .ai-set-field 样式保证。
 */
export function FieldLabel({ label, tip }: { label: string; tip?: string }) {
  return (
    <span className="ai-set-field-label">
      {label}
      {tip && (
        <Tooltip>
          <TooltipTrigger asChild>
            <button type="button" className="ai-set-field-help" aria-label={`${label}说明`}>
              <CircleHelp size={14} strokeWidth={1.5} />
            </button>
          </TooltipTrigger>
          <TooltipContent side="top" className="max-w-64">
            {tip}
          </TooltipContent>
        </Tooltip>
      )}
    </span>
  );
}
