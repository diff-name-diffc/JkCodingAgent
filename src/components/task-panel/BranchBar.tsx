import { useState, useMemo, useEffect, useCallback } from "react";
import { Search, Plus, ChevronDown, X, Tag, Check, GitFork, GitBranch } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import * as Popover from "@radix-ui/react-popover";
import { isImeComposing } from "../../utils";

interface GitBranchInfo {
  name: string;
  current: boolean;
  remote: string | null;
}

function BranchDialog({
  projectPath,
  branches,
  onClose,
  onCreated,
}: {
  projectPath: string;
  branches: GitBranchInfo[];
  onClose: () => void;
  onCreated: () => void;
}) {
  const currentBranch = branches.find((b) => b.current);
  const [branchName, setBranchName] = useState("");
  const [fromBranch, setFromBranch] = useState(currentBranch?.name ?? "");
  const [branchSearch, setBranchSearch] = useState("");
  const [popoverOpen, setPopoverOpen] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");

  const filteredBranches = useMemo(() => {
    const q = branchSearch.toLowerCase();
    return branches.filter((b) => !q || b.name.toLowerCase().includes(q));
  }, [branches, branchSearch]);

  const localBranches = filteredBranches.filter((b) => b.remote === null);
  const remoteGroups = filteredBranches
    .filter((b) => b.remote !== null)
    .reduce<Record<string, GitBranchInfo[]>>((acc, b) => {
      const key = b.remote!;
      if (!acc[key]) acc[key] = [];
      acc[key].push(b);
      return acc;
    }, {});

  const handleSelect = (name: string) => {
    setFromBranch(name);
    setPopoverOpen(false);
    setBranchSearch("");
  };

  const handleCreate = useCallback(async () => {
    const name = branchName.trim();
    if (!name) return;
    setLoading(true);
    setError("");
    try {
      await invoke("git_create_branch", {
        projectPath,
        branchName: name,
        fromBranch,
      });
      onCreated();
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [branchName, fromBranch, projectPath, onCreated]);

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (isImeComposing(e)) return;
    if (e.key === "Enter" && branchName.trim() && !loading) handleCreate();
    if (e.key === "Escape") onClose();
  };

  return (
    <div
      className="ai-dialog-overlay"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="ai-dialog ai-branch-dialog" onKeyDown={handleKeyDown}>
        <div className="ai-dialog-header">
          <div className="ai-dialog-title-block">
            <GitBranch size={16} strokeWidth={2} />
            <span className="ai-dialog-title">新建分支</span>
          </div>
          <button className="ai-dialog-close" onClick={onClose} type="button">
            <X size={15} />
          </button>
        </div>

        <div className="ai-field-stack">
          <label className="ai-field-label">
            <Tag size={12} strokeWidth={2} />
            分支名
          </label>
          <input
            className="ai-field ai-branch-input"
            placeholder="feature/my-branch"
            value={branchName}
            onChange={(e) => setBranchName(e.target.value)}
            autoFocus
          />
        </div>

        <div className="ai-field-stack">
          <label className="ai-field-label">
            <GitFork size={12} strokeWidth={2} />
            基于
          </label>
          <Popover.Root open={popoverOpen} onOpenChange={setPopoverOpen}>
            <Popover.Trigger asChild>
              <button className="ai-branch-select-trigger" type="button">
                <span>{fromBranch || "选择分支…"}</span>
                <ChevronDown size={13} strokeWidth={2} />
              </button>
            </Popover.Trigger>
            <Popover.Portal>
              <Popover.Content
                className="branch-popover-content ai-branch-popover"
                sideOffset={4}
                align="start"
                onOpenAutoFocus={(e) => e.preventDefault()}
              >
                {/* Search input */}
                <div className="branch-popover-search">
                  <Search size={13} strokeWidth={2} />
                  <input
                    className="branch-popover-search-input"
                    placeholder="搜索分支…"
                    value={branchSearch}
                    onChange={(e) => setBranchSearch(e.target.value)}
                    onKeyDown={(e) => e.stopPropagation()}
                    autoFocus
                  />
                  {branchSearch && (
                    <button
                      className="branch-popover-clear"
                      onClick={() => setBranchSearch("")}
                      type="button"
                    >
                      <X size={11} />
                    </button>
                  )}
                </div>
                <div className="branch-popover-list">
                  {localBranches.length > 0 && (
                    <>
                      <div className="branch-popover-group-label">本地</div>
                      {localBranches.map((b) => (
                        <button
                          key={b.name}
                          className="branch-popover-item"
                          onClick={() => handleSelect(b.name)}
                          type="button"
                        >
                          <GitBranch size={12} strokeWidth={2} />
                          <span className="branch-popover-item-name">
                            {b.name}
                            {b.current ? "（当前）" : ""}
                          </span>
                          {fromBranch === b.name && (
                            <Check size={12} strokeWidth={2.5} className="ai-branch-check" />
                          )}
                        </button>
                      ))}
                    </>
                  )}
                  {Object.entries(remoteGroups).map(([remote, bs]) => (
                    <div key={remote}>
                      <div className="branch-popover-separator" />
                      <div className="branch-popover-group-label">{remote}</div>
                      {bs.map((b) => (
                        <button
                          key={b.name}
                          className="branch-popover-item"
                          onClick={() => handleSelect(b.name)}
                          type="button"
                        >
                          <GitBranch size={12} strokeWidth={2} />
                          <span className="branch-popover-item-name">{b.name}</span>
                          {fromBranch === b.name && (
                            <Check size={12} strokeWidth={2.5} className="ai-branch-check" />
                          )}
                        </button>
                      ))}
                    </div>
                  ))}
                  {localBranches.length === 0 && Object.keys(remoteGroups).length === 0 && (
                    <div className="ai-branch-empty">没有找到分支</div>
                  )}
                </div>
              </Popover.Content>
            </Popover.Portal>
          </Popover.Root>
        </div>

        {error && <div className="ai-dialog-error">{error}</div>}

        <div className="ai-dialog-footer">
          <button className="ai-secondary-button" onClick={onClose} type="button">
            取消
          </button>
          <button
            className="ai-primary-button"
            disabled={!branchName.trim() || loading}
            onClick={handleCreate}
            type="button"
          >
            {loading ? "创建中…" : "创建"}
          </button>
        </div>
      </div>
    </div>
  );
}

export function BranchBar({ projectPath }: { projectPath: string }) {
  const [branches, setBranches] = useState<GitBranchInfo[]>([]);
  const [showDialog, setShowDialog] = useState(false);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [search, setSearch] = useState("");
  const [switching, setSwitching] = useState<string | null>(null);
  const [switchError, setSwitchError] = useState("");

  const fetchBranches = useCallback(async () => {
    try {
      const result = await invoke<GitBranchInfo[]>("git_list_branches", { projectPath });
      setBranches(result);
    } catch {
      // not a git repo or git not available
    }
  }, [projectPath]);

  useEffect(() => {
    fetchBranches();
  }, [fetchBranches]);

  // 检测外部分支切换：窗口获焦时刷新 + 10 秒轮询兜底
  useEffect(() => {
    const onFocus = () => fetchBranches();
    window.addEventListener("focus", onFocus);
    const timer = setInterval(fetchBranches, 10_000);
    return () => {
      window.removeEventListener("focus", onFocus);
      clearInterval(timer);
    };
  }, [fetchBranches]);

  const currentBranch = branches.find((b) => b.current);

  const filtered = useMemo(() => {
    const q = search.toLowerCase();
    return branches.filter((b) => !q || b.name.toLowerCase().includes(q));
  }, [branches, search]);

  const localBranches = filtered.filter((b) => b.remote === null);
  const remoteGroups = filtered
    .filter((b) => b.remote !== null)
    .reduce<Record<string, GitBranchInfo[]>>((acc, b) => {
      const key = b.remote!;
      if (!acc[key]) acc[key] = [];
      acc[key].push(b);
      return acc;
    }, {});

  if (branches.length === 0) return null;

  const handleSwitch = async (branch: GitBranchInfo) => {
    if (branch.current || switching) return;
    setSwitching(branch.name);
    setSwitchError("");
    try {
      await invoke("git_checkout_branch", {
        projectPath,
        branchName: branch.name,
        isRemote: branch.remote !== null,
      });
      await fetchBranches();
      setPickerOpen(false);
      setSearch("");
    } catch (e) {
      setSwitchError(String(e));
    } finally {
      setSwitching(null);
    }
  };

  return (
    <>
      <Popover.Root
        open={pickerOpen}
        onOpenChange={(open) => {
          setPickerOpen(open);
          if (!open) {
            setSearch("");
            setSwitchError("");
          }
        }}
      >
        <Popover.Trigger asChild>
          <button
            className={pickerOpen ? "ai-branch-bar is-open" : "ai-branch-bar"}
            title="切换分支"
            type="button"
          >
            <GitBranch size={12} strokeWidth={2} />
            <span className="ai-branch-bar-name">{currentBranch?.name ?? "游离 HEAD"}</span>
            <ChevronDown size={11} strokeWidth={2} className="ai-branch-bar-chevron" />
          </button>
        </Popover.Trigger>
        <Popover.Portal>
          <Popover.Content
            className="branch-popover-content ai-branch-popover"
            sideOffset={4}
            align="start"
            onOpenAutoFocus={(e) => e.preventDefault()}
          >
            {/* Search */}
            <div className="branch-popover-search">
              <Search size={13} strokeWidth={2} />
              <input
                className="branch-popover-search-input"
                placeholder="切换到分支…"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                onKeyDown={(e) => e.stopPropagation()}
                autoFocus
              />
              {search && (
                <button className="branch-popover-clear" onClick={() => setSearch("")} type="button">
                  <X size={11} />
                </button>
              )}
            </div>

            {/* Branch list */}
            <div className="branch-popover-list">
              {localBranches.length > 0 && (
                <>
                  <div className="branch-popover-group-label">本地</div>
                  {localBranches.map((b) => (
                    <button
                      key={b.name}
                      className={[
                        "branch-popover-item",
                        b.current ? "is-current" : "",
                        switching && switching !== b.name ? "is-muted" : "",
                      ]
                        .filter(Boolean)
                        .join(" ")}
                      onClick={() => handleSwitch(b)}
                      disabled={!!switching}
                      type="button"
                    >
                      <GitBranch size={12} strokeWidth={2} />
                      <span className="branch-popover-item-name">{b.name}</span>
                      {b.current && <Check size={12} strokeWidth={2.5} className="ai-branch-check" />}
                      {switching === b.name && <span className="ai-branch-switching">…</span>}
                    </button>
                  ))}
                </>
              )}
              {Object.entries(remoteGroups).map(([remote, bs]) => (
                <div key={remote}>
                  <div className="branch-popover-separator" />
                  <div className="branch-popover-group-label">{remote}</div>
                  {bs.map((b) => (
                    <button
                      key={b.name}
                      className={[
                        "branch-popover-item",
                        switching && switching !== b.name ? "is-muted" : "",
                      ]
                        .filter(Boolean)
                        .join(" ")}
                      onClick={() => handleSwitch(b)}
                      disabled={!!switching}
                      type="button"
                    >
                      <GitBranch size={12} strokeWidth={2} />
                      <span className="branch-popover-item-name">{b.name}</span>
                      {switching === b.name && <span className="ai-branch-switching">…</span>}
                    </button>
                  ))}
                </div>
              ))}
              {localBranches.length === 0 && Object.keys(remoteGroups).length === 0 && (
                <div className="ai-branch-empty">没有找到分支</div>
              )}
            </div>

            {switchError && <div className="ai-branch-popover-error">{switchError}</div>}

            {/* Footer: new branch */}
            <div className="branch-popover-separator" />
            <button
              className="branch-popover-item ai-branch-create-option"
              onClick={() => {
                setPickerOpen(false);
                setSearch("");
                setShowDialog(true);
              }}
              type="button"
            >
              <Plus size={12} strokeWidth={2.5} />
              <span>新建分支…</span>
            </button>
          </Popover.Content>
        </Popover.Portal>
      </Popover.Root>

      {showDialog && (
        <BranchDialog
          projectPath={projectPath}
          branches={branches}
          onClose={() => setShowDialog(false)}
          onCreated={() => {
            fetchBranches();
            setShowDialog(false);
          }}
        />
      )}
    </>
  );
}
