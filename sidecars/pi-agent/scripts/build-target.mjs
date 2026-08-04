const HOST_TRIPLES = {
  "darwin-arm64": "aarch64-apple-darwin",
  "darwin-x64": "x86_64-apple-darwin",
  "win32-arm64": "aarch64-pc-windows-msvc",
  "win32-x64": "x86_64-pc-windows-msvc",
  "linux-arm64": "aarch64-unknown-linux-gnu",
  "linux-x64": "x86_64-unknown-linux-gnu",
};

const BUILD_TARGETS = {
  "aarch64-apple-darwin": { bunTarget: "bun-darwin-arm64", extension: "" },
  "x86_64-apple-darwin": { bunTarget: "bun-darwin-x64", extension: "" },
  "aarch64-pc-windows-msvc": { bunTarget: "bun-windows-arm64", extension: ".exe" },
  "x86_64-pc-windows-msvc": { bunTarget: "bun-windows-x64", extension: ".exe" },
  "aarch64-unknown-linux-gnu": { bunTarget: "bun-linux-arm64", extension: "" },
  "x86_64-unknown-linux-gnu": { bunTarget: "bun-linux-x64", extension: "" },
  "aarch64-unknown-linux-musl": { bunTarget: "bun-linux-arm64-musl", extension: "" },
  "x86_64-unknown-linux-musl": { bunTarget: "bun-linux-x64-musl", extension: "" },
};

export function hostTargetTriple(platform, arch) {
  const triple = HOST_TRIPLES[`${platform}-${arch}`];
  if (!triple) throw new Error(`不支持的 sidecar 构建宿主：${platform}-${arch}`);
  return triple;
}

export function resolveBuildTarget(triple) {
  const target = BUILD_TARGETS[triple];
  if (!target) throw new Error(`不支持的 sidecar 目标平台：${triple}`);
  return target;
}
