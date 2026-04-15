import { useId } from "react";
import { getFileIconSpec } from "./iconRegistry";
import { resolveFilePresentation } from "./resolveFilePresentation";
import type { FileIconGlyph, FilePresentation } from "./types";

function useSafeSvgId(prefix: string): string {
  const reactId = useId();
  return `${prefix}-${reactId.replace(/[^a-zA-Z0-9_-]/g, "")}`;
}

function renderMonogram(
  text: string,
  x: number,
  y: number,
  size: number,
  fill: string,
  weight = 800,
) {
  return (
    <text
      x={x}
      y={y}
      textAnchor="middle"
      fontSize={size}
      fontWeight={weight}
      letterSpacing={text.length > 2 ? "0.15" : "0.05"}
      fill={fill}
      fontFamily="'JetBrains Mono', 'SFMono-Regular', monospace"
    >
      {text}
    </text>
  );
}

function renderGlyph(glyph: FileIconGlyph, accent: string, soft: string, ink: string) {
  switch (glyph) {
    case "react":
      return (
        <>
          <circle cx="10.2" cy="10.2" r="1.1" fill={accent} />
          <ellipse cx="10.2" cy="10.2" rx="4.3" ry="1.7" stroke={accent} strokeWidth="1.05" fill="none" />
          <ellipse
            cx="10.2"
            cy="10.2"
            rx="4.3"
            ry="1.7"
            stroke={accent}
            strokeWidth="1.05"
            fill="none"
            transform="rotate(60 10.2 10.2)"
          />
          <ellipse
            cx="10.2"
            cy="10.2"
            rx="4.3"
            ry="1.7"
            stroke={accent}
            strokeWidth="1.05"
            fill="none"
            transform="rotate(120 10.2 10.2)"
          />
        </>
      );
    case "vue":
      return (
        <>
          <path d="M4.85 6.1h2.45l2.9 4.55L13.1 6.1h2.25L10.2 14.25z" fill={soft} />
          <path d="M6.6 6.1h1.45l2.15 3.35 2.1-3.35h1.35l-3.45 5.4z" fill={ink} />
        </>
      );
    case "svelte":
      return (
        <path
          d="M12.65 5.55c-.48-.52-1.25-.82-2.2-.82-1.7 0-2.75.9-2.75 2.18 0 1.15.73 1.72 2.08 2.04l.82.2c.72.17 1.02.35 1.02.8 0 .53-.45.83-1.15.83-.73 0-1.38-.3-2-.92L7.35 11c.72.87 1.85 1.35 3.15 1.35 1.82 0 3-.93 3-2.36 0-1.2-.77-1.84-2.25-2.18l-.8-.18c-.68-.15-.92-.33-.92-.67 0-.43.4-.73 1.03-.73.58 0 1.1.22 1.55.67z"
          fill={ink}
        />
      );
    case "astro":
      return (
        <>
          <path d="M7.3 4.95h5.6l2.3 7.15-2.15-1.1-1.35-4.1-1.5 4.25-2.1 1.05z" fill={soft} />
          <path d="M8.35 11.05c.38.67 1.05 1.05 1.93 1.05.9 0 1.6-.38 2.05-1.08-.2 1.02-1 2.15-2.55 2.15-1.43 0-2.35-.93-2.45-2.1.28.3.62.56 1.02.74z" fill={ink} />
        </>
      );
    case "markdown":
      return (
        <>
          <path d="M5.1 6.65h2.05l1.2 1.7 1.2-1.7h2.15v5.25H10.2V8.95l-1.9 2.55-1.8-2.45v2.85H5.1z" fill={ink} />
          <path d="M12.1 9.2h2.95l-1.45 1.72 1.45 1.76H12.1l1.23-1.76z" fill={accent} />
        </>
      );
    case "image":
      return (
        <>
          <circle cx="13.25" cy="7.35" r="1.25" fill={accent} />
          <path d="M4.95 13.25 8 9.95l2 2.05 2.25-2.65 3.3 3.9z" fill={ink} />
        </>
      );
    case "video":
      return <path d="M7.55 7.1v6l5.2-3z" fill={ink} />;
    case "audio":
      return (
        <path
          d="M11.8 6.3v5.85a1.9 1.9 0 1 1-1-.2V7.4l3.15-.8v4.75a1.9 1.9 0 1 1-1-.22V6.3z"
          fill={ink}
        />
      );
    case "archive":
      return (
        <>
          <path d="M7.15 4.9h5.2v2.05h-5.2zM7.15 7.85h5.2v1.65h-5.2zM7.15 10.4h5.2v1.65h-5.2z" fill={ink} />
          <path d="M12.95 4.9h1.7v7.15h-1.7z" fill={accent} />
        </>
      );
    case "database":
      return (
        <>
          <ellipse cx="10.1" cy="6.25" rx="4.4" ry="1.6" fill={accent} />
          <path d="M5.7 6.4v4.6c0 .92 1.98 1.7 4.4 1.7s4.4-.78 4.4-1.7V6.4" fill={soft} />
          <path
            d="M5.7 8.45c0 .92 1.98 1.7 4.4 1.7s4.4-.78 4.4-1.7M5.7 10.45c0 .92 1.98 1.7 4.4 1.7s4.4-.78 4.4-1.7"
            stroke={ink}
            strokeWidth="1.02"
            fill="none"
          />
        </>
      );
    case "font":
      return <path d="M6.2 13.3 7.35 10.1h5.15l1.15 3.2h1.8L11.2 4.95H8.7l-4.35 8.35zm1.88-4.75 1.82-3.75 1.82 3.75z" fill={ink} />;
    case "lock":
      return (
        <>
          <rect x="6.05" y="8.05" width="8.1" height="5.55" rx="1.55" fill={ink} />
          <path d="M7.5 8.05V6.85a2.6 2.6 0 1 1 5.2 0v1.2" stroke={accent} strokeWidth="1.1" fill="none" />
        </>
      );
    case "key":
      return (
        <path
          d="M8.6 10.05a2.8 2.8 0 1 1 1.55 2.5l-1.35 1.35H7.5v-1.05H6.45V11.8H5.4v-1.05h1.28l1.6-1.6a2.9 2.9 0 0 1 .32-.1Zm0-1.35a1.45 1.45 0 1 0 0 2.9 1.45 1.45 0 0 0 0-2.9Z"
          fill={ink}
        />
      );
    case "certificate":
      return (
        <>
          <path d="M5.55 5.55h9.1v5.4h-9.1z" fill={soft} />
          <path d="M6.7 6.75h6.8M6.7 8.6h5.15" stroke={ink} strokeWidth="1.02" strokeLinecap="round" />
          <path d="M9.6 11.15 10.75 10l1.2 1.15v2.55l-1.2-.7-1.15.7z" fill={accent} />
        </>
      );
    case "terminal":
      return (
        <>
          <path d="m6.2 7.2 2.1 2.1-2.1 2.1" stroke={ink} strokeWidth="1.15" strokeLinecap="round" strokeLinejoin="round" fill="none" />
          <path d="M9.7 11.45h3.4" stroke={accent} strokeWidth="1.15" strokeLinecap="round" />
        </>
      );
    case "gear":
      return (
        <>
          <circle cx="10.25" cy="10.1" r="1.85" fill={accent} />
          <path
            d="m10.25 6.35.62.52.98-.22.45.8.86.34-.08.95.62.66-.62.66.08.95-.86.34-.45.8-.98-.25-.62.52-.62-.52-.98.25-.45-.8-.86-.34.08-.95-.62-.66.62-.66-.08-.95.86-.34.45-.8.98.22z"
            stroke={ink}
            strokeWidth="1"
            fill="none"
            strokeLinejoin="round"
          />
        </>
      );
    case "package":
      return (
        <>
          <path d="M5.45 6.45 10.1 4.3l4.65 2.15-4.65 2.15z" fill={soft} />
          <path d="M5.45 6.45v5.85l4.65 2.1 4.65-2.1V6.45" stroke={ink} strokeWidth="1.02" fill="none" strokeLinejoin="round" />
          <path d="M10.1 8.6v5.8" stroke={accent} strokeWidth="1.02" />
        </>
      );
    case "git":
      return (
        <>
          <circle cx="7.8" cy="7.2" r="1.05" fill={ink} />
          <circle cx="12.45" cy="7.2" r="1.05" fill={accent} />
          <circle cx="12.45" cy="12" r="1.05" fill={ink} />
          <path d="M7.8 7.2v3.15c0 .72.56 1.1 1.25 1.1h2.25M8.95 8.55l1.3-1.35h1.15" stroke={ink} strokeWidth="1.05" fill="none" strokeLinecap="round" />
        </>
      );
    case "storybook":
      return (
        <>
          <path d="M6.35 5.05h6.1c.72 0 1.3.58 1.3 1.3v6.5l-3.55-1.35-3.85 1.35z" fill={soft} />
          <path d="M7.45 6.5h4.5M7.45 8.55h3.2" stroke={ink} strokeWidth="1.02" strokeLinecap="round" />
          <path d="M10.2 5.8v6.15" stroke={accent} strokeWidth="1.02" />
        </>
      );
    case "test":
      return (
        <>
          <path d="M7.05 5.35h6.35l-1.45 2.35v1.45a2.7 2.7 0 1 1-3.45 0V7.7z" fill={soft} />
          <path d="m8.25 10.3 1.25 1.25 2.45-2.8" stroke={ink} strokeWidth="1.12" strokeLinecap="round" strokeLinejoin="round" fill="none" />
        </>
      );
    case "json":
      return (
        <>
          <path d="M8.15 5.95c-.82.32-1.2.98-1.2 1.95v.95c0 .55-.18.82-.62.98.45.17.62.45.62 1v.98c0 .97.38 1.62 1.2 1.92M12.25 5.95c.82.3 1.2.95 1.2 1.95v.95c0 .55.18.82.62.98-.45.17-.62.45-.62 1v.98c0 .95-.38 1.6-1.2 1.92" stroke={ink} strokeWidth="1.08" fill="none" strokeLinecap="round" />
          <circle cx="10.2" cy="10.3" r="0.9" fill={accent} />
        </>
      );
    case "table":
      return (
        <>
          <rect x="5.7" y="5.95" width="8.9" height="6.95" rx="1.05" fill={soft} />
          <path d="M6.35 8.2h7.6M9 6.55v5.75M11.65 6.55v5.75" stroke={ink} strokeWidth="1" />
        </>
      );
    case "globe":
      return (
        <>
          <circle cx="10.2" cy="10.1" r="4.2" stroke={ink} strokeWidth="1.02" fill="none" />
          <path d="M6.15 10.1h8.1M10.2 5.9c1.18 1.15 1.85 2.58 1.85 4.2 0 1.6-.67 3.05-1.85 4.18-1.2-1.12-1.88-2.58-1.88-4.18 0-1.62.68-3.05 1.88-4.2Z" stroke={accent} strokeWidth="1" fill="none" />
        </>
      );
    case "wasm":
      return (
        <>
          {renderMonogram("WA", 10.1, 11.75, 4.65, ink)}
          <path d="M6.55 6.2h7.1" stroke={accent} strokeWidth="1.15" strokeLinecap="round" />
        </>
      );
    case "badge":
      return renderMonogram("•", 10.2, 11.7, 6.5, accent);
    case "lines":
    default:
      return (
        <>
          <path d="M6.2 7.2h8.05M6.2 9.65h5.85M6.2 12.1h4.1" stroke={ink} strokeWidth="1.1" strokeLinecap="round" />
        </>
      );
  }
}

