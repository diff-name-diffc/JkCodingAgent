const PATH_SEPARATORS = ["/", "\\"] as const;

function findLastSeparatorIndex(path: string) {
  return Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
}

export function getPathBasename(path: string) {
  const separatorIndex = findLastSeparatorIndex(path);
  return separatorIndex === -1 ? path : path.slice(separatorIndex + 1);
}

export function buildSiblingPath(path: string, nextName: string) {
  const separatorIndex = findLastSeparatorIndex(path);
  if (separatorIndex === -1) {
    return nextName;
  }
  return `${path.slice(0, separatorIndex + 1)}${nextName}`;
}

export function isSameOrChildPath(parentPath: string, targetPath: string) {
  if (parentPath === targetPath) {
    return true;
  }

  return PATH_SEPARATORS.some((separator) => targetPath.startsWith(`${parentPath}${separator}`));
}

export function replacePathPrefix(path: string, currentPrefix: string, nextPrefix: string) {
  if (path === currentPrefix) {
    return nextPrefix;
  }

  for (const separator of PATH_SEPARATORS) {
    const prefixWithSeparator = `${currentPrefix}${separator}`;
    if (path.startsWith(prefixWithSeparator)) {
      return `${nextPrefix}${path.slice(currentPrefix.length)}`;
    }
  }

  return path;
}

export function getRelativePathDisplay(rootPath: string, path: string) {
  if (path === rootPath) {
    return ".";
  }

  for (const separator of PATH_SEPARATORS) {
    const prefixWithSeparator = `${rootPath}${separator}`;
    if (path.startsWith(prefixWithSeparator)) {
      return path.slice(prefixWithSeparator.length);
    }
  }

  return path;
}
