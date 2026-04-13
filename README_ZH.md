<p align="center">
  <img src="docs/images/logo.png" alt="JKCodingAgent Logo" width="150" />
</p>

<h1 align="center">JKCodingAgent: AI 编程智能体桌面任务管理器</h1>

<p align="center">
  面向 AI 编程智能体（Claude Code、Codex）的桌面任务管理器
</p>

---

JKCodingAgent 是一款专为 AI 编程场景打造的桌面应用。它把多项目管理、任务生命周期追踪、原生终端体验、会话回放、代码浏览和完整 Git 工作流整合到同一个界面里，让你不必在终端、编辑器、Git 工具和会话记录之间来回切换。

## 功能特性

- **多项目工作区** — 一键切换不同项目
- **任务生命周期管理** — 创建、追踪和管理跨项目的 AI 编程任务
- **原生终端** — 内置完整 PTY 支持的终端，直接与智能体交互
- **会话回放** — 查看和回放智能体对话会话（JSONL 格式）
- **Git 集成** — 暂存、提交、推送、拉取、查看差异，无需离开应用
- **代码浏览器** — 语法高亮的项目文件查看和导航
- **分析看板** — 追踪每个智能体和项目的每周 token 用量

## 安装

在使用 JKCodingAgent 之前，需要先安装好 Claude Code / Codex。

初次安装在 macOS 上可能会遇到安全提示，执行以下命令即可：

```bash
xattr -rd com.apple.quarantine /Applications/JKCodingAgent.app
```

## 开发

```bash
pnpm install          # 安装依赖
pnpm dev              # 启动 Vite 开发服务器（端口 1420）
pnpm build            # TypeScript 类型检查 + Vite 打包
pnpm tauri dev        # 启动完整桌面应用（自动启动开发服务器）
pnpm tauri build      # 构建生产环境桌面二进制包
```

## 技术栈

- **前端**：React 19 + TypeScript + Vite
- **桌面壳**：Tauri 2 + Rust
- **终端**：xterm.js
- **语法高亮**：Shiki + CodeMirror

## 许可证

GPL-3.0
