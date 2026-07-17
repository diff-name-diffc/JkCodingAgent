# 主题按钮功能重构实现计划

> 创建时间：2026-07-10  
> 目标：将主题状态统一到 Zustand `useUIStore` 作为唯一权威源，消除 prop drilling；用 Radix Popover 三选菜单替换现有的二元 toggle 按钮；清理死代码与重复持久化。

---

## 1. 背景与现状分析

### 1.1 当前主题机制

| 方面 | 现状 |
|------|------|
| 主题切换原理 | `<html>` 元素上切换 `dark` CSS 类；`:root` = 浅色，`html.dark` = 深色 |
| 模式枚举 | `ThemeMode = "system" \| "dark" \| "light"`（`types.ts:10`） |
| 状态源 | `App.tsx` 中的 `useState<ThemeMode>`，初始化自 `localStorage.getItem("jkcodingagent:theme")` |
| Zustand 镜像 | `useUIStore.theme` 存在但**休眠**——注释说明仅为镜像，非权威源（`ui-store.ts:9-13`） |
| 持久化（冗余） | ① 原始 localStorage key `jkcodingagent:theme`（`App.tsx:178`）；② Zustand persist key `jkcodingagent:ui`（`ui-store.ts:67`，partialize 含 `theme`） |
| 原生窗口主题 | `getCurrentWindow().setTheme()` 跟随 `themeMode`（`App.tsx:181-187`） |
| 快捷按钮 | `SidebarFooterActions.tsx:41` — Sun/Moon 图标，二元切换（dark ↔ light） |
| 完整选择面板 | `AppSettingsDialog.tsx` ThemePanel — system/dark/light 三选 + 预览卡片 |
| RightToolbar | **无主题按钮**，且未接收任何 theme props（`ProjectPage.tsx:1197`） |

### 1.2 核心问题

1. **状态碎片化** — 三个位置并行持有主题：App 的 `useState`、原始 localStorage key、休眠的 Zustand 字段。三者可能不同步。
2. **Prop drilling** — 主题 props（`isDark`、`themeMode`、`systemPrefersDark`、`onThemeModeChange`、`onToggleTheme`）穿透 4 层组件：App → ProjectPage/WelcomePage → SessionPanel → SidebarFooterActions。每新增一个使用方就多一层传递。
3. **快捷按钮丢失 system 模式** — `handleToggleTheme`（`App.tsx:189-194`）只在 dark ↔ light 间切换。一旦点击，`system` 模式静默丢失，只能从设置面板恢复。
4. **死代码** — `App.css:1767-1791` 的 `:root[data-theme="light"]` 选择器从未被激活（无任何代码设置 `data-theme` 属性）。
5. **重复持久化** — App.tsx 手动写 `jkcodingagent:theme`，Zustand persist 也写 `jkcodingagent:ui.theme`，两份 localStorage key 存放同一信息。

### 1.3 Prop drilling 全链路

```
App.tsx (useState + useEffect 副作用)
  │  isDark, themeMode, systemPrefersDark, onThemeModeChange, onToggleTheme
  ├──→ WelcomePage.tsx (props 声明 :87-101)
  │     ├──→ SidebarFooterActions.tsx (props 声明 :12-27, 使用 :41-51)
  │     ├──→ HomeChatPage.tsx (props 声明 :24-32)
  │     │     └──→ chat-page-v2.tsx (props 声明 :77-79, 使用 :608-610 → DispatcherChatInput)
  │     │           └──→ dispatcher-chat/ (theme prop 向下传递)
  │     └──→ AppSettingsDialog.tsx (props 声明 :632-648, ThemePanel :53-256)
  │
  └──→ ProjectPage.tsx (props 声明 :160-199)
        ├──→ SessionPanel.tsx (props 声明 :42-59)
        │     └──→ SidebarFooterActions.tsx (props :370-375)
        ├──→ chat-page-v2.tsx (props :920-922)
        ├──→ FileViewer.tsx (props :22-33 → file-viewer/*)
        ├──→ FileExplorer.tsx (props :33-43, 当前 _isDark 未使用)
        ├──→ AppSettingsDialog.tsx (props :1210-1213)
        └──→ RightToolbar.tsx (未传递任何 theme props)
```

**受影响的组件清单（需要移除 theme props）：**

