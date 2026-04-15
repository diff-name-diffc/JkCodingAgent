export type FileIconGlyph =
  | "badge"
  | "lines"
  | "markdown"
  | "image"
  | "video"
  | "audio"
  | "archive"
  | "database"
  | "font"
  | "lock"
  | "key"
  | "certificate"
  | "terminal"
  | "gear"
  | "package"
  | "git"
  | "storybook"
  | "test"
  | "react"
  | "vue"
  | "svelte"
  | "astro"
  | "json"
  | "table"
  | "globe"
  | "wasm";

export type FileCategory =
  | "folder"
  | "code"
  | "config"
  | "document"
  | "image"
  | "video"
  | "audio"
  | "archive"
  | "binary"
  | "font"
  | "database"
  | "security"
  | "package";

export interface FileIconSpec {
  key: string;
  kind: "file" | "folder";
  glyph: FileIconGlyph;
  category: FileCategory;
  accent: string;
  accentSoft: string;
  badge?: string;
}

export interface ResolveFilePresentationInput {
  name?: string;
  path?: string;
  extension?: string;
  isDir?: boolean;
}

export interface FilePresentation {
  name: string;
  path: string | null;
  normalizedName: string;
  extension: string;
  isDir: boolean;
  iconKey: string;
  category: FileCategory;
  accentColor: string;
  monacoLanguage: string;
  isMarkdown: boolean;
  isPreviewableImage: boolean;
  isBinaryLike: boolean;
}
