## 主题按钮功能重构 — 执行计划

基于 `docs/theme-button-refactor-plan.md`，当前 7 个 Phase 全部未实施。代码现状与计划文档完全一致。以下为经过代码核查后的执行计划（保留原计划 7 阶段结构，补充核查中发现的精确行号与细节）。

---

### Phase 1：Store 改造（基础设施）
**文件：** `src/stores/ui-store.ts`

改造 `useUIStore` 成为主题唯一权威源：
- 新增字段 `systemPrefersDark: boolean`（初始 `false`）
- `setTheme` 加入副作用：`document.documentElement.classList.toggle("dark", resolved)` + Tauri `getCurrentWindow().setTheme()`（含 `typeof document !== "undefined"` 守卫）
- 新增 `setSystemPrefersDark(dark)` — 更新字段；若当前 `theme === "system"` 同步切换 `html.dark`
- 新增 `toggleTheme()` — 快捷二元切换 dark ↔ light（`systemPrefersDark` 解析）
- 新增 `getResolvedIsDark()` — 派生值
- 移除「休眠镜像」注释
- `persist` 新增 `onRehydrateStorage`：旧 `jkcodingagent:theme` key → store 迁移，迁移后 `localStorage.removeItem` 旧 key

**验证：** `setTheme("dark")` 后 `html.classList.contains("dark")` 为 true；旧 key 能迁移。

---

### Phase 2：选择器 hook + ThemeToggleButton 组件 + 样式
**新建文件：**
- `src/hooks/useResolvedIsDark.ts` — 选择器 hook（`useUIStore` 读 `theme` + `systemPrefersDark`，派生 boolean）
- `src/components/ThemeToggleButton.tsx` — Radix Popover 三选菜单

**ThemeToggleButton 设计（匹配现有 Popover 约定）：**
- `import * as Popover from "@radix-ui/react-popover"`（直接使用，不建 shadcn wrapper）
- 受控 `open`/`onOpenChange` state
- `Popover.Trigger asChild` 包裹 `<button className="ai-sidebar-tool-button">`（trigger 继承现有按钮样式，IconButton 不支持 asChild/forwardRef 故不用）
- 图标：resolved dark → Sun（提示切浅色）；resolved light → Moon
- `Popover.Content side="top" align="center" sideOffset={8} className="ai-theme-popover"`，内含三行 `.ai-theme-option`（跟随系统/浅色/深色），当前项加 `.is-selected` + ✓
- Props: `variant: "toolbar" | "sidebar"`（控制尺寸/对齐）
- 内部直接 `useUIStore()`，无需外部传 theme props

**修改文件：**
- `src/styles/tailwind.css` `@layer components` 内新增 `.ai-theme-popover`、`.ai-theme-option`、`.ai-theme-option.is-selected`（参照 `.ai-usage-popover` 和 `.ai-tool-option` 风格）

**验证：** 独立使用按钮，单击切 dark↔light；展开 Popover 可选三模式；选中项有 ✓ + 高亮。

---

### Phase 3：接入新按钮 + 替换旧 toggle
**文件：**
- `src/components/RightToolbar.tsx` — 在 `ai-project-right-toolbar-spacer`（:54）前插入 `<ThemeToggleButton variant="toolbar" />`
- `src/components/SidebarFooterActions.tsx` — 用 `<ThemeToggleButton variant="sidebar" />` 替换 :41-51 的 Sun/Moon toggle button

**验证：** 项目页右工具栏 + 侧栏底部主题按钮功能正常，两处状态一致。

---