| 组件文件 | 当前接收的 theme props | 改造后 |
|----------|----------------------|--------|
| `App.tsx` | useState 源 + 副作用 | 仅初始化 store + system listener |
| `WelcomePage.tsx` | isDark, themeMode, systemPrefersDark, onThemeModeChange, onToggleTheme | 全部移除 |
| `ProjectPage.tsx` | 同上 | 全部移除 |
| `SessionPanel.tsx` | 同上 | 全部移除 |
| `SidebarFooterActions.tsx` | 同上 | 改为直接 `useUIStore()` |
| `HomeChatPage.tsx` | isDark, themeMode, systemPrefersDark, onThemeModeChange | 全部移除 |
| `chat-page-v2.tsx` | theme, isDark, onThemeChange | 全部移除 |
| `FileViewer.tsx` | isDark | 改为 `useUIStore()` 选择器 |
| `FileExplorer.tsx` | isDark（_isDark 未使用） | 移除 |
| `file-viewer/FileTabPane.tsx` | isDark | 改为 `useUIStore()` |
| `file-viewer/MonacoEditorPane.tsx` | isDark | 改为 `useUIStore()` |
| `AppSettingsDialog.tsx` | isDark, themeMode, systemPrefersDark, onThemeModeChange | 改为 `useUIStore()` |
| `RightToolbar.tsx` | 无（当前无按钮） | 新增主题按钮 |

---

## 2. 设计方案

### 2.1 架构决策

| 决策点 | 选择 | 理由 |
|--------|------|------|
| 状态源 | `useUIStore.theme` 升级为权威源 | 消除 useState/localStorage/Zustand 三源分裂 |
| 副作用位置 | `useUIStore.setTheme` 内部执行 | `html.dark` 切换 + Tauri 窗口主题原子化 |
| 组件获取方式 | 直接 `useUIStore()` 选择器 | 消除 prop drilling；符合 AGENTS.md「UI 状态用 Zustand」 |
| 快捷按钮形态 | Radix Popover 三选菜单 | 暴露 system/dark/light 全部三模式，不丢失 system |
| 按钮放置位置 | RightToolbar 底部（divider 后）+ SidebarFooterActions（替换现有 toggle） | 项目页和主页都有快捷入口 |
| Popover 实现 | `@radix-ui/react-popover`（已安装） | 符合 AGENTS.md「优先用 Radix 原语」 |
| isDark 派生 | `useUIStore` 新增 `resolvedIsDark` 选择器 | system 模式需运行时解析 |
| system 监听 | App.tsx 保留 `matchMedia` listener，更新 store 的 `systemPrefersDark` | 监听器只需一个，放 App 层 |

### 2.2 Store 设计

```ts
// stores/ui-store.ts — 改造后

export interface UIState {
  sidebarCollapsed: boolean;
  artifactPanelOpen: boolean;
  theme: ThemeMode;                  // 用户选择（权威源）
  systemPrefersDark: boolean;        // OS 偏好（由 App.tsx listener 更新）
  activeConversationId: string | null;
  commandPaletteOpen: boolean;

  toggleSidebar: () => void;
  setSidebarCollapsed: (collapsed: boolean) => void;

  setArtifactPanelOpen: (open: boolean) => void;
  toggleArtifactPanel: () => void;

  setTheme: (theme: ThemeMode) => void;                    // 用户调用
  setSystemPrefersDark: (dark: boolean) => void;            // App.tsx listener 调用
  getResolvedIsDark: () => boolean;                         // 派生值
  toggleTheme: () => void;                                   // 快捷二元切换（保留语义）

  setActiveConversationId: (id: string | null) => void;

  setCommandPaletteOpen: (open: boolean) => void;
  toggleCommandPalette: () => void;
}
```

**`setTheme` 副作用（原 App.tsx:176-187 逻辑）：**

```ts
setTheme: (theme) => {
  set({ theme });
  // 副作用：切换 html.dark + Tauri 窗口主题
  const resolved = theme === "system" ? get().systemPrefersDark : theme === "dark";
  document.documentElement.classList.toggle("dark", resolved);
  if ("__TAURI_INTERNALS__" in window) {
    getCurrentWindow().setTheme(theme === "system" ? null : theme).catch(console.error);
  }
},
```

**`setSystemPrefersDark` 副作用（system 模式下自动跟随 OS）：**

```ts
setSystemPrefersDark: (dark) => {
  set({ systemPrefersDark: dark });
  // 若当前为 system 模式，同步更新 html.dark
  const { theme } = get();
  if (theme === "system") {
    document.documentElement.classList.toggle("dark", dark);
  }
},
```

