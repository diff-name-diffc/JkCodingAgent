#!/usr/bin/env bash
# reset-dev-data.sh — 重置 JKCodingAgent 的本地开发数据，让应用回到全新初始状态。
#
# 背景：应用处于开发阶段（无存量用户），schema 已重置为 v1 基线
# （src-tauri/src/agent/db/schema.rs）。旧的 v0→v33 开发库无法自动迁移，
# 启动时会明确报错并引导运行本脚本。
#
# 用法：
#   scripts/reset-dev-data.sh            # 默认：备份数据库后仅重置数据库与迁移残留
#   scripts/reset-dev-data.sh --full     # 备份后清空整个 ~/.jkcodingagent
#                                       #（含聊天图片、memory、skills、python 运行器等）
#   scripts/reset-dev-data.sh --purge    # 不做备份直接删除（不可恢复，慎用）
#   scripts/reset-dev-data.sh --dry-run  # 只打印将执行的动作
#
# 可用 AHA_HOME 覆盖数据目录（默认 ~/.jkcodingagent），便于测试。

set -euo pipefail

ROOT="${AHA_HOME:-$HOME/.jkcodingagent}"
# 安全护栏：AHA_HOME 指向危险路径时 --full 会直接 `rm -rf`，不可恢复。
# 去掉尾部斜杠后拒绝根目录、家目录、相对路径等非数据目录取值。
while [[ "$ROOT" == */ && "$ROOT" != "/" ]]; do ROOT="${ROOT%/}"; done
case "$ROOT" in
  ""|"/"|"$HOME"|"."|"..")
    echo "拒绝执行：数据目录是危险路径（${ROOT:-<空>}），请检查 AHA_HOME 设置。" >&2
    exit 1
    ;;
