import { useEffect, useRef, useState } from "react";
import { X } from "lucide-react";

interface ChatNewCategoryDialogProps {
  open: boolean;
  initialName: string;
  onSubmit: (name: string) => void;
  onClose: () => void;
  title: string;
  confirmLabel: string;
}

export function ChatNewCategoryDialog({
  open,
  initialName,
  onSubmit,
  onClose,
  title,
  confirmLabel,
}: ChatNewCategoryDialogProps) {
  const [name, setName] = useState(initialName);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (open) {
      setName(initialName);
      setTimeout(() => inputRef.current?.focus(), 50);
    }
  }, [open, initialName]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open) return null;

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = name.trim();
    if (trimmed) onSubmit(trimmed);
  };

  return (
    <div
      style={{
        position: "fixed",
        inset: 0,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        background: "rgba(0,0,0,0.45)",
        zIndex: 100,
        animation: "fadeIn 0.12s ease-out",
      }}
      onClick={onClose}
    >
      <form
        onSubmit={handleSubmit}
        onClick={(e) => e.stopPropagation()}
        style={{
          width: 340,
          padding: "20px 22px 18px",
          background: "var(--bg-card)",
          border: "1px solid var(--border-medium)",
          borderRadius: 12,
          boxShadow: "0 12px 40px rgba(0,0,0,0.3)",
          position: "relative",
        }}
      >
        <button
          type="button"
          onClick={onClose}
          style={{
            position: "absolute",
            top: 10,
            right: 10,
            background: "none",
            border: "none",
            cursor: "pointer",
            padding: 2,
            color: "var(--text-muted)",
          }}
        >
          <X size={14} />
        </button>
        <div style={{ fontSize: 14, fontWeight: 650, marginBottom: 14, color: "var(--text-primary)" }}>
          {title}
        </div>
        <input
          ref={inputRef}
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="输入分类名称"
          style={{
            width: "100%",
            padding: "9px 12px",
            border: "1px solid var(--border-medium)",
            borderRadius: 8,
            background: "var(--bg-input)",
            color: "var(--text-primary)",
            fontSize: 13,
            outline: "none",
            boxSizing: "border-box",
            marginBottom: 16,
          }}
        />
        <div style={{ display: "flex", justifyContent: "flex-end", gap: 8 }}>
          <button
            type="button"
            onClick={onClose}
            style={{
              padding: "7px 14px",
              background: "var(--bg-subtle)",
              border: "1px solid var(--border-medium)",
              borderRadius: 7,
              fontSize: 12.5,
              color: "var(--text-secondary)",
              cursor: "pointer",
            }}
          >
            取消
          </button>
          <button
            type="submit"
            disabled={!name.trim()}
            style={{
              padding: "7px 16px",
              background: "var(--accent)",
              border: "none",
              borderRadius: 7,
              fontSize: 12.5,
              color: "white",
              fontWeight: 600,
              cursor: name.trim() ? "pointer" : "not-allowed",
              opacity: name.trim() ? 1 : 0.5,
            }}
          >
            {confirmLabel}
          </button>
        </div>
      </form>
    </div>
  );
}
