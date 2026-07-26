/**
 * 模型库条目 / SSH 服务器的最近测试结果。
 * 这些信息没有对应的存储字段，因此只存 localStorage，属于纯 UI 偏好，换设备不同步。
 * 模型条目的 key 为条目 id（ModelLibraryEntry.id）。
 */

export type ProviderTestRecord = { status: "ok" | "failed"; at: number };

export type ProviderPref = {
  lastTest?: ProviderTestRecord;
};

const PROVIDER_PREFS_KEY = "jkcodingagent.settings.providers.v1";
const SSH_TEST_PREFS_KEY = "jkcodingagent.settings.ssh-tests.v1";

function readJson<T>(key: string, fallback: T): T {
  try {
    const raw = window.localStorage.getItem(key);
    return raw ? (JSON.parse(raw) as T) : fallback;
  } catch {
    return fallback;
  }
}

function writeJson(key: string, value: unknown) {
  try {
    window.localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // localStorage 不可用（隐私模式等）时静默降级为会话内行为。
  }
}

export function loadProviderPrefs(): Record<string, ProviderPref> {
  return readJson(PROVIDER_PREFS_KEY, {});
}

export function patchProviderPref(id: string, patch: Partial<ProviderPref>): void {
  const prefs = loadProviderPrefs();
  prefs[id] = { ...prefs[id], ...patch };
  writeJson(PROVIDER_PREFS_KEY, prefs);
}

export function removeProviderPref(id: string): void {
  const prefs = loadProviderPrefs();
  delete prefs[id];
  writeJson(PROVIDER_PREFS_KEY, prefs);
}

// ── SSH 服务器最近测试记录 ────────────────────────────────────────────────────

export function loadSshTestRecords(): Record<string, ProviderTestRecord> {
  return readJson(SSH_TEST_PREFS_KEY, {});
}

export function recordSshTest(serverId: string, record: ProviderTestRecord): void {
  const records = loadSshTestRecords();
  records[serverId] = record;
  writeJson(SSH_TEST_PREFS_KEY, records);
}
