const URL_SCHEME_PATTERN = /^[a-zA-Z][a-zA-Z\d+.-]*:/;

/** Preserve explicit browser URL schemes; treat bare input as an HTTPS address. */
export function normalizeBrowserUrlInput(input: string): string {
  const url = input.trim();
  return URL_SCHEME_PATTERN.test(url) ? url : `https://${url}`;
}
