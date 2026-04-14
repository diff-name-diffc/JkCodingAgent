/**
 * Strip ANSI escape codes from terminal output text.
 * Handles: CSI sequences, OSC sequences, SGR params, cursor movement, etc.
 */
function stripAnsi(text: string): string {
  const esc = String.fromCharCode(0x1b);
  const csi = String.fromCharCode(0x9b);
  const ansiPattern = new RegExp(
    `[${esc}${csi}][[()#;?]*(?:[0-9]{1,4}(?:;[0-9]{0,4})*)?[0-9A-ORZcf-nqry=><~]`,
    "g",
  );
  return text.replace(ansiPattern, "");
}

/**
 * Clean and truncate terminal output for injection back into LLM context.
 * - Strips ANSI escape codes
 * - Removes excessive blank lines
 * - Truncates to maxChars (default 30000) keeping tail (most recent output)
 */
export function cleanTerminalOutput(
  raw: string,
  maxChars: number = 30000,
): string {
  let cleaned = stripAnsi(raw);

  // Collapse 3+ consecutive blank lines into 2
  cleaned = cleaned.replace(/\n{3,}/g, "\n\n");

  // Remove leading/trailing whitespace
  cleaned = cleaned.trim();

  // If still too long, keep the tail (most recent output is most relevant)
  if (cleaned.length > maxChars) {
    cleaned =
      "[...output truncated...]\n" + cleaned.slice(cleaned.length - maxChars);
  }

  return cleaned;
}