function renderIconAccent(iconKey: string, spec: ReturnType<typeof getFileIconSpec>, ink: string) {
  switch (iconKey) {
    case "typescript":
      return renderMonogram("TS", 10.15, 11.85, 5.2, ink);
    case "javascript":
      return renderMonogram("JS", 10.2, 11.85, 5.2, ink);
    case "python":
      return (
        <>
          <path d="M8.15 6.15h2.75c.95 0 1.7.73 1.7 1.65v2.1H9.5c-.82 0-1.45.63-1.45 1.42v.7H6.9c-.95 0-1.7-.73-1.7-1.65V8.25c0-1.18.88-2.1 2.95-2.1Z" fill={ink} />
          <path d="M12.25 13.95H9.5c-.95 0-1.7-.73-1.7-1.65v-2.1h3.1c.82 0 1.45-.63 1.45-1.42v-.73h1.15c.95 0 1.7.73 1.7 1.65v2.15c0 1.18-.88 2.1-2.95 2.1Z" fill={spec.accent} />
          <circle cx="9.15" cy="7.3" r="0.42" fill="#fff" />
          <circle cx="10.85" cy="12.8" r="0.42" fill="#fff" />
        </>
      );
    case "rust":
      return (
        <>
          <circle cx="10.2" cy="10.1" r="2.15" fill={spec.accent} />
          <path
            d="m10.2 6.15.62.52.98-.22.45.8.86.34-.08.95.62.66-.62.66.08.95-.86.34-.45.8-.98-.25-.62.52-.62-.52-.98.25-.45-.8-.86-.34.08-.95-.62-.66.62-.66-.08-.95.86-.34.45-.8.98.22z"
            stroke={ink}
            strokeWidth="1"
            fill="none"
            strokeLinejoin="round"
          />
          <circle cx="10.2" cy="10.1" r="0.8" fill={ink} />
        </>
      );
    case "go":
      return (
        <>
          {renderMonogram("GO", 10.25, 11.8, 4.9, ink)}
          <path d="M4.9 8.25h1.9M5.55 6.95h2.2" stroke={spec.accent} strokeWidth="1.05" strokeLinecap="round" />
        </>
      );
    case "yaml":
      return (
        <>
          <path d="M7 6.15 10.2 10.35 13.35 6.15" stroke={ink} strokeWidth="1.15" strokeLinecap="round" strokeLinejoin="round" fill="none" />
          <path d="M10.2 10.35v3.1" stroke={spec.accent} strokeWidth="1.15" strokeLinecap="round" />
        </>
      );
    case "toml":
      return (
        <>
          <path d="M6.2 8.05h7.9M6.2 10.2h7.9M6.2 12.35h7.9" stroke={ink} strokeWidth="1.02" strokeLinecap="round" />
          <circle cx="8.35" cy="8.05" r="0.9" fill={spec.accent} />
          <circle cx="11.8" cy="10.2" r="0.9" fill={spec.accent} />
          <circle cx="9.75" cy="12.35" r="0.9" fill={spec.accent} />
        </>
      );
    case "docker":
      return (
        <>
          <path d="M6.35 10.2h5.1v1.15h-5.1zm1.15-1.45h1.3V10H7.5zm1.65 0h1.3V10h-1.3zm1.65 0h1.3V10H10.8zm.3 2.6c.62 0 1.05-.2 1.4-.58.22-.22.4-.53.53-.92.43.02.88-.1 1.25-.38.12-.1.22-.2.33-.33l.07-.07.18.12c.22.15.48.27.77.35-.25.78-.7 1.4-1.28 1.88-.72.58-1.72.88-2.97.88H8.85c-1.45 0-2.45-.72-2.95-2.03h4.9Z" fill={ink} />
          <circle cx="13.55" cy="8.25" r="0.75" fill={spec.accent} />
        </>
      );
    case "make":
      return (
        <>
          <path d="m7.2 11.85 2.2-2.2 1.4 1.4-2.2 2.2H7.2z" fill={ink} />
          <path d="M11.05 6.2a1.6 1.6 0 0 1 2.25 2.25l-1.2 1.2-2.25-2.25z" fill={spec.accent} />
          <path d="M6.3 12.75h3.1" stroke={ink} strokeWidth="1.05" strokeLinecap="round" />
        </>
      );
    case "env":
      return (
        <>
          <path d="M9.15 5.5h2.1v2.15l2.05 3.55a2.45 2.45 0 0 1-2.12 3.7H9.25a2.45 2.45 0 0 1-2.13-3.7l2.03-3.55z" fill={spec.accentSoft} />
          <path d="M8.1 11.75h4.35" stroke={ink} strokeWidth="1.05" strokeLinecap="round" />
          <path d="M9.15 5.5h2.1" stroke={ink} strokeWidth="1.05" />
        </>
      );
    case "git":
      return (
        <>
          <circle cx="7.7" cy="7.05" r="0.95" fill={ink} />
          <circle cx="12.35" cy="7.05" r="0.95" fill={spec.accent} />
          <circle cx="12.35" cy="11.95" r="0.95" fill={ink} />
          <path d="M7.7 7.05v3.15c0 .68.53 1.05 1.2 1.05h2.2M8.9 8.4l1.2-1.35h1.2" stroke={ink} strokeWidth="1.05" fill="none" strokeLinecap="round" />
        </>
      );
    case "css":
    case "sass":
    case "c":
    case "cpp":
    case "java":
    case "kotlin":
    case "swift":
    case "ruby":
    case "php":
    case "csharp":
    case "lua":
    case "r":
    case "html":
    case "xml":
    case "protobuf":
    case "package":
    case "lock":
    case "key":
    case "certificate":
    case "font":
    case "pdf":
    case "spreadsheet":
    case "database":
    case "log":
      return renderMonogram(
        spec.badge ?? spec.key.slice(0, 3).toUpperCase(),
        10.2,
        11.8,
        (spec.badge ?? spec.key.slice(0, 3)).length > 2 ? 4.55 : 5.05,
        ink,
      );
    default:
      return renderGlyph(spec.glyph, spec.accent, spec.accentSoft, ink);
  }
}