### Phase 4：消除 prop drilling（App → 页面 → 子组件）
逐个移除 theme props 声明 + 传递：
1. `src/App.tsx` — 移除 `themeMode` useState(:53)、`isDark` 派生(:55)、`getInitialThemeMode`(:45-48)、手动 localStorage `useEffect`(:176-179)、Tauri 窗口 `useEffect`(:181-187)、`handleToggleTheme`(:189-194)；移除向 `<ProjectPage>`(:503-507) 和 `<WelcomePage>`(:525-529) 传递的 theme props；保留 `matchMedia` listener(:166-174) 改为调用 `useUIStore.setSystemPrefersDark`；rehydrate 后触发一次 `setTheme` 应用副作用
2. `src/components/WelcomePage.tsx` — 移除 :87-101 的 5 个 theme props 声明 + :158-162/:170-173 的传递
3. `src/components/ProjectPage.tsx` — 移除 :160-199 的 5 个 theme props 声明 + :884-888(SessionPanel)/:920-922(ChatPageV2)/:1210-1213(AppSettingsDialog) 的传递
4. `src/components/SessionPanel.tsx` — 移除 :42-59 的 5 个 props + :371-375 的传递
5. `src/components/HomeChatPage.tsx` — 移除 :24-32 的 4 个 props + :131-133(ChatPageV2)/:159-162(AppSettingsDialog) 的传递
6. `src/components/chat-page-v2.tsx` — 移除 `theme`/`isDark`/`onThemeChange` props(:69-97 接口, :109-111 解构)；移除向 `ChatShell` :608-610 的传递。**注意：** `ChatShell` 也接收这三个 props，需一并处理（核查发现 ChatShell 是 ChatPageV2 的直接子组件）

**验证：** `pnpm build` 通过；`grep -rn "themeMode\|onThemeModeChange\|onToggleTheme" src/components/` 无残留（除 store/hook/ThemeToggleButton）。

---

### Phase 5：文件/编辑器组件改用 store
1. `src/components/FileViewer.tsx` — 移除 :22 的 `isDark` prop，内部 `const isDark = useResolvedIsDark()`
2. `src/components/FileExplorer.tsx` — 移除 :43 的 `_isDark` prop（未使用，直接删）
3. `src/components/file-viewer/FileTabPane.tsx` — 移除 :229 的 `isDark` prop，内部 `useResolvedIsDark()`
4. `src/components/file-viewer/MonacoEditorPane.tsx` — 移除 :118 的 `isDark` prop，内部 `useResolvedIsDark()`；:236 `theme={isDark ? MONACO_THEME_DARK : MONACO_THEME_LIGHT}` 保持
5. `src/components/ProjectPage.tsx` — 移除向 FileViewer(:1018)/FileExplorer(:1145) 传递的 `isDark`

**验证：** Monaco/Shiki 主题切换后正确刷新；`pnpm build` 通过。

---

### Phase 6：AppSettingsDialog 改用 store
- `src/components/AppSettingsDialog.tsx` — 移除 :641-644 的 `isDark`/`themeMode`/`systemPrefersDark`/`onThemeModeChange` props
  - `ThemePanel`(:53-256) 内部改用 `useUIStore()` 读写 `themeMode` + `systemPrefersDark`
  - `AgentConfigPanel`(:386-603) 的 `isDark` 改用 `useResolvedIsDark()`（Shiki :470 + caret color :579）
- 移除 ProjectPage/HomeChatPage/SidebarFooterActions 向 AppSettingsDialog 传递的 theme props（Phase 4 中已处理大部分）

**验证：** 设置面板三选菜单与快捷按钮完全同步；system toggle 正常；预览卡片正确。

---

### Phase 7：清理死代码 + 旧 localStorage
- `src/App.css:1767-1791` — `:root[data-theme="light"]` 选择器迁移为 `:root:not(.dark)` 选择器（git-diff 覆盖样式）
- 确认 `onRehydrateStorage` 中 `localStorage.removeItem("jkcodingagent:theme")` 生效
- 移除 App.tsx 中 `getSystemPrefersDark`(:41-43) 若已被 store 替代

**验证：** `grep -rn "data-theme" src/` 无结果；`grep -rn "jkcodingagent:theme" src/` 仅在 onRehydrateStorage 中出现；Git diff 面板浅/深色下颜色正确。

---

### 最终验收
- `pnpm build` 成功（tsc 严格模式 + Vite）
- `pnpm lint` 通过（--max-warnings 0）
- 快捷按钮单击 dark↔light；Popover 三选含 system；system 模式跟随 OS
- 设置面板与快捷按钮状态同步
- Monaco/Shiki 切换后正确刷新
- 无 prop drilling 残留
- 单一持久化源（`jkcodingagent:ui`），旧 key 已清理

---

### 执行顺序
Phase 1 → 2 → 3 → 4 → 5 → 6 → 7。Phase 4（prop drilling 消除）是最大改动面，将拆分为多个小步骤逐个组件处理。每个 Phase 完成后验证 build。