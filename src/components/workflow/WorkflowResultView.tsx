import { useMemo, useState } from "react";
import { ClipboardList, FileSearch } from "lucide-react";
import type { LucideIcon } from "lucide-react";
import type { WorkflowPlanRecord } from "../../types";
import { cn } from "../../lib/cn";
import { Button } from "../ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "../ui/select";
import { StatusPill } from "../detail/StatusPill";
import { MarkdownRenderer } from "../markdown/MarkdownRenderer";
import { FileGlyph } from "../../file-icons";
import { formatRelativeTime } from "../../utils";
import { resultKindMeta } from "./workflow-utils";
import { CopyIconButton } from "./CopyIconButton";

/** 后端对工作区外路径的审计前缀（mapping.rs normalize_locations）：这些文件
 * 无法经工作区路径校验打开，清单中只读展示。 */
const OUTSIDE_WORKSPACE_PREFIX = "[工作区外] ";

export interface WorkflowResultViewProps {
  plan: WorkflowPlanRecord | null;
  /** 跳到画布并打开指定节点抽屉（无结论时引导查看节点输出）。 */
  onOpenNode: (nodeId: string) => void;
  /** 打开工作区文件（修改文件清单点击 → 主区文件标签）。 */
  onOpenFile: (path: string, name: string) => void;
}

/** 结果视图空态：图标 + 主文案 + 弱化说明（区别于设置域 EmptyState）。 */
function ResultBlank({ icon: Icon, title, hint }: { icon: LucideIcon; title: string; hint?: string }) {
  return (
    <div className="ai-workflow-result-blank">
      <Icon className="h-5 w-5" strokeWidth={1.5} aria-hidden />
      <p className="ai-workflow-result-blank-title">{title}</p>
      {hint && <p className="ai-workflow-result-blank-hint">{hint}</p>}
    </div>
  );
}

/**
 * 执行结果视图（工作流详情态与画布平级的一级视图）：渲染 run 收尾组装的
 * 结构化执行结果——验收结论（徽标 + 理由正文）、结论文本 markdown、
 * 修改文件清单。多 attempt 经 Select 切换，默认跟随最近一次运行。
 * 取消/中断路径的运行无结果记录，给出降级提示；
 * 收尾时无成功节点的运行落 resultKind="none"，呈现「执行完成、无结果」。
 */
