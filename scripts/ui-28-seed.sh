#!/bin/bash
# UI-28 运行态验收辅助：隔离 HOME + 测试项目种子数据。
# 用途见 docs/ui-redesign-2026-09-06/03-tasks.md 第 22 节。
#
# 产出：
#   $ISO_HOME            全新隔离 HOME（应用以 HOME=<该目录> 启动即数据隔离，
#                        全新目录保证 dispatcher DB 以当前 v5 基线创建）
#   $ISO_PROJECTS/proj-a git 仓库：56 提交（>50 触发历史「加载更多」分页）
#                        + 工作区三态（未暂存修改 / 未跟踪文件；暂存留待 UI 走查中操作）
#   $ISO_PROJECTS/proj-b 非 git 目录（项目切换 / 文件树 / 浏览器目标页）
#
# 重复执行会先清空上述目录重建。
set -euo pipefail

ISO_HOME="${ISO_HOME:-/tmp/jka-m4}"
ISO_PROJECTS="${ISO_PROJECTS:-/tmp/jka-m4-projects}"

rm -rf "$ISO_HOME" "$ISO_PROJECTS"
mkdir -p "$ISO_HOME" "$ISO_PROJECTS/proj-a" "$ISO_PROJECTS/proj-b"

# ---------- proj-a：git 仓库 ----------
cd "$ISO_PROJECTS/proj-a"
git init -q -b main
git config user.email "m4@test.local"
git config user.name "M4 Tester"
git config commit.gpgsign false
mkdir -p history
for i in $(seq 1 55); do
  echo "line $i from commit $i" >> history/log.txt
  git add -A
  git commit -qm "chore: 变更 $i"
done
cat > readme.md <<'EOF'
# proj-a

UI-28 功能回归测试项目 A（git）。
用于验证：变更 → diff → 暂存 → 提交流程。
EOF
cat > app.py <<'EOF'
def add(a, b):
    return a + b


if __name__ == "__main__":
    print("sum:", add(1, 2))
EOF
cat > notes.md <<'EOF'
# proj-a 笔记

- 文件树/编辑器流程用
- 长路径测试：src/deep/nested/path/module/very/long/directory/structure/example_file_name.ts
EOF
mkdir -p src/deep/nested/path/module/very/long/directory/structure
echo "export const hello = 'world';" > src/deep/nested/path/module/very/long/directory/structure/example_file_name.ts
git add -A
git commit -qm "feat: 添加示例文件（readme/app/notes/src）"
# 工作区三态（重造未暂存修改 + 未跟踪文件；已暂存态由走查过程在 UI 内操作）
echo "modified line for unstaged diff — UI-28 验收" >> history/log.txt
cat > untracked-scratch.txt <<'EOF'
未跟踪文件，用于 GitChanges 未暂存区验证。
EOF

# ---------- proj-b：非 git 目录 ----------
cd "$ISO_PROJECTS/proj-b"
mkdir -p docs
cat > index.html <<'EOF'
<!doctype html>
<html>
<head><meta charset="utf-8"><title>proj-b</title></head>
<body><h1>proj-b 本地页面</h1><p>UI-28 浏览器流程目标页。</p></body>
</html>
EOF
cat > docs/guide.md <<'EOF'
# proj-b 指南

非 git 项目，用于文件树、编辑器与项目切换验证。
EOF

echo "seed done: $ISO_HOME / $ISO_PROJECTS"