function renderDocumentFrame(spec: ReturnType<typeof getFileIconSpec>, ink: string, softId: string) {
  return (
    <>
      <path
        d="M4.95 2.35h6.6l3.5 3.55v9.05a2.2 2.2 0 0 1-2.2 2.2H4.95a2.2 2.2 0 0 1-2.2-2.2v-10.4a2.2 2.2 0 0 1 2.2-2.2Z"
        fill={`url(#${softId})`}
        stroke={ink}
        strokeWidth="1"
      />
      <path d="M11.55 2.35v2.8c0 .64.51 1.15 1.15 1.15h2.35" fill={spec.accentSoft} fillOpacity="0.72" />
      <path d="M11.55 2.35v2.8c0 .64.51 1.15 1.15 1.15h2.35" stroke={ink} strokeWidth="1" fill="none" />
    </>
  );
}

function renderCodeFrame(spec: ReturnType<typeof getFileIconSpec>, ink: string, softId: string) {
  return (
    <>
      <path
        d="M4.3 2.35h8.2c1.55 0 2.8 1.25 2.8 2.8v9.75c0 1.55-1.25 2.8-2.8 2.8H4.3c-1.55 0-2.8-1.25-2.8-2.8V5.15c0-1.55 1.25-2.8 2.8-2.8Z"
        fill={`url(#${softId})`}
        stroke={ink}
        strokeWidth="1"
      />
      <path d="M4.1 2.9h2.45v14.2H4.1c-1.43 0-2.6-1.18-2.6-2.6V5.5c0-1.45 1.17-2.6 2.6-2.6Z" fill={spec.accent} />
      <path d="M5.05 5.15h.5M5.05 7.2h.5M5.05 9.25h.5" stroke="#fff" strokeWidth="0.92" strokeLinecap="round" />
    </>
  );
}