export function WorkflowResultView({ plan, onOpenNode, onOpenFile }: WorkflowResultViewProps) {
  // useMemo 稳定引用：runs 参与 run 的派生依赖，`plan?.runs ?? []` 每次渲染
  // 新建空数组会让下游 useMemo 失效（react-hooks/exhaustive-deps 守护）。
  const runs = useMemo(() => plan?.runs ?? [], [plan]);
  // null = 跟随最近一次运行（runs 按 attemptNo 倒序，首位即最新）。
  const [selectedRunId, setSelectedRunId] = useState<string | null>(null);
  const run = useMemo(
    () => runs.find((candidate) => candidate.id === selectedRunId) ?? runs[0] ?? null,
    [runs, selectedRunId],
  );

  if (!plan) {
    return (
      <div className="ai-workflow-result">
        <ResultBlank icon={ClipboardList} title="计划加载中…" />
      </div>
    );
  }
  if (runs.length === 0) {
    return (
      <div className="ai-workflow-result">
        <ResultBlank
          icon={ClipboardList}
          title="尚未运行"
          hint="工作流还没有运行记录，启动后这里会展示执行结果。"
        />
      </div>
    );
  }
  if (!run) return null;

  const result = run.result ?? null;
  const kind = resultKindMeta(result?.resultKind ?? "unknown");
  const running = run.status === "running";
  const conclusionNodeId = result?.conclusionNodeId ?? null;

  return (
    <div className="ai-workflow-result" role="region" aria-label="执行结果">
      <div className="ai-workflow-result-meta">
        <span className="ai-workflow-result-chips">
          <span className={cn("ai-workflow-chip", kind.className)}>{kind.label}</span>
          <StatusPill domain="verdict" status={run.verdictStatus} />
        </span>
        <span className="ai-workflow-result-meta-text">
          第 {run.attemptNo} 次运行
          {run.finishedAt
            ? ` · ${formatRelativeTime(new Date(run.finishedAt).toISOString())}结束`
            : ""}
        </span>
        {runs.length > 1 && (
          <Select value={run.id} onValueChange={setSelectedRunId}>
            <SelectTrigger className="h-7 w-40 text-xs" aria-label="选择运行次数">
              <SelectValue placeholder="选择运行" />
            </SelectTrigger>
            <SelectContent>
              {runs.map((candidate) => (
                <SelectItem key={candidate.id} value={candidate.id}>
                  第 {candidate.attemptNo} 次 · {candidate.mode === "resume" ? "续跑" : "完整"} ·{" "}
                  {candidate.status === "running"
                    ? "运行中"
                    : candidate.status === "completed"
                      ? "已完成"
                      : candidate.status === "failed"
                        ? "有失败"
                        : "已取消"}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        )}
      </div>

      {run.verdictReason && (
        <p className="ai-workflow-result-verdict" data-status={run.verdictStatus}>
          {run.verdictReason}
        </p>
      )}

      {running ? (
        <ResultBlank
          icon={ClipboardList}
          title="运行中…"
          hint="执行完成后这里会生成本次运行的结论文本与修改文件清单。"
        />
      ) : !result ? (
        <ResultBlank
          icon={ClipboardList}
          title="本次运行没有结构化结果"
          hint="历史运行（结果记录上线前）与被取消的运行不产出执行结果；可在画布中点开节点查看输出。"
        />
      ) : result.resultKind === "none" ? (
        <ResultBlank
          icon={ClipboardList}
          title="执行完成、无结果"
          hint="本次运行没有任何成功的节点，未产出结论文本与修改文件；可在画布中点开节点查看输出。"
        />
      ) : (
        <>
          <section className="ai-workflow-result-section" aria-label="结论文本">
            <div className="ai-workflow-result-section-label">
              {result.resultKind === "edit" ? "执行结论" : "审查结论"}
              {result.conclusionMd && result.conclusionNodeId && (
                <span className="ai-workflow-result-hint">来自汇总节点 {result.conclusionNodeId}</span>
              )}
              <CopyIconButton
                value={result.conclusionMd}
                label="复制结论文本（markdown 原文）"
                className="ml-auto self-center"
              />
            </div>
            {result.conclusionMd ? (
              <MarkdownRenderer content={result.conclusionMd} variant="document" />
            ) : (
              <div className="ai-workflow-result-empty">
                <span>汇总节点未成功产出结论文本。</span>
                {conclusionNodeId && (
                  <Button variant="outline" size="sm" onClick={() => onOpenNode(conclusionNodeId)}>
                    <FileSearch className="h-3.5 w-3.5" />
                    查看节点输出
                  </Button>
                )}
              </div>
            )}
          </section>

          <section className="ai-workflow-result-section" aria-label="修改文件清单">
            <div className="ai-workflow-result-section-label">
              修改文件
              <span className="ai-workflow-result-hint">
                {result.modifiedFiles.length > 0
                  ? `${result.modifiedFiles.length} 个 · 清单为工具调用自动采集，完整清单以结论为准`
                  : "无"}
              </span>
            </div>
            {result.modifiedFiles.length > 0 && (
              <ul className="ai-workflow-result-files">
                {result.modifiedFiles.map((file) => {
                  const outside = file.startsWith(OUTSIDE_WORKSPACE_PREFIX);
                  const path = outside ? file.slice(OUTSIDE_WORKSPACE_PREFIX.length) : file;
                  const name = path.split("/").pop() ?? path;
                  return (
                    <li key={file}>
                      <button
                        type="button"
                        className="ai-workflow-result-file"
                        disabled={outside}
                        title={outside ? `工作区外文件（仅审计记录）：${path}` : path}
                        onClick={() => onOpenFile(path, name)}
                      >
                        <FileGlyph path={path} size={18} />
                        <span className="ai-workflow-result-file-name">{name}</span>
                        <span className="ai-workflow-result-file-dir">
                          {outside
                            ? "（工作区外，仅审计）"
                            : path.slice(0, Math.max(0, path.length - name.length))}
                        </span>
                      </button>
                    </li>
                  );
                })}
              </ul>
            )}
          </section>
        </>
      )}
    </div>
  );
}
