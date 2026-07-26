import { useAhaSettings } from "../use-aha-settings";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../../ui/select";
import type { ModelCategory } from "../../../types";
import {
  bindPurpose,
  getPurposeBinding,
  modelCapabilityTags,
  type PurposeDef,
} from "./provider-registry";
import {
  entriesForCategory,
  entryLabel,
  findEnabledEntryForConfig,
  purposeCategory,
} from "./model-library";

const UNBOUND = "__unbound__";
/** 当前绑定不在模型库中（条目已删除/停用或旧配置）时的兜底选项值。 */
const EXTERNAL = "__external__";

/**
 * 模型用途绑定下拉：选项来自「模型服务」页按分类配置的模型库条目。
 * 选中后把条目的 url/apiKey/model 拷贝进用途槽位（落盘结构与运行时不变）。
 */
export function PurposeSelect({
  def,
  onNavigateProviders,
}: {
  def: PurposeDef;
  onNavigateProviders: (category: ModelCategory) => void;
}) {
  const store = useAhaSettings();
  const settings = store.settings;
  const category = purposeCategory(def.kind);
  const entries = entriesForCategory(settings?.modelLibrary ?? [], category, {
    enabledOnly: true,
  });

  const binding = settings ? getPurposeBinding(settings, def.kind) : null;
  const boundEntry = settings
    ? findEnabledEntryForConfig(settings.modelLibrary ?? [], binding)
    : undefined;
  // 绑定指向已停用或已删除的库条目、或尚未迁移的旧配置时，仍展示该选项让用户看到现状。
  const bindingExternal = Boolean(binding?.url.trim()) && !boundEntry;
  const bindingValue = boundEntry ? boundEntry.id : bindingExternal ? EXTERNAL : UNBOUND;

  const fieldId = `purpose:${def.kind}`;
  const fieldError = store.saveError?.fieldId === fieldId ? store.saveError.message : null;

  function handleChange(value: string) {
    if (value === UNBOUND) {
      store.updateSettings(
        (prev) => bindPurpose(prev, def.kind, { url: "", apiKey: "", model: "" }),
        fieldId,
      );
      return;
    }
    if (value === EXTERNAL) return;
    const entry = entries.find((item) => item.id === value);
    if (!entry) return;
    store.updateSettings((prev) => bindPurpose(prev, def.kind, entry), fieldId);
  }

  if (entries.length === 0 && !bindingExternal) {
    return (
      <div className="ai-set-purpose-empty">
        <span>该分类还没有可用的模型。</span>
        <button
          type="button"
          className="ai-set-link-button"
          onClick={() => onNavigateProviders(category)}
        >
          前往「模型服务」添加
        </button>
      </div>
    );
  }

  return (
    <div className="ai-set-purpose-select">
      <Select value={bindingValue} onValueChange={handleChange}>
        <SelectTrigger aria-label={def.title}>
          <SelectValue placeholder="未配置" />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value={UNBOUND}>未配置</SelectItem>
          {bindingExternal && binding && (
            <SelectItem value={EXTERNAL}>
              {binding.model || binding.url}（模型已停用或不在模型库中）
            </SelectItem>
          )}
          {entries.map((entry) => (
            <SelectItem key={entry.id} value={entry.id}>
              <span className="ai-set-purpose-option">
                <span className="ai-set-purpose-option-name">{entryLabel(entry)}</span>
                {modelCapabilityTags(entry.model).map((tag) => (
                  <span key={tag} className="ai-set-capability-badge">
                    {tag}
                  </span>
                ))}
              </span>
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      {fieldError && <div className="ai-set-field-error">{fieldError}</div>}
    </div>
  );
}
