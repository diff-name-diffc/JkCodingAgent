import type { ComponentProps, CSSProperties, ReactNode } from "react";
import { Button, Card, IconButton, Text, Tooltip } from "@radix-ui/themes";

function cx(...parts: Array<string | false | null | undefined>) {
  return parts.filter(Boolean).join(" ");
}

export function BrandButton({ className, children, ...props }: ComponentProps<typeof Button>) {
  return (
    <Button
      {...props}
      className={cx("dispatcher-composer-button", className)}
      style={{
        fontWeight: 700,
        letterSpacing: 0,
        ...props.style,
      }}
    >
      {children}
    </Button>
  );
}

export function IconAction({
  label,
  className,
  children,
  ...props
}: ComponentProps<typeof IconButton> & { label: string }) {
  return (
    <Tooltip content={label}>
      <IconButton
        {...props}
        aria-label={props["aria-label"] ?? label}
        className={cx("dispatcher-composer-button", className)}
      >
        {children}
      </IconButton>
    </Tooltip>
  );
}

export function StatusPill({
  tone = "neutral",
  children,
  style,
}: {
  tone?: "neutral" | "accent" | "warning" | "danger" | "success";
  children: ReactNode;
  style?: CSSProperties;
}) {
  const color =
    tone === "accent"
      ? "var(--accent)"
      : tone === "warning"
        ? "var(--warning)"
        : tone === "danger"
          ? "var(--danger)"
          : tone === "success"
            ? "var(--success)"
            : "var(--text-muted)";

  return (
    <Text
      as="span"
      size="1"
      weight="bold"
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 6,
        minHeight: 22,
        padding: "0 8px",
        borderRadius: 999,
        border: "1px solid color-mix(in srgb, currentColor 24%, transparent)",
        background: "color-mix(in srgb, currentColor 10%, transparent)",
        color,
        whiteSpace: "nowrap",
        ...style,
      }}
    >
      {children}
    </Text>
  );
}

export function SurfacePanel({ className, children, ...props }: ComponentProps<typeof Card>) {
  return (
    <Card {...props} className={cx("nezha-brand-surface", className)}>
      {children}
    </Card>
  );
}

export function CommandComposer({
  className,
  children,
  style,
  ...props
}: ComponentProps<"div"> & {
  className?: string;
  children: ReactNode;
  style?: CSSProperties;
}) {
  return (
    <div
      {...props}
      className={cx("dispatcher-composer-shell", "nezha-brand-surface", className)}
      style={style}
    >
      {children}
    </div>
  );
}
