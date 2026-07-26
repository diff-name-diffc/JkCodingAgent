import { Bot, UserRound } from "lucide-react";
import assistantAvatar from "../../assets/dispatcher-assistant-avatar.png";
import userAvatar from "../../assets/dispatcher-user-avatar.png";
import { cn } from "../../lib/cn";
import { Avatar, AvatarFallback, AvatarImage } from "../ui/avatar";

interface ChatAvatarProps {
  role: "assistant" | "user";
  active?: boolean;
  hidden?: boolean;
  className?: string;
}

const AVATARS = {
  assistant: {
    src: assistantAvatar,
    alt: "Aha AI",
    fallback: Bot,
  },
  user: {
    src: userAvatar,
    alt: "你",
    fallback: UserRound,
  },
} as const;

export function ChatAvatar({
  role,
  active = false,
  hidden = false,
  className,
}: ChatAvatarProps) {
  const avatar = AVATARS[role];
  const FallbackIcon = avatar.fallback;

  return (
    <Avatar
      className={cn(
        "ai-chat-avatar",
        `is-${role}`,
        active && "is-active",
        hidden && "invisible",
        className,
      )}
    >
      <AvatarImage src={avatar.src} alt={avatar.alt} />
      <AvatarFallback>
        <FallbackIcon className="h-3.5 w-3.5" />
      </AvatarFallback>
    </Avatar>
  );
}
