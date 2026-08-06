import { Section } from "./Section";
import { useAhaSettings } from "./use-aha-settings";

/**
 * 「执行图」设置页：执行图编排的运行期行为开关。
 * 数据存于 `AhaSettingsV2.graph`，走 use-aha-settings 自动保存管线。
 */
export function GraphPage() {
  const { settings, updateSettings } = useAhaSettings();
  const graph = settings?.graph ?? { pauseBeforeWrite: false };

  return (
    <div className="ai-set-page">
      <Section
        title="高危写操作检查点"
        description="执行图中第一个修改类（coding）节点启动前暂停运行，等你在图面板确认后继续。适合在执行文件修改/命令前人工把关；关闭后执行图将一口气跑到结束。"
      >
        <div className="flex items-center gap-1.5">
          <button
            type="button"
            role="switch"
            aria-checked={graph.pauseBeforeWrite}
            aria-label="写操作前暂停确认"
            className={graph.pauseBeforeWrite ? "ai-set-switch is-on" : "ai-set-switch"}
            onClick={() =>
              updateSettings((prev) => ({
                ...prev,
                graph: {
                  ...(prev.graph ?? { pauseBeforeWrite: false }),
                  pauseBeforeWrite: !(prev.graph?.pauseBeforeWrite ?? false),
                },
              }))
            }
          >
            <span className="ai-set-switch-thumb" />
          </button>
          <span className="ai-aha-hint">
            {graph.pauseBeforeWrite ? "已开启：写操作前暂停" : "已关闭：直接执行"}
          </span>
        </div>
      </Section>
    </div>
  );
}
