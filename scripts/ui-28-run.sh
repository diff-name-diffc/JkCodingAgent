#!/bin/bash
# UI-28 运行态验收辅助：一键启动隔离验证环境（mock LLM + vite + 带 bundle 身份的应用）。
# 用途见 docs/ui-redesign-2026-09-06/03-tasks.md 第 22 节。
#
# 前置：cargo build（src-tauri 增量）已产出 src-tauri/target/debug/jkcodingagent；
#       如改动过 Rust 代码先重新 cargo build。前端由 vite dev server 实时提供
#       （debug 二进制加载 build.devUrl，无需 pnpm build）。
#
# 关键点：debug 裸二进制没有 bundle 身份，macOS 自动化工具的原始指针通道会被
# 安全策略拒绝（悬停显隐按钮无法点击）——本脚本用最小 Info.plist 的 .app 包装壳
# 启动同一二进制（CFBundleIdentifier=com.jk.jkcodingagent）解锁该通道。
#
# 停止：kill <app pid>；lsof -ti :1420,8802 | xargs kill
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ISO_HOME="${ISO_HOME:-/tmp/jka-m4}"
WORK_DIR="${WORK_DIR:-/tmp/jka-ui28-run}"
BIN="$REPO_ROOT/src-tauri/target/debug/jkcodingagent"
APP="$WORK_DIR/JKCodingAgent.app"

[ -x "$BIN" ] || { echo "缺少 $BIN，先 cargo build"; exit 1; }

# 1) 种子数据（隔离 HOME + 测试项目）
ISO_HOME="$ISO_HOME" bash "$REPO_ROOT/scripts/ui-28-seed.sh"

mkdir -p "$WORK_DIR"

# 2) mock LLM（:8802）
lsof -ti :8802 | xargs kill 2>/dev/null || true
node "$REPO_ROOT/scripts/ui-28-mock-llm.mjs" > "$WORK_DIR/mock.log" 2>&1 &

# 3) vite dev server（:1420）
if ! lsof -ti :1420 >/dev/null 2>&1; then
  (cd "$REPO_ROOT" && pnpm dev > "$WORK_DIR/vite.log" 2>&1 &)
  sleep 4
fi
curl -sf -o /dev/null http://localhost:1420/ || { echo "vite 未就绪"; exit 1; }

# 4) .app 包装壳（提供 bundle 身份）
mkdir -p "$APP/Contents/MacOS"
cat > "$APP/Contents/Info.plist" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key><string>com.jk.jkcodingagent</string>
    <key>CFBundleName</key><string>JKCodingAgent</string>
    <key>CFBundleExecutable</key><string>jkcodingagent</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>0.2.1</string>
    <key>CFBundleVersion</key><string>0.2.1</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
EOF
ln -sf "$BIN" "$APP/Contents/MacOS/jkcodingagent"

# 5) 启动应用（隔离 HOME）
HOME="$ISO_HOME" "$APP/Contents/MacOS/jkcodingagent" > "$WORK_DIR/app.log" 2>&1 &
APP_PID=$!
sleep 6

echo "应用已启动 pid=$APP_PID（HOME=$ISO_HOME）"
echo "接下来：设置 → 模型服务 → 对话模型添加条目（URL http://127.0.0.1:8802/v1，Key 任意），"
echo "       模型用途绑定「聊天主模型」「项目主模型」；走查清单见 03-tasks.md 第 22 节。"
