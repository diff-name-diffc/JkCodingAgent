
export const LARGE_FILE_LINE_HEIGHT = 22;
export const LARGE_FILE_OVERSCAN = 40;
export const LARGE_FILE_CHUNK_SIZE = 200;

export interface FileMeta {
  sizeBytes: number;
  lineCount: number;
  isText: boolean;
}

export interface RopeMeta {
  lineCount: number;
  charCount: number;
  byteLen: number;
}
