import { useEffect, useState } from "react";
import type React from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirm, open } from "@tauri-apps/plugin-dialog";
import { BookOpen, Check, Database, Pencil, Plus, Trash2, Upload, X } from "lucide-react";
import type { KnowledgeCollection, KnowledgeSettings } from "../../types";
import { KnowledgeWorkbench } from "./KnowledgeWorkbench";
import { defaultKnowledgeSettings } from "./KnowledgeSettingsPanel";
import s from "../../styles";

export function KnowledgePage() {
  const [collections, setCollections] = useState<KnowledgeCollection[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [settings, setSettings] = useState<KnowledgeSettings>(defaultKnowledgeSettings());
  const [loading, setLoading] = useState(true);
  const [message, setMessage] = useState<string | null>(null);
  const [refreshToken, setRefreshToken] = useState(0);
  const [creating, setCreating] = useState(false);
  const [createName, setCreateName] = useState("");
  const [renaming, setRenaming] = useState(false);
  const [renameName, setRenameName] = useState("");
  const [busyAction, setBusyAction] = useState(false);

  const selected = collections.find((collection) => collection.id === selectedId) ?? collections[0] ?? null;
  const nameDialogOpen = creating || renaming;
  const nameDialogValue = creating ? createName : renameName;
  const nameDialogTitle = creating ? "新建集合" : "重命名集合";
  const nameDialogPlaceholder = creating ? "新集合名称" : "集合名称";

  useEffect(() => {
    void refresh();
    invoke<KnowledgeSettings>("knowledge_get_settings")
      .then(setSettings)
      .catch((error) => setMessage(String(error)));
  }, []);

  async function refresh() {
    setLoading(true);
    try {
      const next = await invoke<KnowledgeCollection[]>("knowledge_list_collections");
      setCollections(next);
      setSelectedId((current) => current ?? next[0]?.id ?? null);
    } catch (error) {
      setMessage(String(error));
    } finally {
      setLoading(false);
    }
  }

  async function createCollection() {
    const name = createName.trim();
    if (!name) {
      setMessage("集合名称不能为空");
      return;
    }
    setBusyAction(true);
    try {
      const created = await invoke<KnowledgeCollection>("knowledge_create_collection", { name });
      setCollections((prev) => [created, ...prev]);
      setSelectedId(created.id);
      setCreateName("");
      setCreating(false);
      setMessage("集合已创建");
    } catch (error) {
      setMessage(String(error));
    } finally {
      setBusyAction(false);
    }
  }

  async function renameSelected() {
    if (!selected) return;
    const name = renameName.trim();
    if (!name) {
      setMessage("集合名称不能为空");
      return;
    }
    setBusyAction(true);
    try {
      const updated = await invoke<KnowledgeCollection>("knowledge_update_collection", {
        collectionId: selected.id,
        name,
      });
      setCollections((prev) => prev.map((collection) => (collection.id === updated.id ? updated : collection)));
      setRenaming(false);
      setMessage("集合已重命名");
    } catch (error) {
      setMessage(String(error));
    } finally {
      setBusyAction(false);
    }
  }

  async function deleteSelected() {
    if (!selected) return;
    const confirmed = await confirm(`删除知识库集合「${selected.name}」？该操作会删除集合目录。`, {
      title: "删除知识库集合",
      kind: "warning",
    });
    if (!confirmed) return;
    setBusyAction(true);
    try {
      await invoke("knowledge_delete_collection", { collectionId: selected.id });
      setCollections((prev) => {
        const next = prev.filter((collection) => collection.id !== selected.id);
        setSelectedId(next[0]?.id ?? null);
        return next;
      });
      setRenaming(false);
      setMessage("集合已删除");
    } catch (error) {
      setMessage(String(error));
    } finally {
      setBusyAction(false);
    }
  }

  async function importSources() {
    if (!selected) return;
    const selectedFiles = await open({
      multiple: true,
      directory: false,
      title: "导入知识库源文件",
    });
    if (!selectedFiles) return;
    const paths = Array.isArray(selectedFiles) ? selectedFiles : [selectedFiles];
    if (paths.length === 0) return;
    setBusyAction(true);
    setMessage("正在导入，LLM 解析可能需要一些时间...");
    try {
      await invoke("knowledge_import_sources", { collectionId: selected.id, paths });
      setMessage("导入完成");
      setRefreshToken((value) => value + 1);
    } catch (error) {
      setMessage(String(error));
    } finally {
      setBusyAction(false);
    }
  }

  function closeNameDialog() {
    setCreating(false);
    setRenaming(false);
    setCreateName("");
    setRenameName("");
  }

  function submitNameDialog(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (creating) {
      void createCollection();
      return;
    }
    void renameSelected();
  }

  return (
    <div style={s.knowledgePane}>
      <aside style={s.knowledgeSidebar}>
        <div style={s.knowledgeSidebarHeader}>
          <div style={{ display: "flex", alignItems: "center", gap: 9 }}>
            <Database size={18} color="var(--accent)" />
            <div style={s.knowledgeTitle}>知识库</div>
          </div>
          <div style={s.knowledgeSubtitle}>应用内集合统一管理，供 Wiki 页面、向量检索和 Agent 工具使用。</div>
          {message && <div style={{ marginTop: 10, color: "var(--text-muted)", fontSize: 12 }}>{message}</div>}
        </div>
        <div style={s.knowledgeToolbar}>
          <button
            style={s.knowledgeSmallBtn}
            onClick={() => {
              setCreating(true);
              setRenaming(false);
              setMessage(null);
            }}
            disabled={busyAction}
          >
            <Plus size={14} />
            新建
          </button>
          <button
            style={s.knowledgeSmallBtn}
            onClick={importSources}
            disabled={!selected || busyAction}
            aria-label="导入源文件"
          >
            <Upload size={14} />
            导入
          </button>
          <button
            style={s.knowledgeSmallBtn}
            onClick={deleteSelected}
            disabled={!selected || busyAction}
            aria-label="删除集合"
          >
            <Trash2 size={14} />
          </button>
        </div>
        <div style={s.knowledgeList}>
          {loading ? (
            <div style={{ padding: 12, color: "var(--text-muted)", fontSize: 12 }}>加载中...</div>
          ) : collections.length === 0 ? (
            <div style={{ padding: 12, color: "var(--text-muted)", fontSize: 12, lineHeight: 1.55 }}>
              还没有集合。新建一个，别让知识继续流浪。
            </div>
          ) : (
            collections.map((collection) => {
              const active = collection.id === selected?.id;
              return (
                <div
                  key={collection.id}
                  role="button"
                  tabIndex={0}
                  style={{
                    ...s.knowledgeCollectionItem,
                    background: active ? "var(--bg-selected)" : "transparent",
                    borderColor: active ? "var(--border-medium)" : "transparent",
                  }}
                  onClick={() => setSelectedId(collection.id)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter" || event.key === " ") {
                      event.preventDefault();
                      setSelectedId(collection.id);
                    }
                  }}
                >
                  <span style={s.knowledgeCollectionHeader}>
                    <span style={s.knowledgeCollectionName}>{collection.name}</span>
                    {active && (
                      <button
                        style={s.knowledgeIconBtn}
                        type="button"
                        aria-label="重命名集合"
                        disabled={busyAction}
                        onClick={(event) => {
                          event.stopPropagation();
                          setRenameName(collection.name);
                          setRenaming(true);
                          setCreating(false);
                          setMessage(null);
                        }}
                      >
                        <Pencil size={13} />
                      </button>
                    )}
                  </span>
                  <span style={s.knowledgeCollectionMeta}>{collection.rootPath}</span>
                </div>
              );
            })
          )}
        </div>
      </aside>
      {selected ? (
        <KnowledgeWorkbench
          collection={selected}
          settings={settings}
          onSettingsSaved={setSettings}
          onImportSources={importSources}
          importRefreshToken={refreshToken}
        />
      ) : (
        <main style={s.knowledgeMain}>
          <div style={{ ...s.emptyState, background: "var(--bg-panel)" }}>
            <BookOpen size={40} color="var(--text-hint)" />
            <div style={{ marginTop: 14, fontSize: 14, fontWeight: 700, color: "var(--text-secondary)" }}>
              新建集合后开始导入资料
            </div>
          </div>
        </main>
      )}
      {nameDialogOpen && (
        <div style={s.knowledgeDialogOverlay} onMouseDown={closeNameDialog}>
          <form
            style={s.knowledgeDialogBox}
            onSubmit={submitNameDialog}
            onMouseDown={(event) => event.stopPropagation()}
          >
            <div style={s.knowledgeDialogHeader}>
              <div>
                <div style={s.knowledgeDialogTitle}>{nameDialogTitle}</div>
                <div style={s.knowledgeDialogSubtitle}>
                  {creating ? "创建一个独立知识库集合。" : selected?.rootPath}
                </div>
              </div>
              <button
                style={s.knowledgeIconBtn}
                type="button"
                aria-label="关闭集合名称弹层"
                onClick={closeNameDialog}
                disabled={busyAction}
              >
                <X size={14} />
              </button>
            </div>
            <input
              style={s.knowledgeInput}
              value={nameDialogValue}
              placeholder={nameDialogPlaceholder}
              autoFocus
              onChange={(event) => {
                if (creating) {
                  setCreateName(event.target.value);
                } else {
                  setRenameName(event.target.value);
                }
              }}
            />
            <div style={s.knowledgeDialogFooter}>
              <button style={s.knowledgeSmallBtn} type="button" onClick={closeNameDialog} disabled={busyAction}>
                取消
              </button>
              <button style={s.knowledgeSmallBtn} type="submit" disabled={busyAction}>
                <Check size={14} />
                确认
              </button>
            </div>
          </form>
        </div>
      )}
    </div>
  );
}