**`toggleTheme` 语义（快捷按钮 click 直接调用）：**

```ts
toggleTheme: () => {
  const { theme, systemPrefersDark } = get();
  const currentlyDark = theme === "system" ? systemPrefersDark : theme === "dark";
  get().setTheme(currentlyDark ? "light" : "dark");
},
```

**持久化** — Zustand `persist` partialize 保留 `sidebarCollapsed` + `theme`（不变）。移除 App.tsx 中 `localStorage.setItem("jkcodingagent:theme", ...)` 手动写入。

**迁移兼容** — 首次加载时，若 `jkcodingagent:ui` 无 `theme` 字段（旧用户），读取旧的 `jkcodingagent:theme` key 兜底，然后删除旧 key：

```ts
// stores/ui-store.ts — persist onRehydrateStorage
{
  name: "jkcodingagent:ui",
  partialize: (s) => ({ sidebarCollapsed: s.sidebarCollapsed, theme: s.theme }),
  // 迁移：旧 localStorage key → 新 store
  onRehydrateStorage: () => (state) => {
    if (state && state.theme === undefined) {
      const legacy = localStorage.getItem("jkcodingagent:theme");
      if (legacy === "dark" || legacy === "light" || legacy === "system") {
        state.theme = legacy;
      }
    }
    // 清理旧 key
    localStorage.removeItem("jkcodingagent:theme");
  },
}
```

### 2.3 新增 `useResolvedIsDark` 选择器 hook

部分组件需要 `isDark` 派生值（如 MonacoEditorPane 选 Monaco 主题、Shiki 高亮参数）。提供一个轻量选择器 hook，避免每个组件重复计算：

```ts
// hooks/useResolvedIsDark.ts
import { useUIStore } from "../stores/ui-store";

export function useResolvedIsDark(): boolean {
  const theme = useUIStore((s) => s.theme);
  const systemPrefersDark = useUIStore((s) => s.systemPrefersDark);
  return theme === "system" ? systemPrefersDark : theme === "dark";
}
```

### 2.4 新增 `ThemeToggleButton` 组件

新建 `src/components/ThemeToggleButton.tsx`，封装 Radix Popover 三选菜单，供 RightToolbar 和 SidebarFooterActions 复用。

```
┌─────────────────────────────┐
│  ThemeToggleButton          │
│  ┌───────────┐              │
│  │ ☀/🌙 icon │ ← IconButton │
│  └─────┬─────┘              │
│        │ click → toggleTheme │
│        │ (快捷二元切换)       │
│        │                     │
│        │ long-press /        │
│        │ Popover trigger     │
│        ▼                     │
│  ┌───────────────────────┐  │
│  │ ○ 跟随系统 · 深色     │  │  ← system
│  │ ○ 浅色               │  │  ← light
│  │ ● 深色               │  │  ← dark (当前)
│  └───────────────────────┘  │
└─────────────────────────────┘
```

**交互设计：**
- **单击图标** → 调用 `toggleTheme()`，dark ↔ light 快捷切换（与现有行为一致，用户肌肉记忆不变）。
- **点击 chevron / 悬停 >300ms** → 打开 Popover 三选菜单。
- **Popover 内容**：三个选项（跟随系统 / 浅色 / 深色），当前选中项有 ✓ 标记 + `is-active` 高亮。
- **键盘**：Popover 内支持 ↑↓ 导航 + Enter 选择 + Esc 关闭（Radix Popover 原生支持）。
- **图标**：当前 resolved dark → 显示 Sun（提示「点击切到浅色」）；当前 resolved light → 显示 Moon。

**组件 Props：**

```tsx
interface ThemeToggleButtonProps {
  variant: "toolbar" | "sidebar";  // 控制尺寸/样式
  size?: number;                    // 图标尺寸
}
// 内部直接调用 useUIStore()，无需外部传入任何 theme props
```

### 2.5 清理死代码

