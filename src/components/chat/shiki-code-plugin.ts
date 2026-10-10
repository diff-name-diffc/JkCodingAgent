import type { CodeHighlighterPlugin } from "streamdown";
import {
  isSupportedHighlightLanguage,
  SUPPORTED_HIGHLIGHT_LANGUAGES,
  NEUTRAL_DARK_THEME,
  NEUTRAL_LIGHT_THEME,
  tokenizeCodeDualTheme,
} from "../../utils/shiki";

/**
 * streamdown 代码高亮插件（自研），替代 @streamdown/code：
 * 复用 utils/shiki.ts 的 shiki core 单 highlighter + 13 语言按需 import +
 * 双中性主题，避免 @streamdown/code 拖入的 shiki 全量语言/主题注册表
 * （300+ 动态 chunk）与第二份 shiki 运行时。
 *
 * 调用约定与 @streamdown/code 一致：命中缓存同步返回 TokensResult，未命中
 * 返回 null 并异步回填（streamdown 先渲纯文本骨架，callback 触发后替换）。
 * 主题固定为亮暗两套（预载于 highlighter），不参与缓存键。
 */

type TokenResult = NonNullable<ReturnType<CodeHighlighterPlugin["highlight"]>>;
type HighlightCallback = (result: TokenResult) => void;
type SupportedLanguages = ReturnType<CodeHighlighterPlugin["getSupportedLanguages"]>;

const MAX_CACHED_CODE_LENGTH = 64 * 1024;
const TOKEN_CACHE_CAPACITY = 200;

// LRU：与 utils/shiki-cache.ts 同构（Map 插入序 + get 命中提升），value 为 TokensResult。
const tokenCache = new Map<string, TokenResult>();

function cacheGet(key: string): TokenResult | undefined {
  const hit = tokenCache.get(key);
  if (hit !== undefined) {
    tokenCache.delete(key);
    tokenCache.set(key, hit);
  }
  return hit;
}

function cacheSet(key: string, value: TokenResult, codeLength: number): void {
  if (codeLength > MAX_CACHED_CODE_LENGTH) return;
  if (tokenCache.has(key)) tokenCache.delete(key);
  tokenCache.set(key, value);
  while (tokenCache.size > TOKEN_CACHE_CAPACITY) {
    const oldest = tokenCache.keys().next();
    if (oldest.done) break;
    tokenCache.delete(oldest.value);
  }
}

const pendingCallbacks = new Map<string, Set<HighlightCallback>>();
const inflightKeys = new Set<string>();

export function createChatCodePlugin(): CodeHighlighterPlugin {
  return {
    name: "shiki",
    type: "code-highlighter",
    supportsLanguage(language) {
      return isSupportedHighlightLanguage(language);
    },
    getSupportedLanguages() {
      return [...SUPPORTED_HIGHLIGHT_LANGUAGES] as SupportedLanguages;
    },
    getThemes() {
      return [NEUTRAL_LIGHT_THEME, NEUTRAL_DARK_THEME];
    },
    highlight({ code, language }, callback) {
      const lang = (language ?? "").trim().toLowerCase();
      const key = `${lang}\x00${code}`;

      const cached = cacheGet(key);
      if (cached) return cached;

      if (callback) {
        let set = pendingCallbacks.get(key);
        if (!set) {
          set = new Set();
          pendingCallbacks.set(key, set);
        }
        set.add(callback as HighlightCallback);
      }
      if (inflightKeys.has(key)) return null;
      inflightKeys.add(key);

      tokenizeCodeDualTheme(code, language)
        .then((tokens) => {
          const result = tokens as unknown as TokenResult;
          cacheSet(key, result, code.length);
          const set = pendingCallbacks.get(key);
          if (set) {
            for (const cb of set) cb(result);
          }
        })
        .catch((error) => {
          console.error("[chat-code-plugin] 代码高亮失败:", error);
        })
        .finally(() => {
          pendingCallbacks.delete(key);
          inflightKeys.delete(key);
        });

      return null;
    },
  };
}
