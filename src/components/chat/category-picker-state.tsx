import { useState } from "react";
import { motion } from "framer-motion";
import { ArrowUpRight, Plus } from "lucide-react";
import type { ChatCategory } from "../../types";
import { cn } from "../../lib/cn";
import { hexWithAlpha } from "../../lib/hex-alpha";
import { resolveCategoryIcon } from "../../lib/category-icon";
import { Button } from "../ui/button";
import {
  ChatNewCategoryDialog,
  type ChatCategoryCreateConfig,
} from "../ChatNewCategoryDialog";

/**
 * 「选择分类开始新对话」空态（未选会话时取代消息区渲染）。
 *
 * 背景：新建会话不再默认落在某个隐式分类（旧 ensureSession 硬编码
 * "tech"）——分类决定会话的系统提示词、可用工具与子智能体，必须由
 * 用户显式选择。点击分类卡片即在对应分类下创建新会话；没有分类时
 * 可就地新建（与侧边栏「新建分类」共用同一个弹窗与回调）。
 */
export interface CategoryPickerStateProps {
  categories: ChatCategory[];
  loading?: boolean;
  onPickCategory: (categoryId: string) => void;
  onCreateCategory?: (name: string, config?: ChatCategoryCreateConfig) => void;
  className?: string;
}

export function CategoryPickerState({
  categories,
  loading = false,
  onPickCategory,
  onCreateCategory,
  className,
}: CategoryPickerStateProps) {
  const [dialogOpen, setDialogOpen] = useState(false);

  return (
    <div
      className={cn(
        "ai-category-picker flex min-h-0 flex-1 flex-col items-center justify-center px-6 pt-12 pb-[10vh]",
        className,
      )}
    >
      <motion.div
        initial={{ opacity: 0, scale: 0.96 }}
        animate={{ opacity: 1, scale: 1 }}
        transition={{ duration: 0.25, ease: [0.2, 0.8, 0.2, 1] }}
        className="flex flex-col items-center text-center"
      >
        <h2 className="text-xl font-semibold tracking-tight text-foreground">
          选择分类，开始新对话
        </h2>
        <p className="mt-2 max-w-md text-sm text-muted-foreground">
          分类决定这场对话的系统提示词、可用工具与子智能体——不同分类侧重的能力不同。
          选择后即在对应分类下创建新会话。
        </p>
      </motion.div>

      {loading ? (
        <div className="ai-category-picker-grid mt-9 w-full max-w-2xl">
          {Array.from({ length: 6 }).map((_, index) => (
            <div
              key={index}
              className="ai-category-picker-card is-loading"
              aria-hidden="true"
            />
          ))}
        </div>
      ) : categories.length === 0 ? (
        <div className="mt-9 flex flex-col items-center gap-3 text-sm text-muted-foreground">
          <p>还没有分类。先创建一个分类，再开始新对话。</p>
          {onCreateCategory && (
            <Button variant="outline" size="sm" onClick={() => setDialogOpen(true)}>
              <Plus size={14} strokeWidth={2} />
              新建分类
            </Button>
          )}
        </div>
      ) : (
        <motion.div
          initial="hidden"
          animate="show"
          variants={{
            hidden: {},
            show: { transition: { staggerChildren: 0.04, delayChildren: 0.1 } },
          }}
          className="ai-category-picker-grid mt-9 w-full max-w-2xl"
        >
          {categories.map((category) => (
            <CategoryCard
              key={category.id}
              category={category}
              onPick={() => onPickCategory(category.id)}
            />
          ))}
        </motion.div>
      )}

      {!loading && categories.length > 0 && onCreateCategory && (
        <button
          type="button"
          className="ai-category-picker-create mt-5 text-xs text-muted-foreground"
          onClick={() => setDialogOpen(true)}
        >
          <Plus size={12} strokeWidth={2} aria-hidden="true" />
          新建分类
        </button>
      )}

      {onCreateCategory && (
        <ChatNewCategoryDialog
          open={dialogOpen}
          initialName=""
          title="新建分类"
          confirmLabel="创建"
          showAgentConfig
          onSubmit={(name, config) => {
            onCreateCategory(name, config);
            setDialogOpen(false);
          }}
          onClose={() => setDialogOpen(false)}
        />
      )}
    </div>
  );
}

function CategoryCard({
  category,
  onPick,
}: {
  category: ChatCategory;
  onPick: () => void;
}) {
  const Icon = resolveCategoryIcon(category.icon);
  // 分类色是运行期数据，走内联 tint；非 hex 时回退 CSS 类中的中性令牌色。
  const tint = hexWithAlpha(category.color, "1f");
  const iconStyle = tint ? { color: category.color, background: tint } : undefined;
  return (
    <motion.button
      variants={{
        hidden: { opacity: 0, y: 8 },
        show: { opacity: 1, y: 0 },
      }}
      type="button"
      className="ai-category-picker-card group"
      title={`在「${category.name}」分类下开始新对话`}
      aria-label={`在「${category.name}」分类下开始新对话`}
      onClick={onPick}
    >
      <span className="ai-category-picker-card-icon" style={iconStyle}>
        <Icon size={16} strokeWidth={2} aria-hidden="true" />
      </span>
      <span className="min-w-0 flex-1 text-left">
        <span className="ai-category-picker-card-name">{category.name}</span>
        <span className="ai-category-picker-card-meta">
          {category.sessionCount > 0 ? `${category.sessionCount} 个会话` : "还没有会话"}
        </span>
      </span>
      <ArrowUpRight className="ai-prompt-card-arrow mt-0.5 h-4 w-4 shrink-0" />
    </motion.button>
  );
}
