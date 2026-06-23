#!/usr/bin/env bash
# 构建 RAG sidecar 二进制并按 Tauri 2.0 externalBin 约定放入 src-tauri/binaries/。
#
# Tauri sidecar 命名规则：运行时 Tauri 会在 externalBin 声明的名字后
# 自动追加当前平台的 target-triple 后缀去查找，因此产物必须重命名为：
#   rag-server-<triple>           (macOS/Linux)
#   rag-server-<triple>.exe       (Windows)
#
# 本脚本负责：
#   1. 在 rag/ 下用 PyInstaller 产出 dist/rag-server[.exe]
#   2. 探测当前平台 triple
#   3. 复制+重命名到 ../src-tauri/binaries/
#
# 用法：
#   bash rag/scripts/build_sidecar.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RAG_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$(cd "$RAG_ROOT/.." && pwd)"
BINARIES_DIR="$REPO_ROOT/src-tauri/binaries"
BINARY_NAME="rag-server"

# ---- 探测 Tauri target-triple ----
detect_triple() {
    local os_name arch triple
    os_name="$(uname -s)"
    arch="$(uname -m)"
    case "$os_name" in
        Darwin)
            if [[ "$arch" == "arm64" ]]; then triple="aarch64-apple-darwin"
            elif [[ "$arch" == "x86_64" ]]; then triple="x86_64-apple-darwin"
            else echo "ERROR: 不支持的 macOS 架构 $arch" >&2; exit 1; fi
            ;;
        Linux)
            if [[ "$arch" == "x86_64" ]]; then triple="x86_64-unknown-linux-gnu"
            elif [[ "$arch" == "aarch64" ]]; then triple="aarch64-unknown-linux-gnu"
            else echo "ERROR: 不支持的 Linux 架构 $arch" >&2; exit 1; fi
            ;;
        MINGW*|MSYS*|CYGWIN*)
            if [[ "$arch" == "x86_64" || "$arch" == "amd64" ]]; then triple="x86_64-pc-windows-msvc"
            else echo "ERROR: 不支持的 Windows 架构 $arch" >&2; exit 1; fi
            ;;
        *) echo "ERROR: 不支持的系统 $os_name" >&2; exit 1 ;;
    esac
    echo "$triple"
}

is_windows() {
    [[ "$(uname -s)" == MINGW* || "$(uname -s)" == MSYS* || "$(uname -s)" == CYGWIN* ]]
}

echo "==> [1/4] uv 同步依赖"
cd "$RAG_ROOT"
uv sync --no-dev

echo "==> [2/4] PyInstaller 打包（onefile）"
# 借助 [project.optional-dependencies].build 中的 pyinstaller
uv run --extra build pyinstaller --noconfirm --clean rag_server.spec

# 探测 PyInstaller 产物路径
if is_windows; then
    ARTIFACT="$RAG_ROOT/dist/$BINARY_NAME.exe"
else
    ARTIFACT="$RAG_ROOT/dist/$BINARY_NAME"
fi
if [[ ! -f "$ARTIFACT" ]]; then
    echo "ERROR: PyInstaller 产物未找到：$ARTIFACT" >&2
    exit 1
fi

echo "==> [3/4] 探测 target-triple"
TRIPLE="$(detect_triple)"
echo "    triple = $TRIPLE"

echo "==> [4/4] 安装到 $BINARIES_DIR/"
mkdir -p "$BINARIES_DIR"
DEST="$BINARIES_DIR/$BINARY_NAME-$TRIPLE"
if is_windows; then DEST="$DEST.exe"; fi
cp -f "$ARTIFACT" "$DEST"
echo "    -> $DEST"
echo "完成。下次 tauri build/dev 将自动使用此 sidecar。"
