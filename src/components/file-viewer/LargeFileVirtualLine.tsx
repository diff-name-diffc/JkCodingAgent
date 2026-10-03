import { memo } from "react";
import { LARGE_FILE_LINE_HEIGHT } from "./large-file-types";

interface LargeFileVirtualLineProps {
  idx: number;
  text: string;
  gutterWidth: number;
}

export const LargeFileVirtualLine = memo(function LargeFileVirtualLine({
  idx,
  text,
  gutterWidth,
}: LargeFileVirtualLineProps) {
  return (
    <div
      className="ai-large-file-line"
      style={{ position: "absolute", top: idx * LARGE_FILE_LINE_HEIGHT }}
    >
      <span className="ai-large-file-gutter" style={{ width: gutterWidth }}>
        {idx + 1}
      </span>
      <span data-line={idx} className="ai-large-file-content">
        {text}
      </span>
    </div>
  );
});
