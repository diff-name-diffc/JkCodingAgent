import { useState, useEffect, useCallback } from "react";
import { Bell, X, ExternalLink, Check, CheckCheck, Info, AlertTriangle, AlertCircle } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { NotificationItem, NotificationResult } from "../types";

function LevelIcon({ level }: { level: string }) {
  switch (level) {
    case "warning":
      return <AlertTriangle size={14} strokeWidth={2} color="var(--warning)" />;
    case "error":
      return <AlertCircle size={14} strokeWidth={2} color="var(--danger, #ef4444)" />;
    default:
      return <Info size={14} strokeWidth={2} color="var(--accent)" />;
  }
}

function NotificationEntry({
  item,
  onMarkRead,
}: {
  item: NotificationItem;
  onMarkRead: (id: string) => void;
}) {
  const handleClick = async () => {
    if (!item.isRead) onMarkRead(item.id);
    if (item.url) {
      await openUrl(item.url);
    }
  };

  return (
    <div
      onClick={handleClick}
      className={[
        "ai-notification-entry",
        item.url ? "is-clickable" : "",
        item.isRead ? "is-read" : "is-unread",
      ].filter(Boolean).join(" ")}
    >
      <div className="ai-notification-entry-icon">
        <LevelIcon level={item.level} />
      </div>
      <div className="ai-notification-entry-main">
        <div className="ai-notification-entry-head">
          <span
            className={item.isRead ? "ai-notification-entry-title" : "ai-notification-entry-title is-unread"}
          >
            {item.title}
          </span>
          {item.url && (
            <ExternalLink
              size={11}
              strokeWidth={2}
              color="var(--text-hint)"
              className="ai-notification-entry-link"
            />
          )}
        </div>
        <div className="ai-notification-entry-body">
          {item.body}
        </div>
        <div className="ai-notification-entry-time">
          {item.createdAt}
        </div>
      </div>
      {!item.isRead && (
        <button
          title="标记为已读"
          onClick={(e) => {
            e.stopPropagation();
            onMarkRead(item.id);
          }}
          className="ai-notification-mark-read"
        >
          <Check size={12} strokeWidth={2.5} />
        </button>
      )}
    </div>
  );
}

export function NotificationBell() {
  const [open, setOpen] = useState(false);
  const [result, setResult] = useState<NotificationResult | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const fetchNotifications = useCallback(async () => {
    setLoading(true);
    try {
      const data = await invoke<NotificationResult>("get_notifications");
      setResult(data);
      setError(null);
    } catch (err) {
      const message =
        typeof err === "string"
          ? err
          : err instanceof Error
            ? err.message
            : "加载通知失败";
      setError(message);
      console.error("加载通知失败:", err);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchNotifications();
  }, [fetchNotifications]);

  const handleMarkRead = useCallback(
    async (id: string) => {
      try {
        await invoke("mark_notification_read", { id });
        setResult((prev) => {
          if (!prev) return prev;
          const notifications = prev.notifications.map((n) =>
            n.id === id ? { ...n, isRead: true } : n,
          );
          const unreadCount = notifications.filter((n) => !n.isRead).length;
          return { notifications, unreadCount };
        });
      } catch {
        // Silent
      }
    },
    [],
  );

  const handleMarkAllRead = useCallback(async () => {
    try {
      await invoke("mark_all_notifications_read");
      setResult((prev) => {
        if (!prev) return prev;
        const notifications = prev.notifications.map((n) => ({ ...n, isRead: true }));
        return { notifications, unreadCount: 0 };
      });
    } catch {
      // Silent
    }
  }, []);

  const unreadCount = result?.unreadCount ?? 0;
  const isActive = unreadCount > 0 || loading || Boolean(error);
  const bellColor = error
    ? "var(--danger, #ef4444)"
    : unreadCount > 0
      ? "var(--accent)"
      : "var(--text-hint)";

  function handleOverlayClick(e: React.MouseEvent<HTMLDivElement>) {
    if (e.target === e.currentTarget) {
      setOpen(false);
    }
  }

  return (
    <>
      <button
        className={isActive ? "ai-sidebar-tool-button is-active" : "ai-sidebar-tool-button"}
        title="通知"
        onClick={() => setOpen((v) => !v)}
      >
        <Bell size={14} strokeWidth={1.6} color={bellColor} />
        {unreadCount > 0 && (
          <span className="ai-notification-count">
            {unreadCount > 99 ? "99+" : unreadCount}
          </span>
        )}
      </button>

      {open && (
        <div
          className="ai-notification-overlay"
          onClick={handleOverlayClick}
        >
          <div className="ai-notification-dialog">
            <div className="ai-notification-header">
              <span className="ai-notification-title">
                通知
                {unreadCount > 0 && (
                  <span className="ai-notification-title-meta">
                    （{unreadCount} 条未读）
                  </span>
                )}
              </span>
              {unreadCount > 0 && (
                <button
                  title="全部标为已读"
                  onClick={handleMarkAllRead}
                  className="ai-notification-header-button"
                >
                  <CheckCheck size={14} strokeWidth={2} />
                </button>
              )}
              <button
                title="关闭"
                onClick={() => setOpen(false)}
                className="ai-notification-header-button"
              >
                <X size={16} strokeWidth={2} />
              </button>
            </div>

            <div className="ai-notification-list chat-scroll">
              {loading && !result ? (
                <div className="ai-notification-empty">
                  加载中...
                </div>
              ) : error && !result ? (
                <div className="ai-notification-empty is-error">
                  {error}
                </div>
              ) : !result || result.notifications.length === 0 ? (
                <div className="ai-notification-empty">
                  暂无通知
                </div>
              ) : (
                result.notifications.map((item) => (
                  <NotificationEntry key={item.id} item={item} onMarkRead={handleMarkRead} />
                ))
              )}
            </div>
          </div>
        </div>
      )}
    </>
  );
}