esac
if [[ "$ROOT" != /* ]]; then
  echo "拒绝执行：数据目录必须是绝对路径（当前：${ROOT}）。" >&2
  exit 1
fi

MODE="db"
PURGE=0
DRY_RUN=0

usage() {
  sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'
}

for arg in "$@"; do
  case "$arg" in
    --full) MODE="full" ;;
    --purge) PURGE=1 ;;
    --dry-run) DRY_RUN=1 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "未知参数：$arg" >&2; usage; exit 1 ;;
  esac
done

if ! command -v sqlite3 >/dev/null 2>&1; then
  echo "提示：未找到 sqlite3 命令，将跳过项目内迁移残留的清理。" >&2
fi

# 打包产物的进程名是 productName（JKCodingAgent），开发构建为小写；两者都匹配，
# 否则应用实际在运行却检测不到，重置后数据库会被重新写回。
if pgrep -x jkcodingagent >/dev/null 2>&1 || pgrep -x JKCodingAgent >/dev/null 2>&1; then
  echo "警告：检测到 JKCodingAgent 正在运行，请先退出应用再重置，否则数据库会被重新写回。" >&2
  exit 1
fi

if [[ ! -e "$ROOT" ]]; then
  echo "数据目录不存在，无需重置：${ROOT}"
  exit 0
fi

# 规范化护栏：上面的字面量检查拦不住 "$HOME/."、"$HOME/.."、指向家目录的
# 符号链接等取值。目录已确认存在，解析软链与相对组件后按同一标准复查：
# 不得是根目录 / 家目录 / 顶级目录（如 /Users）。
if ! ROOT="$(cd "$ROOT" 2>/dev/null && pwd -P)"; then
  echo "拒绝执行：无法解析数据目录，请检查 AHA_HOME 设置。" >&2
  exit 1
fi
case "$ROOT" in
  ""|"/"|"$HOME")
    echo "拒绝执行：数据目录解析后是危险路径（${ROOT:-<空>}），请检查 AHA_HOME 设置。" >&2
    exit 1
    ;;
esac
if [[ "$(dirname "$ROOT")" == "/" ]]; then
  echo "拒绝执行：数据目录解析后是顶级目录（${ROOT}），请检查 AHA_HOME 设置。" >&2
  exit 1
fi

STAMP="$(date +%Y%m%d-%H%M%S)"
BACKUP_DIR="$HOME/.jkcodingagent-backups/$STAMP"

run() {
  if [[ "$DRY_RUN" -eq 1 ]]; then
    echo "[dry-run] $*"
  else
    echo "$*"
    "$@"
  fi
}

# ── 1. 备份（--purge 跳过）───────────────────────────────────────────────────
if [[ "$PURGE" -eq 0 ]]; then
  if [[ "$DRY_RUN" -eq 1 ]]; then
    echo "[dry-run] 备份 $ROOT -> $BACKUP_DIR"
  else
    mkdir -p "$BACKUP_DIR"
    # 排除 python-runner 的 venv 与运行产物（体积大且无保留价值），其余全量备份。
    # 两条路径都把内容直接铺平到 $BACKUP_DIR/，与结尾的恢复指引保持一致。
    if command -v rsync >/dev/null 2>&1; then
      rsync -a --exclude 'python-runner/' "$ROOT/" "$BACKUP_DIR/"
    else
      ( cd "$ROOT" && tar -cf - --exclude 'python-runner' . ) | ( cd "$BACKUP_DIR" && tar -xf - )
    fi
    echo "已备份到 $BACKUP_DIR"
  fi
fi

# ── 2. 项目内迁移残留（v30 之前的按项目 SSH 凭据，需先定位项目路径）──────────
if [[ "$MODE" == "db" ]]; then
  project_paths=()

  # 来源一：数据库 projects 表。查询失败时显式告警，不再静默跳过。
  if command -v sqlite3 >/dev/null 2>&1 && [[ -f "$ROOT/jkbot.sqlite3" ]]; then
    if paths_sql=$(sqlite3 "$ROOT/jkbot.sqlite3" "SELECT path FROM projects;" 2>&1); then
      while IFS= read -r project_dir; do
        [[ -n "$project_dir" ]] && project_paths+=("$project_dir")
      done <<< "$paths_sql"
    else
      echo "警告：查询 projects 表失败，项目内旧版 SSH 凭据可能清理不完全：$paths_sql" >&2
    fi
  fi

  # 来源二：projects.json（v30 之前项目登记于此、未入库；第 3 步会删除该文件，
  # 必须先解析收集路径）。
  if [[ -f "$ROOT/projects.json" ]]; then
    if command -v python3 >/dev/null 2>&1; then
      if paths_json=$(python3 -c '
import json, sys
try:
    with open(sys.argv[1], encoding="utf-8") as fh:
        data = json.load(fh)
except Exception as error:
    print(f"projects.json 解析失败：{error}", file=sys.stderr)
    sys.exit(1)
entries = data if isinstance(data, list) else data.get("projects", [])
for entry in entries:
    if isinstance(entry, dict) and entry.get("path"):
        print(entry["path"])
' "$ROOT/projects.json" 2>&1); then
        while IFS= read -r project_dir; do
          [[ -n "$project_dir" ]] && project_paths+=("$project_dir")
        done <<< "$paths_json"
      else
        echo "警告：$paths_json，项目内旧版 SSH 凭据可能清理不完全。" >&2
      fi
    else
      echo "提示：未找到 python3，无法解析 projects.json 收集项目路径。" >&2
    fi
  fi

  if [[ ${#project_paths[@]} -gt 0 ]]; then
    while IFS= read -r project_dir; do
      [[ -z "$project_dir" ]] && continue
      # 路径来自旧库/旧文件，可能被污染（"/"、家目录、相对路径）；与 ROOT
      # 同级的护栏：必须是绝对路径，且不得是根目录 / 家目录 / 顶级目录 /
      # 数据目录自身内部（数据目录由第 3 步整体处理）。
      if [[ "$project_dir" != /* || "$project_dir" == "/" || "$project_dir" == "$HOME" ]] \
        || [[ "$(dirname "$project_dir")" == "/" ]] \
        || [[ "$project_dir" == "$ROOT" || "$project_dir" == "$ROOT"/* ]]; then
        echo "警告：跳过可疑的项目路径（不做清理）：$project_dir" >&2
        continue
      fi
      for legacy in "$project_dir/.jkcodingagent/local_env/ssh" \
                    "$project_dir/.jkcodingagent/ssh-tools.json"; do
        [[ -e "$legacy" ]] && run rm -rf -- "$legacy"
      done
    done < <(printf '%s\n' "${project_paths[@]}" | sort -u)
  fi
fi

# ── 3. 删除 ──────────────────────────────────────────────────────────────────
if [[ "$MODE" == "full" ]]; then
  run rm -rf -- "$ROOT"
else
  # 数据库本体（含 WAL/SHM）与历史迁移残留（破坏性图迁移备份、失败标记、
  # 旧版按项目分键的 SSH 凭据仓库、projects.json）。
  for target in \
    "$ROOT/jkbot.sqlite3" \
    "$ROOT/jkbot.sqlite3-wal" \
    "$ROOT/jkbot.sqlite3-shm" \
    "$ROOT/projects.json" \
    "$ROOT/ssh-tools"; do
    [[ -e "$target" ]] && run rm -rf -- "$target"
  done
  if compgen -G "$ROOT/jkbot.sqlite3.pre-graph-rebuild*" >/dev/null; then
    run rm -f -- "$ROOT"/jkbot.sqlite3.pre-graph-rebuild*
  fi
fi

echo
if [[ "$DRY_RUN" -eq 1 ]]; then
  echo "预览完成（未做任何修改）。去掉 --dry-run 执行。"
else
  echo "重置完成。启动应用后将按 schema v1 基线重新初始化 ${ROOT}。"
  if [[ "$PURGE" -eq 0 ]]; then
    echo "如需恢复：备份位于 ${BACKUP_DIR}（复制回 ${ROOT} 即可）。"
  fi
fi
