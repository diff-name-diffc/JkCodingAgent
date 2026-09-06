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

/** 路径的目录部分（含结尾分隔符）；根下文件返回空串。 */
export function getPathDirectory(path: string) {
  const separatorIndex = findLastSeparatorIndex(path);
  return separatorIndex === -1 ? "" : path.slice(0, separatorIndex + 1);
}

/**
 * 长路径中间折叠：超出 maxChars 时保留头部与尾部、中间以省略号相连。
 * 头部略短于尾部——路径的可辨识段（文件名、深层目录）通常在尾部。
 */
export function collapseMiddlePath(path: string, maxChars: number) {
  if (path.length <= maxChars || maxChars < 2) {
    return path;
  }
  const budget = maxChars - 1;
  const headLength = Math.max(1, Math.floor(budget * 0.45));
  const tailLength = Math.max(1, budget - headLength);
  return `${path.slice(0, headLength)}…${path.slice(path.length - tailLength)}`;
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