function renderNotebookFrame(spec: ReturnType<typeof getFileIconSpec>, ink: string, softId: string) {
  return (
    <>
      <rect x="2.35" y="2.8" width="15.15" height="13.8" rx="2.15" fill={`url(#${softId})`} stroke={ink} strokeWidth="1" />
      <path d="M6.1 2.8v13.8" stroke={spec.accent} strokeWidth="1.15" />
      <path d="M4.15 5.2h1.25M4.15 7.9h1.25M4.15 10.6h1.25M4.15 13.3h1.25" stroke={ink} strokeWidth="0.95" strokeLinecap="round" />
    </>
  );
}

function renderPanelFrame(spec: ReturnType<typeof getFileIconSpec>, ink: string, softId: string) {
  return (
    <>
      <rect x="2.25" y="3" width="15.45" height="13.25" rx="2.25" fill={`url(#${softId})`} stroke={ink} strokeWidth="1" />
      <path d="M3.8 6.05h12.35" stroke={spec.accent} strokeWidth="1.05" />
      <circle cx="5.1" cy="4.55" r="0.5" fill={spec.accent} />
      <circle cx="6.65" cy="4.55" r="0.5" fill={spec.accent} opacity="0.85" />
    </>
  );
}

function renderBoxFrame(spec: ReturnType<typeof getFileIconSpec>, ink: string, softId: string) {
  return (
    <>
      <path d="M3.25 6.2 10.1 3.1l6.65 3.1v7.35L10.1 16.7l-6.85-3.15z" fill={`url(#${softId})`} stroke={ink} strokeWidth="1" strokeLinejoin="round" />
      <path d="M3.25 6.2 10.1 9.3l6.65-3.1" stroke={ink} strokeWidth="1" fill="none" strokeLinejoin="round" />
      <path d="M10.1 9.3v7.35" stroke={spec.accent} strokeWidth="1.05" />
    </>
  );
}

