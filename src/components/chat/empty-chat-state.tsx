import { motion, useReducedMotion } from "framer-motion";
import { ArrowUpRight } from "lucide-react";
import { cn } from "../../lib/cn";
import { PLAIN_CHAT_EMPTY_STATE } from "./chat-empty-content";
import { ChatWelcomeHeading } from "./chat-welcome-heading";

/**
 * Empty state for a chat surface with no messages yet.
 *
 * Shows a short welcome + a grid of starter prompts. The prompts are passed
 * in by the parent (so they can be localized / model-aware); clicking one
 * calls `onPickPrompt`, which the parent feeds into the prompt input.
 *
 * 缺省文案 = 普通聊天领域空态（UI-25 A06）；项目 / 架构入口各自传入贴合语境的
 * title/copy/prompts（见 chat-empty-content 与 ArchitectureChatPanel）。
 */
export interface EmptyChatStateProps {
  onPickPrompt: (prompt: string) => void;
  prompts?: string[];
  /** 领域化标题/副文案（UI-15 A06）：缺省为普通聊天欢迎语。 */
  title?: string;
  copy?: string;
  className?: string;
}

const DEFAULT_PROMPTS = PLAIN_CHAT_EMPTY_STATE.prompts;
const DEFAULT_TITLE = PLAIN_CHAT_EMPTY_STATE.title;
const DEFAULT_COPY = PLAIN_CHAT_EMPTY_STATE.copy;

export function EmptyChatState({
  onPickPrompt,
  prompts = DEFAULT_PROMPTS,
  title = DEFAULT_TITLE,
  copy = DEFAULT_COPY,
  className,
}: EmptyChatStateProps) {
  const reducedMotion = useReducedMotion();
  return (
    <div
      className={cn(
        "ai-empty-state flex h-full flex-col items-center justify-center px-4 py-8",
        className,
      )}
    >
      <motion.div
        initial={reducedMotion ? false : { opacity: 0, scale: 0.98 }}
        animate={{ opacity: 1, scale: 1 }}
        transition={{ duration: 0.25, ease: [0.2, 0.8, 0.2, 1] }}
      >
        <ChatWelcomeHeading title={title} copy={copy} />
      </motion.div>

      <motion.div
        initial={reducedMotion ? false : "hidden"}
        animate="show"
        variants={{
          hidden: {},
          show: { transition: { staggerChildren: 0.04, delayChildren: 0.1 } },
        }}
        className="ai-chat-starters mt-8 grid w-full max-w-xl grid-cols-2 gap-3"
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
