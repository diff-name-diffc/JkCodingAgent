import { create } from "zustand";

/**
 * 「分类文件」面板（普通聊天侧）的开合状态：入口在侧栏分类会话列表的
 * 文件夹行，弹层挂在 HomeChatPage。走 store 而非 props 链，避免穿透
 * Sidebar / ChatShell / ChatPageV2 等两侧共用组件（项目聊天无此功能）。
 */
interface ChatFilesState {
  openCategoryId: string | null;
  open: (categoryId: string) => void;
  close: () => void;
}

export const useChatFilesStore = create<ChatFilesState>((set) => ({
  openCategoryId: null,
  open: (categoryId) => set({ openCategoryId: categoryId }),
  close: () => set({ openCategoryId: null }),
}));