function renderDatabaseFrame(spec: ReturnType<typeof getFileIconSpec>, ink: string, softId: string) {
  return (
    <>
      <ellipse cx="10.1" cy="5.1" rx="5.55" ry="2.05" fill={`url(#${softId})`} stroke={ink} strokeWidth="1" />
      <path d="M4.55 5.1v8.05c0 1.15 2.5 2.1 5.55 2.1s5.55-.95 5.55-2.1V5.1" fill={spec.accentSoft} fillOpacity="0.65" />
      <path d="M4.55 8.1c0 1.15 2.5 2.1 5.55 2.1s5.55-.95 5.55-2.1M4.55 11.05c0 1.15 2.5 2.1 5.55 2.1s5.55-.95 5.55-2.1" stroke={ink} strokeWidth="1" fill="none" />
    </>
  );
}

function renderShieldFrame(spec: ReturnType<typeof getFileIconSpec>, ink: string, softId: string) {
  return (
    <>
      <path d="M10.1 2.55 15.6 4.6v4.55c0 3.72-2.28 6.52-5.5 7.95-3.22-1.43-5.5-4.23-5.5-7.95V4.6z" fill={`url(#${softId})`} stroke={ink} strokeWidth="1" strokeLinejoin="round" />
      <path d="M10.1 3.8v12.25" stroke={spec.accent} strokeWidth="1.05" opacity="0.8" />
    </>
  );
}

