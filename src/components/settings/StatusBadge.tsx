export type ProviderTestStatus = "untested" | "ok" | "failed";

const STATUS_LABEL: Record<ProviderTestStatus, string> = {
  untested: "未测试",
  ok: "可用",
  failed: "失败",
};

/** 状态徽标：未测试(灰) / 可用(绿) / 失败(红)，999px 圆角。 */
export function StatusBadge({ status }: { status: ProviderTestStatus }) {
  return (
    <span className={`ai-set-badge is-${status}`}>
      <span className="ai-set-badge-dot" />
      {STATUS_LABEL[status]}
    </span>
  );
}
