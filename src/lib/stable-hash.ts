/**
 * Fast, non-crypto hash (FNV-1a) for stable content identification — python
 * 代码块运行记录键（python_runs 表 PK 的一部分）等场景使用。两条 markdown
 * 管线共用，算法不可变更（改动即破坏存量运行记录的匹配）。
 */
export function stableHash(text: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) {
    h ^= text.charCodeAt(i);
    h = (h * 0x01000193) >>> 0;
  }
  return h.toString(36);
}