function renderCircleFrame(spec: ReturnType<typeof getFileIconSpec>, ink: string, softId: string) {
  return (
    <>
      <circle cx="10.1" cy="10.1" r="7.2" fill={`url(#${softId})`} stroke={ink} strokeWidth="1" />
      <path d="M10.1 2.95v14.3" stroke={spec.accent} strokeWidth="1.02" opacity="0.38" />
    </>
  );
}

function getFrameRenderer(presentation: FilePresentation) {
  if (
    [
      "typescript",
      "tsx",
      "javascript",
      "jsx",
      "python",
      "rust",
      "go",
      "java",
      "kotlin",
      "swift",
      "ruby",
      "php",
      "csharp",
      "c",
      "cpp",
      "lua",
      "r",
      "graphql",
      "html",
      "css",
      "sass",
      "vue",
      "svelte",
      "astro",
      "protobuf",
      "test",
      "storybook",
      "git",
    ].includes(presentation.iconKey)
  ) {
    return renderCodeFrame;
  }

  if (["markdown", "notebook"].includes(presentation.iconKey)) {
    return renderNotebookFrame;
  }

  if (["image", "svg", "video", "audio", "pdf", "spreadsheet"].includes(presentation.iconKey)) {
    return renderPanelFrame;
  }

  if (["archive", "package", "docker"].includes(presentation.iconKey)) {
    return renderBoxFrame;
  }

  if (["database", "sql"].includes(presentation.iconKey)) {
    return renderDatabaseFrame;
  }

  if (["lock", "key", "certificate"].includes(presentation.iconKey)) {
    return renderShieldFrame;
  }

  if (["font", "wasm", "binary"].includes(presentation.iconKey)) {
    return renderCircleFrame;
  }

  return renderDocumentFrame;
}

