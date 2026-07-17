import type { ReactNode } from "react";

export function IconButton({
  icon,
  title,
  active = false,
  disabled = false,
  onClick,
  size = 32,
}: {
  icon: ReactNode;
  title?: string;
  active?: boolean;
  disabled?: boolean;
  onClick?: () => void;
  size?: number;
}) {
  return (
    <button
      type="button"
      title={title}
      disabled={disabled}
      onClick={onClick}
      className={`ai-icon-button${active ? " is-active" : ""}${disabled ? " is-disabled" : ""}`}
      style={{ width: size, height: size }}
    >
      {icon}
    </button>
  );
}
