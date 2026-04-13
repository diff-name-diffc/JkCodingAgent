<p align="center">
  <img src="docs/images/logo.png" alt="JKCodingAgent Logo" width="150" />
</p>

<h1 align="center">JKCodingAgent: An Agent-First Application For Vibe Coding</h1>

<p align="center">
  Desktop task manager for AI coding agents (Claude Code & Codex)
</p>

---

JKCodingAgent is an Agent-First Vibe Coding desktop application built for true parallel programming. It lets Claude Code and Codex run together across multiple projects, while unifying task lifecycle tracking, a native terminal experience, session playback, code browsing, and a complete Git workflow in one interface.

## Features

- **Multi-Project Workspace** — Switch between projects instantly with a single click
- **Task Lifecycle Management** — Create, track, and manage AI coding tasks across projects
- **Native Terminal** — Built-in terminal with full PTY support for agent interaction
- **Session Playback** — Review and replay agent conversation sessions (JSONL)
- **Git Integration** — Stage, commit, push, pull, and browse diffs without leaving the app
- **Code Browser** — View and navigate project files with syntax highlighting
- **Analytics Dashboard** — Track weekly token usage per agent and project

## Installation

Before using JKCodingAgent, ensure that you have installed Claude Code / Codex.

Upon the first installation on macOS, you might encounter a security prompt. You can resolve this by executing the following command:

```bash
xattr -rd com.apple.quarantine /Applications/JKCodingAgent.app
```

## Development

```bash
pnpm install          # Install dependencies
pnpm dev              # Start Vite dev server (port 1420)
pnpm build            # TypeScript check + Vite build
pnpm tauri dev        # Launch full desktop app (auto-starts dev server)
pnpm tauri build      # Build production desktop binary
```

## Tech Stack

- **Frontend**: React 19 + TypeScript + Vite
- **Desktop Shell**: Tauri 2 + Rust
- **Terminal**: xterm.js
- **Syntax Highlighting**: Shiki + CodeMirror

## License

GPL-3.0
