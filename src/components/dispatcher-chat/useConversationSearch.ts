import { useState, useRef, useEffect, useLayoutEffect, useCallback } from "react";
import type { DispatcherMessage } from "../../types";
import type { AssistantThinkingBlock, AssistantTurnSegment } from "../dispatcherChatView";
import {
  unwrapConversationSearchMatches,
  highlightConversationSearchMatches,
  SEARCH_MATCH_SELECTOR,
} from "./dispatcherChatUtils";

export interface UseConversationSearchOptions {
  messageListRef: React.RefObject<HTMLDivElement | null>;
  inputRef: React.RefObject<HTMLTextAreaElement | null>;
  messages: DispatcherMessage[];
  streamingSegments: AssistantTurnSegment[];
  liveThinking: AssistantThinkingBlock | null;
  assistantPlaceholder: string | null;
}

export interface UseConversationSearchResult {
  searchOpen: boolean;
  searchQuery: string;
  matchCount: number;
  activeIndex: number;
  searchInputRef: React.RefObject<HTMLInputElement | null>;
  focusSearch: () => void;
  closeSearch: () => void;
  handleSearchChange: (event: React.ChangeEvent<HTMLInputElement>) => void;
  handleSearchKeyDown: (event: React.KeyboardEvent<HTMLInputElement>) => void;
  moveSearchMatch: (direction: 1 | -1) => void;
}

export function useConversationSearch({
  messageListRef,
  inputRef,
  messages,
  streamingSegments,
  liveThinking,
  assistantPlaceholder,
}: UseConversationSearchOptions): UseConversationSearchResult {
  const [searchOpen, setSearchOpen] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");
  const [matchCount, setMatchCount] = useState(0);
  const [activeIndex, setActiveIndex] = useState(0);
  const searchInputRef = useRef<HTMLInputElement>(null);

  const normalizedQuery = searchQuery.trim();

  const focusSearch = useCallback(() => {
    setSearchOpen(true);
    window.requestAnimationFrame(() => {
      searchInputRef.current?.focus();
      searchInputRef.current?.select();
    });
  }, []);

  const closeSearch = useCallback(() => {
    setSearchOpen(false);
    setSearchQuery("");
    setMatchCount(0);
    setActiveIndex(0);
  }, []);

  const moveSearchMatch = useCallback(
    (direction: 1 | -1) => {
      setActiveIndex((current) => {
        if (matchCount <= 0) return 0;
        return (current + direction + matchCount) % matchCount;
      });
    },
    [matchCount],
  );

  const handleSearchChange = useCallback(
    (event: React.ChangeEvent<HTMLInputElement>) => {
      setSearchQuery(event.target.value);
      setActiveIndex(0);
    },
    [],
  );

  const handleSearchKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLInputElement>) => {
      if (event.key === "ArrowDown" || (event.key === "Enter" && !event.shiftKey)) {
        event.preventDefault();
        moveSearchMatch(1);
        return;
      }
      if (event.key === "ArrowUp" || (event.key === "Enter" && event.shiftKey)) {
        event.preventDefault();
        moveSearchMatch(-1);
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        closeSearch();
        inputRef.current?.focus();
      }
    },
    [closeSearch, inputRef, moveSearchMatch],
  );

  // Global Ctrl+F shortcut
  useEffect(() => {
    const handleWindowKeyDown = (event: globalThis.KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "f") {
        event.preventDefault();
        focusSearch();
      }
    };
    window.addEventListener("keydown", handleWindowKeyDown);
    return () => window.removeEventListener("keydown", handleWindowKeyDown);
  }, [focusSearch]);

  // Highlight search matches in the message list DOM
  useLayoutEffect(() => {
    const root = messageListRef.current;
    if (!root) return;

    unwrapConversationSearchMatches(root);
    if (!normalizedQuery) {
      setMatchCount(0);
      return;
    }

    const count = highlightConversationSearchMatches(root, normalizedQuery);
    setMatchCount(count);
    setActiveIndex((current) => (count <= 0 ? 0 : current >= count ? count - 1 : current));

    return () => {
      unwrapConversationSearchMatches(root);
    };
  }, [assistantPlaceholder, liveThinking?.text, messageListRef, messages, normalizedQuery, streamingSegments]);

  // Scroll the active match into view
  useLayoutEffect(() => {
    const root = messageListRef.current;
    if (!root || !normalizedQuery) return;

    const matches = Array.from(root.querySelectorAll<HTMLElement>(SEARCH_MATCH_SELECTOR));
    for (const match of matches) {
      const isActive = Number(match.dataset.searchMatchIndex) === activeIndex;
      match.classList.toggle("dispatcher-search-match--active", isActive);
    }

    const activeMatch = matches.find(
      (match) => Number(match.dataset.searchMatchIndex) === activeIndex,
    );
    activeMatch?.scrollIntoView({ block: "center", inline: "nearest", behavior: "smooth" });
  }, [activeIndex, matchCount, messageListRef, normalizedQuery]);

  return {
    searchOpen,
    searchQuery,
    matchCount,
    activeIndex,
    searchInputRef,
    focusSearch,
    closeSearch,
    handleSearchChange,
    handleSearchKeyDown,
    moveSearchMatch,
  };
}
