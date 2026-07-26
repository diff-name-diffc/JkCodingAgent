import { useMemo, useState } from "react";
import { Plus, Server } from "lucide-react";
import { useAhaSettings } from "../use-aha-settings";
import { EmptyState } from "../EmptyState";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "../../ui/tabs";
import type { ModelCategory } from "../../../types";
import { loadProviderPrefs, patchProviderPref, removeProviderPref } from "./provider-prefs";
import {
  CATEGORY_DEFS,
  categoryDef,
  createEntry,
  entriesForCategory,
  entryUsageTitles,
  patchLibraryEntry,
  removeLibraryEntry,
  upsertLibraryEntry,
} from "./model-library";
import { ModelEntryCard } from "./ModelEntryCard";

/**
 * 「模型服务」页：按模型调用方式分标签（对话/视觉/图片/语音/向量），
 * 每个标签页内维护该分类的模型条目（各自独立的 URL / API Key / model），
 * 「模型用途」页从对应分类的条目中引用。
 */
export function ProvidersPage({ initialCategory }: { initialCategory?: ModelCategory }) {
  const store = useAhaSettings();
  const [activeCategory, setActiveCategory] = useState<ModelCategory>(
    initialCategory ?? "text",
  );
  const [expandedId, setExpandedId] = useState<string | null>(null);
  // prefs（最近测试结果）存于 localStorage，非 React 状态；prefsVersion 作为版本号触发重读。
  const [prefsVersion, setPrefsVersion] = useState(0);

  // eslint-disable-next-line react-hooks/exhaustive-deps
  const prefs = useMemo(() => loadProviderPrefs(), [prefsVersion]);

  function changePref(id: string, patch: Parameters<typeof patchProviderPref>[1]) {
    patchProviderPref(id, patch);
    setPrefsVersion((v) => v + 1);
  }

  function handleAdd(category: ModelCategory) {
    const entry = createEntry(category);
    store.updateSettings((prev) => upsertLibraryEntry(prev, entry), `model:${entry.id}`);
    setExpandedId(entry.id);
  }

  if (store.loading || !store.settings) {
    return <div className="ai-settings-empty">加载中...</div>;
  }

  const settings = store.settings;

  return (
    <div className="ai-set-page">
      <div className="ai-set-page-head">
        <div>
          <h2 className="ai-set-page-title">模型服务</h2>
          <p className="ai-set-page-description">
            按模型调用方式分类维护模型（各自独立的地址和密钥），然后在「模型用途」中为各功能选择模型。
          </p>
        </div>
      </div>

      <Tabs
        value={activeCategory}
        onValueChange={(value) => setActiveCategory(value as ModelCategory)}
      >
        <TabsList className="ai-set-tabs-list">
          {CATEGORY_DEFS.map((def) => (
            <TabsTrigger key={def.category} value={def.category}>
              {def.label}
            </TabsTrigger>
          ))}
        </TabsList>

        {CATEGORY_DEFS.map((def) => {
          const entries = entriesForCategory(settings.modelLibrary ?? [], def.category);
          return (
            <TabsContent key={def.category} value={def.category}>
              <div className="ai-set-category">
                <div className="ai-set-category-head">
                  <p className="ai-set-category-description">{def.description}</p>
                  <button
                    type="button"
                    className="ai-primary-button"
                    onClick={() => handleAdd(def.category)}
                  >
                    <Plus size={16} strokeWidth={1.5} />
                    添加模型
                  </button>
                </div>

                {entries.length === 0 ? (
                  <EmptyState
                    icon={Server}
                    title={`还没有${def.label}。添加一个，即可在「模型用途」中绑定。`}
                    actionLabel="添加模型"
                    onAction={() => handleAdd(def.category)}
                  />
                ) : (
                  <div className="ai-set-provider-list">
                    {entries.map((entry) => (
                      <ModelEntryCard
                        key={entry.id}
                        entry={entry}
                        def={categoryDef(entry.category)}
                        expanded={expandedId === entry.id}
                        usageTitles={entryUsageTitles(settings, entry)}
                        testStatus={
                          prefs[entry.id]?.lastTest?.status === "ok"
                            ? "ok"
                            : prefs[entry.id]?.lastTest?.status === "failed"
                              ? "failed"
                              : "untested"
                        }
                        onToggleExpand={() =>
                          setExpandedId((current) => (current === entry.id ? null : entry.id))
                        }
                        onPatch={(patch) =>
                          store.updateSettings(
                            (prev) => patchLibraryEntry(prev, entry.id, patch),
                            `model:${entry.id}`,
                          )
                        }
                        onRemove={() => {
                          store.updateSettings((prev) => removeLibraryEntry(prev, entry.id));
                          removeProviderPref(entry.id);
                          setPrefsVersion((v) => v + 1);
                        }}
                        onTestResult={(status) =>
                          changePref(entry.id, { lastTest: { status, at: Date.now() } })
                        }
                      />
                    ))}
                  </div>
                )}
              </div>
            </TabsContent>
          );
        })}
      </Tabs>
    </div>
  );
}
