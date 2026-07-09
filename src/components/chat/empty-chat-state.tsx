import { motion } from "framer-motion";
import { MessageSquare, Sparkles } from "lucide-react";
import { cn } from "../../lib/cn";
import { Button } from "../ui/button";

/**
 * Empty state for a chat surface with no messages yet.
 *
 * Shows a short welcome + a grid of starter prompts. The prompts are passed
 * in by the parent (so they can be localized / model-aware); clicking one
 * calls `onPickPrompt`, which the parent feeds into the prompt input.
 */
export interface EmptyChatStateProps {
  onPickPrompt: (prompt: string) => void;
  prompts?: string[];
  className?: string;
}

const DEFAULT_PROMPTS = [
  "帮我写一个 Python 脚本，批量重命名当前目录下的图片",
  "解释一下 React 19 的 use() hook 和 Suspense 的关系",
  "把这段 SQL 优化一下，并解释为什么更快",
  "给我一个 Tauri + React 项目的目录结构建议",
];

export function EmptyChatState({
  onPickPrompt,
  prompts = DEFAULT_PROMPTS,
  className,
}: EmptyChatStateProps) {
  return (
    <div className={cn("ai-empty-state flex h-full flex-col items-center justify-center px-6 py-12", className)}>
      <motion.div
        initial={{ opacity: 0, scale: 0.96 }}
        animate={{ opacity: 1, scale: 1 }}
        transition={{ duration: 0.25, ease: [0.2, 0.8, 0.2, 1] }}
        className="ai-empty-core flex flex-col items-center text-center"
      >
        <div className="ai-orb mb-4 flex h-14 w-14 items-center justify-center rounded-2xl bg-primary/12 text-primary">
          <Sparkles className="h-7 w-7" />
        </div>
        <h2 className="ai-empty-title text-xl font-semibold tracking-tight text-foreground">
          启动智能协作舱
        </h2>
        <p className="ai-empty-copy mt-1.5 max-w-sm text-sm text-muted-foreground">
          输入任务、粘贴代码、拆解方案；让模型在同一个控制台里推理、执行与回放。
        </p>
      </motion.div>

      <motion.div
        initial="hidden"
        animate="show"
        variants={{
          hidden: {},
          show: { transition: { staggerChildren: 0.04, delayChildren: 0.1 } },
        }}
        className="ai-prompt-grid mt-8 grid w-full max-w-2xl grid-cols-1 gap-2 sm:grid-cols-2"
      >
        {prompts.map((prompt) => (
          <motion.button
            key={prompt}
            variants={{
              hidden: { opacity: 0, y: 8 },
              show: { opacity: 1, y: 0 },
            }}
            onClick={() => onPickPrompt(prompt)}
            className="ai-prompt-card group flex items-start gap-3 rounded-lg border border-border bg-card p-3 text-left text-sm text-foreground transition-colors hover:border-primary/40 hover:bg-secondary/60"
          >
            <MessageSquare className="mt-0.5 h-4 w-4 shrink-0 text-muted-foreground group-hover:text-primary" />
            <span className="min-w-0 flex-1">{prompt}</span>
          </motion.button>
        ))}
      </motion.div>

      <div className="ai-empty-hint mt-8 flex items-center gap-2 text-xs text-muted-foreground">
        <Button variant="ghost" size="sm" className="pointer-events-none text-xs">
          Enter 发送 · Shift+Enter 换行
        </Button>
      </div>
    </div>
  );
}