function FileSvg({ presentation }: { presentation: FilePresentation }) {
  const spec = getFileIconSpec(presentation.iconKey);
  const softId = useSafeSvgId("file-surface");
  const ink = "#1F2937";
  const renderFrame = getFrameRenderer(presentation);

  return (
    <svg
      viewBox="0 0 20 20"
      width="100%"
      height="100%"
      preserveAspectRatio="xMidYMid meet"
      aria-hidden="true"
      focusable="false"
      style={{ display: "block", overflow: "visible" }}
    >
      <defs>
        <linearGradient id={softId} x1="2.5" y1="2.5" x2="17.5" y2="17.5" gradientUnits="userSpaceOnUse">
          <stop offset="0" stopColor="#FFFFFF" />
          <stop offset="0.55" stopColor={spec.accentSoft} stopOpacity="0.74" />
          <stop offset="1" stopColor={spec.accent} stopOpacity="0.18" />
        </linearGradient>
      </defs>
      {renderFrame(spec, ink, softId)}
      {renderIconAccent(presentation.iconKey, spec, ink)}
    </svg>
  );
}

function renderFolderMark(iconKey: string, accent: string, ink: string) {
  switch (iconKey) {
    case "folder-src":
      return renderMonogram("{}", 10, 12.45, 4.75, ink);
    case "folder-components":
      return (
        <>
          <rect x="6.2" y="8.35" width="3" height="3" rx="0.8" fill={ink} />
          <rect x="10.1" y="8.35" width="3.7" height="3.7" rx="0.9" fill={accent} />
        </>
      );
    case "folder-pages":
      return (
        <>
          <rect x="6.2" y="7.8" width="7.7" height="5.7" rx="0.9" fill="#fff" fillOpacity="0.9" stroke={ink} strokeWidth="0.95" />
          <path d="M8.05 9.4h3.85M8.05 11.15h2.65" stroke={accent} strokeWidth="0.95" strokeLinecap="round" />
        </>
      );
    case "folder-hooks":
      return <path d="M10.85 7.45a2.7 2.7 0 1 0 0 5.4h1.55a2.05 2.05 0 0 0 0-4.1h-.3" stroke={ink} strokeWidth="1.15" fill="none" strokeLinecap="round" />;
    case "folder-assets":
      return (
        <>
          <circle cx="12.7" cy="8.45" r="0.95" fill={accent} />
          <path d="M6 13.15 8.7 10.15l1.75 1.8 2.05-2.4 2.6 3.6z" fill={ink} />
        </>
      );
    case "folder-styles":
      return <path d="M6.15 8.1c.95-1.2 1.9-1.8 2.85-1.8 1.85 0 1.95 1.55 3.15 1.55.52 0 1.1-.18 1.7-.55-.3 1.22-.92 2.02-1.88 2.42 1 .35 1.73 1.05 2.18 2.1-1-.45-1.88-.67-2.58-.67-1.55 0-2.38.95-3.62 2.55.08-.85.12-1.47.12-1.87 0-1.23-.35-2.47-1.92-3.73Z" fill={ink} />;
    case "folder-scripts":
      return <path d="m6.15 8.65 2.1 2.1-2.1 2.1M9.65 12.9h4.05" stroke={ink} strokeWidth="1.15" strokeLinecap="round" strokeLinejoin="round" fill="none" />;
    case "folder-docs":
      return <path d="M6.4 7.7h5.55l2.15 2.15v4.1H6.4z" fill="#fff" fillOpacity="0.9" stroke={ink} strokeWidth="0.95" />;
    case "folder-tests":
      return <path d="m7.05 11.1 1.3 1.3 2.65-3.05" stroke={ink} strokeWidth="1.15" strokeLinecap="round" strokeLinejoin="round" fill="none" />;
    case "folder-public":
      return <path d="M10 6.8a3.85 3.85 0 1 1 0 7.7 3.85 3.85 0 0 1 0-7.7Zm0-1.55v10.8M4.55 10h10.9" stroke={ink} strokeWidth="1.02" fill="none" />;
    case "folder-config":
      return <path d="m10 7.05.6.5.95-.2.45.75.8.35-.1.9.6.65-.6.62.1.95-.8.32-.45.78-.95-.2-.6.5-.62-.5-.95.2-.45-.78-.8-.32.1-.95-.6-.62.6-.65-.1-.9.8-.35.45-.75.95.2z" stroke={ink} strokeWidth="1" fill="none" strokeLinejoin="round" />;
    case "folder-github":
      return (
        <>
          <path d="M10 7.2c-2.1 0-3.8 1.62-3.8 3.62 0 1.57 1.08 2.92 2.6 3.38.2.02.3-.08.3-.22v-.8c-1.05.22-1.28-.45-1.28-.45-.17-.43-.42-.55-.42-.55-.35-.22.03-.22.03-.22.37.02.58.38.58.38.35.55.9.4 1.12.32.02-.25.15-.42.25-.52-.85-.1-1.75-.42-1.75-1.9 0-.42.15-.77.4-1.05-.05-.1-.18-.5.05-1.02 0 0 .35-.1 1.1.4.32-.08.65-.12 1-.12.33 0 .68.05 1 .12.75-.5 1.1-.4 1.1-.4.23.52.1.92.05 1.02.25.28.4.63.4 1.05 0 1.48-.9 1.8-1.78 1.9.15.12.28.38.28.78v1.12c0 .15.1.25.3.2 1.52-.45 2.57-1.8 2.57-3.38 0-2-1.7-3.62-3.8-3.62Z"
            fill={ink}
          />
        </>
      );
    case "folder-node":
      return renderMonogram("N", 10, 12.5, 5.4, ink);
    case "folder-dist":
      return renderMonogram("↗", 10, 12.5, 5.2, ink);
    case "folder-build":
      return (
        <>
          <path d="M6.45 11.75h7.1M7.3 10.05h5.4M8.3 8.35h3.4" stroke={ink} strokeWidth="1.05" strokeLinecap="round" />
          <path d="M9.05 6.1h1.9" stroke={accent} strokeWidth="1.05" strokeLinecap="round" />
        </>
      );
    case "folder-git":
      return <path d="M7.4 8.2v2.6c0 .6.45.95 1.02.95h2.08M8.35 9.2l1.15-1.15h1.05" stroke={ink} strokeWidth="1.05" fill="none" strokeLinecap="round" />;
    default:
      return renderMonogram("•", 10, 12.45, 5.8, accent);
  }
}

