export function resolveKnowledgeImageUrls(
  markdown: string,
  collectionRoot: string,
  convert: (path: string) => string,
) {
  return markdown.replace(/!\[([^\]]*)\]\(([^)]+)\)/g, (match, alt: string, rawUrl: string) => {
    const url = rawUrl.trim();
    if (
      url.startsWith("http://") ||
      url.startsWith("https://") ||
      url.startsWith("data:image/") ||
      url.startsWith("asset://") ||
      url.startsWith("http://asset.localhost/")
    ) {
      return match;
    }
    const clean = url.replace(/^\.?\//, "");
    const absolute = url.startsWith("/")
      ? url
      : clean.startsWith("wiki/")
        ? `${collectionRoot}/${clean}`
        : `${collectionRoot}/wiki/${clean}`;
    return `![${alt}](${convert(absolute)})`;
  });
}
