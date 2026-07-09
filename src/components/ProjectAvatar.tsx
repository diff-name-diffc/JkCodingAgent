import type React from "react";
import { getAvatarGradient } from "../utils";
import { cn } from "../lib/cn";

export function ProjectAvatar({
  name,
  size = 28,
  className,
  style: extraStyle,
}: {
  name: string;
  size?: number;
  className?: string;
  style?: React.CSSProperties;
}) {
  const [from, to] = getAvatarGradient(name);
  const initials =
    name.length >= 2
      ? (name[0] + (name.match(/[-_\s]([a-zA-Z])/)?.[1] ?? name[1])).toUpperCase()
      : name.slice(0, 2).toUpperCase();
  return (
    <div
      className={cn("ai-project-avatar", className)}
      style={{
        width: size,
        height: size,
        borderRadius: Math.round(size * 0.28),
        fontSize: size * 0.38,
        "--project-avatar-from": from,
        "--project-avatar-to": to,
        ...extraStyle,
      } as React.CSSProperties}
    >
      {initials}
    </div>
  );
}