function FolderSvg({ presentation }: { presentation: FilePresentation }) {
  const spec = getFileIconSpec(presentation.iconKey);
  const surfaceId = useSafeSvgId("folder-surface");
  const ink = "#3F2B11";

  return (
    <svg
      viewBox="0 0 20 20"
      width="100%"
      height="100%"
      preserveAspectRatio="xMidYMid meet"
      aria-hidden="true"
      focusable="false"
      style={{ display: "block", overflow: "visible" }}
    >
      <defs>
        <linearGradient id={surfaceId} x1="2" y1="4" x2="16.5" y2="16.2" gradientUnits="userSpaceOnUse">
          <stop offset="0" stopColor="#FFF0BD" />
          <stop offset="0.5" stopColor={spec.accentSoft} stopOpacity="0.95" />
          <stop offset="1" stopColor={spec.accent} stopOpacity="0.24" />
        </linearGradient>
      </defs>
      <path
        d="M2.2 6.05A2.05 2.05 0 0 1 4.25 4h3.05c.58 0 1.12.23 1.5.63l.75.8h6.1A2.15 2.15 0 0 1 17.8 7.6v6.25a2.15 2.15 0 0 1-2.15 2.15H4.35A2.15 2.15 0 0 1 2.2 13.85z"
        fill={`url(#${surfaceId})`}
        stroke={ink}
        strokeWidth="1"
      />
      <path d="M2.2 7.55h15.6v6.3A2.15 2.15 0 0 1 15.65 16H4.35A2.15 2.15 0 0 1 2.2 13.85z" fill="#FFF8E3" fillOpacity="0.62" />
      <path d="M2.9 7.55h14.2" stroke={spec.accent} strokeWidth="1.08" />
      {renderFolderMark(presentation.iconKey, spec.accent, ink)}
    </svg>
  );
}

export function FileGlyph({
  presentation,
  name,
  path,
  extension,
  isDir = false,
  size = 18,
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
      {resolved.isDir ? <FolderSvg presentation={resolved} /> : <FileSvg presentation={resolved} />}
    </span>
  );
}
