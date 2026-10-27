import { create } from "zustand";

/**
 * 普通聊天「分类文件」内联区的打开目标。入口在每个已分类会话行，
 * sessionId 用于定位该会话的临时目录；布局由 ChatShell 接入。
 */
export interface ChatFilesTarget {
  categoryId: string;
  sessionId: string;
}

interface ChatFilesState {
  target: ChatFilesTarget | null;
  open: (categoryId: string, sessionId: string) => void;
  close: () => void;
}

export const useChatFilesStore = create<ChatFilesState>((set) => ({
  target: null,
  open: (categoryId, sessionId) => set({ target: { categoryId, sessionId } }),
  close: () => set({ target: null }),
}));