| 文件 | 行号 | 内容 | 操作 |
|------|------|------|------|
| `src/App.css` | 1767-1791 | `:root[data-theme="light"]` git-diff 覆盖 | 迁移为 `:root:not(.dark)` 选择器（浅色时生效），或合并到 base 选择器用 CSS 变量 |
| `src/App.tsx` | 45-48 | `getInitialThemeMode` + `localStorage.getItem("jkcodingagent:theme")` | 移除（迁移到 store onRehydrateStorage） |
| `src/App.tsx` | 176-179 | `useEffect` 手动写 `localStorage.setItem` | 移除（Zustand persist 已处理） |
| `src/App.tsx` | 181-187 | `useEffect` Tauri 窗口主题 | 移除（移入 `setTheme` 副作用） |
| `src/App.tsx` | 189-194 | `handleToggleTheme` | 移除（移入 store `toggleTheme`） |

### 2.6 App.tsx 改造后职责

改造后 App.tsx 仅保留：
1. 启动时调用 `useUIStore` 的 rehydrate（Zustand persist 自动完成）。
2. `systemPrefersDark` 的 `matchMedia` listener → 调用 `setSystemPrefersDark()`。
3. 初始化时确保 store 副作用执行一次（rehydrate 后触发一次 `setTheme(theme)` 以应用 `html.dark`）。

```tsx
// App.tsx 改造后核心逻辑（示意）
function App() {
  const setSystemPrefersDark = useUIStore((s) => s.setSystemPrefersDark);
  const theme = useUIStore((s) => s.theme);
  const setTheme = useUIStore((s) => s.setTheme);

  // OS 主题变化监听
  useEffect(() => {
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    setSystemPrefersDark(mq.matches);
    const handler = (e: MediaQueryListEvent) => setSystemPrefersDark(e.matches);
    mq.addEventListener("change", handler);
    return () => mq.removeEventListener("change", handler);
  }, [setSystemPrefersDark]);

  // rehydrate 后首次应用主题副作用
  useEffect(() => {
    setTheme(theme); // 触发 html.dark + Tauri 窗口
  }, []); // 仅挂载时一次

  // ...其余业务逻辑不变
}
```

---

## 3. 实施步骤

### Phase 1：Store 改造（基础设施）

**目标：** `useUIStore` 成为主题权威源，含全部副作用逻辑。

**改动文件：**
- `src/stores/ui-store.ts` — 新增 `systemPrefersDark`、`setSystemPrefersDark`、`toggleTheme`、`getResolvedIsDark`；`setTheme` 加入副作用；`onRehydrateStorage` 迁移旧 key。

**验证：**
- store 中 `setTheme("dark")` 后 `document.documentElement.classList.contains("dark")` === `true`。
- store 中 `setTheme("light")` 后 === `false`。
- 旧 `localStorage.getItem("jkcodingagent:theme")` 值能正确迁移到 store。

---

### Phase 2：新增选择器 hook + ThemeToggleButton 组件

**目标：** 提供 `useResolvedIsDark` hook 和可复用的主题按钮。

**改动文件：**
- `src/hooks/useResolvedIsDark.ts`（新建） — 选择器 hook。
- `src/components/ThemeToggleButton.tsx`（新建） — Radix Popover 三选菜单。
- `src/styles/tailwind.css` — 在 `@layer components` 新增 `.ai-theme-popover`、`.ai-theme-option`、`.ai-theme-option.is-active` 等类（命名遵循现有 `.ai-*` 惯例）。

**验证：**
- `ThemeToggleButton` 单独使用时，点击图标触发 dark ↔ light 切换。
- 点击 chevron 打开 Popover，三选菜单中点选 system/dark/light 生效。
- Popover 当前选中项有 ✓ 标记 + 高亮。

---

### Phase 3：接入新按钮 + 替换旧 toggle

**目标：** RightToolbar 和 SidebarFooterActions 使用 `ThemeToggleButton`。

**改动文件：**
- `src/components/RightToolbar.tsx` — 在 `ai-project-right-toolbar-spacer` 前插入 `<ThemeToggleButton variant="toolbar" />`。
- `src/components/SidebarFooterActions.tsx` — 移除 `isDark`/`themeMode`/`systemPrefersDark`/`onThemeModeChange`/`onToggleTheme` props；用 `<ThemeToggleButton variant="sidebar" />` 替换现有 Sun/Moon toggle 按钮。

**验证：**
- 项目页右工具栏底部出现主题按钮，功能正常。
- 侧栏底部的主题按钮功能正常。
- 两处按钮状态一致（都读同一 store）。

---

### Phase 4：消除 prop drilling（App → 页面 → 子组件）

**目标：** 移除 WelcomePage、ProjectPage、SessionPanel、HomeChatPage、chat-page-v2 上的 theme props。

