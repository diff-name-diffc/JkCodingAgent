import { motion } from "framer-motion";
import { ArrowUpRight } from "lucide-react";
import { cn } from "../../lib/cn";
import appLogo from "../../assets/app-logo.png";

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
    <div
      className={cn(
        "ai-empty-state flex h-full flex-col items-center justify-center px-6 pt-12 pb-[10vh]",
        className,
      )}
    >
      <motion.div
        initial={{ opacity: 0, scale: 0.96 }}
        animate={{ opacity: 1, scale: 1 }}
        transition={{ duration: 0.25, ease: [0.2, 0.8, 0.2, 1] }}
        className="ai-empty-core flex flex-col items-center text-center"
      >
        <img src={appLogo} alt="Aha" className="ai-empty-logo mb-5 h-14 w-14 rounded-2xl" />
        <h2 className="ai-empty-title text-xl font-semibold tracking-tight text-foreground">
          有什么可以帮你的？
        </h2>
        <p className="ai-empty-copy mt-2 max-w-sm text-sm text-muted-foreground">
          输入任务、粘贴代码、拆解方案 —— 模型会在同一个工作台里推理、执行与回放。
        </p>
      </motion.div>

      <motion.div
        initial="hidden"
        animate="show"
        variants={{
          hidden: {},
          show: { transition: { staggerChildren: 0.04, delayChildren: 0.1 } },
        }}
        className="ai-prompt-grid mt-9 grid w-full max-w-2xl grid-cols-1 gap-2.5 sm:grid-cols-2"
      >
        {prompts.map((prompt) => (
          <motion.button
            key={prompt}
            variants={{
              hidden: { opacity: 0, y: 8 },
              show: { opacity: 1, y: 0 },
            }}
            onClick={() => onPickPrompt(prompt)}
            className="ai-prompt-card group flex items-start gap-2.5 text-left text-sm"
          >
            <span className="min-w-0 flex-1">{prompt}</span>
            <ArrowUpRight className="ai-prompt-card-arrow mt-0.5 h-4 w-4 shrink-0" />
          </motion.button>
        ))}
      </motion.div>
    </div>
  );
}
