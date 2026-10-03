#!/usr/bin/env bash
# install-app.sh — 将 cargo tauri build 的产物 JKCodingAgent.app 覆盖安装到 /Applications。
#
# 用法：
#   scripts/install-app.sh           # 交互确认后安装
#   scripts/install-app.sh -y        # 跳过确认，适合串联：
#                                     #   cargo tauri build && scripts/install-app.sh -y --open
#   scripts/install-app.sh --open    # 安装完成后立即启动应用
#   scripts/install-app.sh -h        # 查看帮助
#
# 行为细节：
# - ditto 拷贝（保留 ad-hoc 签名与扩展属性），先删旧 .app 再拷，避免残留混入；
# - 应用数据都在 ~/.jkcodingagent，覆盖 .app 不影响任何用户数据；
# - 若应用正在运行，先温和退出（AppleScript quit，不强杀）再覆盖；
# - 安装后清理 quarantine 属性兜底（本地构建一般没有，幂等无害），并弹系统通知。
#
# 编码注意：本机 bash 3.2 会把紧跟变量的多字节字符（全角括号等）并入变量名，
# set -u 下误报 unbound——所有变量引用一律用 ${VAR} 花括号形式。

set -euo pipefail

APP_NAME="JKCodingAgent.app"
DEST="/Applications/${APP_NAME}"
ROOT="$(cd "$(dirname "$0")" && pwd)"
SRC="${ROOT}/src-tauri/target/release/bundle/macos/${APP_NAME}"
ASSUME_YES=0
OPEN_AFTER=0

usage() {
  # 帮助文本 = 头部注释第 2 行起、到「编码注意」段之前——按标记截取而非
  # 行号硬编码，头部注释增删行不会悄悄打偏区间。
  sed -e '1d' -e '/^# 编码注意/,$d' -e 's/^# \{0,1\}//' "$0"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -y|--yes) ASSUME_YES=1 ;;
    --open) OPEN_AFTER=1 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "未知参数：$1" >&2; usage; exit 1 ;;
  esac
  shift
done

if [[ ! -d "${SRC}" ]]; then
  echo "未找到构建产物：${SRC}" >&2
  echo "请先构建：cargo tauri build（或 pnpm tauri build）" >&2
  exit 1
fi

SRC_VERSION="$(defaults read "${SRC}/Contents/Info.plist" CFBundleShortVersionString 2>/dev/null || echo '?')"
SRC_MTIME="$(date -r "${SRC}/Contents/Info.plist" '+%Y-%m-%d %H:%M')"
echo "产物：${SRC}"
echo "版本：v${SRC_VERSION}（构建于 ${SRC_MTIME}）"
echo "目标：${DEST}"
echo

# 进程名：打包产物为 productName（JKCodingAgent），开发构建为小写二进制名
# （jkcodingagent），两者都检测。
is_running() {
  pgrep -x jkcodingagent >/dev/null 2>&1 || pgrep -x JKCodingAgent >/dev/null 2>&1
}

if is_running; then
  echo "提示：JKCodingAgent 正在运行，安装时会先温和退出（未保存状态由应用自身退出流程处理）。"
fi

if [[ "$ASSUME_YES" -eq 0 ]]; then
  read -r -p "覆盖安装到 ${DEST} ？[y/N] "
  [[ "$REPLY" == [yY] || "$REPLY" == [yY][eE][sS] ]] || { echo "已取消。"; exit 0; }
fi

if is_running; then
  osascript -e 'tell application "JKCodingAgent" to quit' >/dev/null 2>&1 || true
  for _ in 1 2 3 4 5 6 7 8; do
    is_running || break
    sleep 1
  done
  if is_running; then
    echo "应用未能在 8 秒内退出，已中止安装（未做任何修改）。请手动退出后重试。" >&2
    exit 1
  fi
fi

rm -rf -- "${DEST}"
ditto "${SRC}" "${DEST}"
xattr -rd com.apple.quarantine "${DEST}" 2>/dev/null || true

echo "已安装：${DEST}"
osascript -e "display notification \"JKCodingAgent v${SRC_VERSION} 已安装到 /Applications\" with title \"JKCodingAgent 安装完成\"" >/dev/null 2>&1 || true

if [[ "$OPEN_AFTER" -eq 1 ]]; then
  open "${DEST}"
  echo "已启动 JKCodingAgent。"
else
  echo "启动：open ${DEST}（或给本脚本加 --open 安装后直接启动）"
fi
