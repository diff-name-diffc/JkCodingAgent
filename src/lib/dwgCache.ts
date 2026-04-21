export interface DwgCacheFingerprintInput {
  projectPath: string;
  filePath: string;
  fileSize: number;
  fileMtime: number;
  parserVersion: string;
}

export function buildDwgCacheFingerprint(input: DwgCacheFingerprintInput): string {
  return [
    input.projectPath,
    input.filePath,
    String(input.fileSize),
    String(input.fileMtime),
    input.parserVersion,
  ].join("::");
}