**改动文件（逐个移除 theme props 声明 + 传递）：**
1. `src/App.tsx` — 移除 `themeMode` useState、`isDark` 派生、`handleToggleTheme`、手动 localStorage、Tauri 窗口 useEffect；移除向 `<WelcomePage>` 和 `<ProjectPage>` 传递的 theme props。
2. `src/components/WelcomePage.tsx` — 移除 props 声明 + 向 `SidebarFooterActions`/`HomeChatPage` 的传递。
3. `src/components/ProjectPage.tsx` — 移除 props 声明 + 向 `SessionPanel`/`chat-page-v2`/`AppSettingsDialog` 的传递。
4. `src/components/SessionPanel.tsx` — 移除 props 声明 + 向 `SidebarFooterActions` 的传递。
5. `src/components/HomeChatPage.tsx` — 移除 props 声明 + 向 `chat-page-v2`/`AppSettingsDialog` 的传递。
6. `src/components/chat-page-v2.tsx` — 移除 `theme`/`isDark`/`onThemeChange` props；内部用 `useResolvedIsDark()` 替代 `isDark`；向下游 DispatcherChatInput 的传递改为内部 `useUIStore`。

**验证：**
- `pnpm build`（tsc 类型检查 + Vite 打包）通过，无 TS 错误。
- 全仓 grep `isDark` / `themeMode` / `onThemeModeChange` / `onToggleTheme` 无残留 prop 传递（仅在 store + hook 内部）。

---

### Phase 5：文件/编辑器组件改用 store

**目标：** FileViewer、FileExplorer、FileTabPane、MonacoEditorPane 等消费 `isDark` 的组件改用 `useResolvedIsDark()`。

**改动文件：**
1. `src/components/FileViewer.tsx` — 移除 `isDark` prop，内部 `const isDark = useResolvedIsDark()`。
2. `src/components/FileExplorer.tsx` — 移除未使用的 `_isDark` prop + prop 声明。
3. `src/components/file-viewer/FileTabPane.tsx` — 移除 `isDark` prop，内部 `useResolvedIsDark()`。
4. `src/components/file-viewer/MonacoEditorPane.tsx` — 移除 `isDark` prop，内部 `useResolvedIsDark()`。
5. `src/components/ProjectPage.tsx` — 移除向 FileViewer/FileExplorer 传递的 `isDark`。

**验证：**
- Monaco 编辑器在切换 dark/light 后正确切换主题。
- Shiki 代码高亮在切换后颜色正确。
- `pnpm build` 通过。

---

### Phase 6：AppSettingsDialog ThemePanel 改造

**目标：** AppSettingsDialog 直接从 store 读写主题，移除 theme props。

**改动文件：**
- `src/components/AppSettingsDialog.tsx` — 移除 `isDark`/`themeMode`/`systemPrefersDark`/`onThemeModeChange` props；`ThemePanel` 内部用 `useUIStore()` 读写；`isDark` 用 `useResolvedIsDark()`；各调用方（ProjectPage、HomeChatPage、SidebarFooterActions）移除传递。

**验证：**
- 设置面板的三选菜单与快捷按钮状态完全同步。
- 设置面板的 system toggle 开关正常。
- 预览卡片正确显示深/浅色效果。

---

### Phase 7：清理死代码 + 旧 localStorage

**目标：** 清理 App.css 死选择器、旧 localStorage key。

**改动文件：**
- `src/App.css:1767-1791` — 将 `:root[data-theme="light"]` 选择器迁移为 `:root:not(.dark)` 或合并到 base 选择器（用 `var(--*)` 令牌替代显式覆盖）。
- `src/stores/ui-store.ts` `onRehydrateStorage` — 确认旧 key 迁移逻辑执行后清理 `localStorage.removeItem("jkcodingagent:theme")`。

**验证：**
- `grep -rn "data-theme" src/` 无结果（CSS + TSX 均无）。
- `grep -rn "jkcodingagent:theme" src/` 仅在 `onRehydrateStorage` 迁移逻辑中出现。
- Git diff 面板在浅色/深色下颜色正确。

---

## 4. 受影响文件清单

### 新建
| 文件 | 职责 |
|------|------|
| `src/hooks/useResolvedIsDark.ts` | 派生 isDark 选择器 hook |
| `src/components/ThemeToggleButton.tsx` | Radix Popover 三选主题按钮 |

