import { useAhaSettings } from "../use-aha-settings";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../../ui/select";
import type { DispatcherModelConfig, ModelCategory } from "../../../types";
import {
  bindPurpose,
  getPurposeBinding,
  getPurposeConfigs,
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
const STALE = "__stale__";
/** 手动配置（url 非空、无库引用）：后端按有效配置保留，只展示、不作为可清除的残留项。 */
const MANUAL = "__manual__";

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

  // 用途槽位的原始配置。残留判定必须看原始配置：读取时后端会清空失效引用的
  // 凭据，而 getPurposeBinding 要求 url 非空，失效引用无法通过 binding 观察到。
  const rawConfig: DispatcherModelConfig | undefined = settings
    ? def.kind === "review"
      ? settings.review?.modelConfig
      : getPurposeConfigs(settings, def.kind).find((config) => config.active)
    : undefined;

  // 残留绑定：指向已停用/已删除的库条目（凭据已被后端清空），可清除。
  const staleConfig = !boundEntry && rawConfig?.libraryId ? rawConfig : undefined;
  // 手动配置：url 非空且无库引用。后端 resolve_from_library 对无引用配置原样
  // 保留、运行时可正常使用——不能当成残留清除，仅惰性展示现状。
  const manualConfig =
    !boundEntry && !rawConfig?.libraryId && rawConfig?.url.trim() ? rawConfig : undefined;

  const bindingValue = boundEntry
    ? boundEntry.id
    : staleConfig
      ? STALE
      : manualConfig
        ? MANUAL
        : UNBOUND;

  const fieldId = `purpose:${def.kind}`;
  const fieldError = store.saveError?.fieldId === fieldId ? store.saveError.message : null;

  function handleChange(value: string) {
    if (value === UNBOUND || value === STALE) {
      // 选中「未配置」或残留绑定项都执行解绑：后者借此值变化清除幽灵绑定。
      store.updateSettings(
        (prev) => bindPurpose(prev, def.kind, { url: "", apiKey: "", model: "" }),
        fieldId,
      );
      return;
    }
    if (value === MANUAL) return; // 惰性展示项：选中不产生变更
    const entry = entries.find((item) => item.id === value);
    if (!entry) return;
    store.updateSettings((prev) => bindPurpose(prev, def.kind, entry), fieldId);
  }

  if (entries.length === 0 && !staleConfig && !manualConfig) {
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

  const staleLabel = staleConfig
    ? staleConfig.model || staleConfig.url || staleConfig.libraryId || ""
    : "";
  const manualLabel = manualConfig ? manualConfig.model || manualConfig.url : "";

  return (
    <div className="ai-set-purpose-select">
      <Select value={bindingValue} onValueChange={handleChange}>
        <SelectTrigger aria-label={def.title}>
          <SelectValue placeholder="未配置" />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value={UNBOUND}>未配置</SelectItem>
          {staleConfig && (
            <SelectItem value={STALE}>
              <span className="ai-set-purpose-option">
                <span className="ai-set-purpose-option-name">
                  {staleLabel}（条目已停用或不在模型库中，改选「未配置」或其它模型可清除）
                </span>
              </span>
            </SelectItem>
          )}
          {manualConfig && (
            <SelectItem value={MANUAL}>
              <span className="ai-set-purpose-option">
                <span className="ai-set-purpose-option-name">{manualLabel}（手动配置）</span>
              </span>
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
