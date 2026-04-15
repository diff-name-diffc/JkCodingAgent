import { resolveFilePresentation } from "./resolveFilePresentation";
import { getSetiIconSvgMarkup } from "./setiIconRegistry";
import type { FilePresentation } from "./types";

export function FileGlyph({
  presentation,
  name,
  path,
  extension,
  isDir = false,
  size = 20,
  className,
}: {
  presentation?: FilePresentation;
  name?: string;
  path?: string;
  extension?: string;
  isDir?: boolean;
  size?: number;
  className?: string;
}) {
  const resolved =
    presentation ??
    resolveFilePresentation({
      name,
      path,
      extension,
      isDir,
    });

  const iconMarkup = getSetiIconSvgMarkup(resolved);

  return (
    <span
      className={className}
      style={{
        width: size,
        height: size,
        display: "inline-flex",
        alignItems: "center",
        justifyContent: "center",
        flexShrink: 0,
      }}
    >
      <span
        aria-hidden="true"
        style={{
          width: "100%",
          height: "100%",
          display: "block",
        }}
        dangerouslySetInnerHTML={{ __html: iconMarkup }}
      />
    </span>
  );
}