### 修改
| 文件 | 改动类型 |
|------|----------|
| `src/stores/ui-store.ts` | 扩展：systemPrefersDark、副作用、toggleTheme、迁移 |
| `src/App.tsx` | 缩减：移除 useState/localStorage/handleToggleTheme，保留 listener |
| `src/components/WelcomePage.tsx` | 移除 theme props |
| `src/components/ProjectPage.tsx` | 移除 theme props（向 SessionPanel、chat-page-v2、FileViewer、AppSettingsDialog） |
| `src/components/SessionPanel.tsx` | 移除 theme props |
| `src/components/SidebarFooterActions.tsx` | 移除 theme props，使用 ThemeToggleButton |
| `src/components/HomeChatPage.tsx` | 移除 theme props |
| `src/components/chat-page-v2.tsx` | 移除 theme/isDark/onThemeChange props |
| `src/components/FileViewer.tsx` | 改用 useResolvedIsDark() |
| `src/components/FileExplorer.tsx` | 移除未使用的 isDark prop |
| `src/components/file-viewer/FileTabPane.tsx` | 改用 useResolvedIsDark() |
| `src/components/file-viewer/MonacoEditorPane.tsx` | 改用 useResolvedIsDark() |
| `src/components/AppSettingsDialog.tsx` | 改用 useUIStore()，移除 theme props |
| `src/components/RightToolbar.tsx` | 新增 ThemeToggleButton |
| `src/App.css` | 迁移/删除死 data-theme 选择器 |
| `src/styles/tailwind.css` | 新增 .ai-theme-popover 等组件类 |

### 不变
| 文件 | 原因 |
|------|------|
| `src/types.ts` | `ThemeMode` 类型不变 |
| `tailwind.config.js` | `darkMode: ["class", "html.dark"]` 不变 |
| Rust 后端 | 主题纯前端，无 Rust 改动 |

---

## 5. 验收标准

1. **功能正确**
   - 快捷按钮单击 → dark ↔ light 切换。
   - 快捷按钮展开 Popover → 可选 system/dark/light 三模式。
   - system 模式下切换 OS 深浅色 → UI 自动跟随。
   - 设置面板三选菜单与快捷按钮状态完全同步。
   - Monaco/Shiki 在主题切换后正确刷新。

2. **无 prop drilling**
   - `grep -rn "themeMode\|onThemeModeChange\|onToggleTheme" src/components/` 无结果（除 store + hook + ThemeToggleButton 内部）。
   - WelcomePage、ProjectPage、SessionPanel、HomeChatPage、chat-page-v2 的 props 接口中无 theme 相关字段。

3. **单一持久化源**
   - `localStorage.getItem("jkcodingagent:theme")` === `null`（旧 key 已清理）。
   - 主题选择仅存在于 `jkcodingagent:ui` 的 Zustand persist 中。

4. **无死代码**
   - `grep -rn "data-theme" src/` 无结果。
   - App.tsx 中无手动 `localStorage.setItem("jkcodingagent:theme", ...)`。

5. **构建通过**
   - `pnpm build` 成功（tsc 严格模式 + Vite 打包）。
   - `pnpm lint` 通过（--max-warnings 0）。

---

## 6. 风险与回退

| 风险 | 缓解 |
|------|------|
| Zustand rehydrate 时机晚于首次渲染，导致 FOUC（闪烁） | `onRehydrateStorage` 中同步读取旧 key 兜底；或在 `index.html` 的 `<head>` 中加内联脚本提前设置 `html.dark` |
| 旧用户 `jkcodingagent:ui` 中无 `theme` 字段 | `onRehydrateStorage` 迁移逻辑从 `jkcodingagent:theme` 兜底 |
| Popover 在 48px 宽工具栏中定位溢出 | Radix Popover `side="left" align="center"` + `collisionPadding` |
| `setTheme` 副作用在非浏览器环境（SSR）报错 | `typeof document !== "undefined"` 守卫（当前为 Tauri 桌面端，风险低） |

**回退方案：** 所有改动在 `refactor/ui-content` 分支进行，如出问题可 `git revert` 整个 commit 序列回退到现有 App.tsx useState 方案。

---

## 7. 推进策略

按 Phase 1→7 顺序执行，每个 Phase 独立提交一次 commit。Phase 1（store）和 Phase 2（新组件）可并行但不可交叉。Phase 4（prop drilling 消除）是最大改动面，建议拆分为多个小 commit（每个组件一个）以便 review 和回退。
