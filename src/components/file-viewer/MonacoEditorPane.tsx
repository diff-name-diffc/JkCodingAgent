import Editor, { loader, type Monaco } from "@monaco-editor/react";
import * as monaco from "monaco-editor";
import type * as MonacoTypes from "monaco-editor";
import editorWorker from "monaco-editor/editor/editor.worker?worker";
import jsonWorker from "monaco-editor/languages/features/json/json.worker?worker";
import cssWorker from "monaco-editor/languages/features/css/css.worker?worker";
import htmlWorker from "monaco-editor/languages/features/html/html.worker?worker";
import tsWorker from "monaco-editor/languages/features/typescript/ts.worker?worker";
import { forwardRef, useCallback, useEffect, useImperativeHandle, useRef } from "react";
import { useIsDarkTheme } from "../../hooks/useIsDarkTheme";

const MONACO_THEME_LIGHT = "nezha-light";
const MONACO_THEME_DARK = "nezha-dark";

let monacoConfigured = false;
let themesRegistered = false;

function ensureMonacoLoader() {
  if (monacoConfigured) {
    return;
  }

  self.MonacoEnvironment = {
    getWorker(_workerId, label) {
      if (label === "json") {
        return new jsonWorker();
      }

      if (label === "css" || label === "scss" || label === "less") {
        return new cssWorker();
      }

      if (label === "html" || label === "handlebars" || label === "razor") {
        return new htmlWorker();
      }

      if (label === "typescript" || label === "javascript") {
        return new tsWorker();
      }

      return new editorWorker();
    },
  };

  loader.config({ monaco });
  monacoConfigured = true;
}

function ensureMonacoThemes(monacoInstance: Monaco) {
  if (themesRegistered) {
    return;
  }

  monacoInstance.editor.defineTheme(MONACO_THEME_LIGHT, {
    base: "vs",
    inherit: true,
    rules: [
      { token: "comment", foreground: "6B6B6B" },
      { token: "string", foreground: "315F8C" },
      { token: "keyword", foreground: "7353A6" },
      { token: "number", foreground: "9A551F" },
      { token: "type", foreground: "7B5A32" },
      { token: "tag", foreground: "365F9B" },
      { token: "attribute.name", foreground: "7353A6" },
      { token: "link", foreground: "365F9B" },
      { token: "meta.separator", foreground: "666666" },
      { token: "emphasis", fontStyle: "italic" },
      { token: "strong", fontStyle: "bold" },
    ],
    colors: {
      "editor.background": "#F7F7F7",
      "editor.foreground": "#262626",
      "editor.lineHighlightBackground": "#EEEEEE",
      "editorLineNumber.foreground": "#858585",
      "editorLineNumber.activeForeground": "#525252",
      "editor.selectionBackground": "#DCE9F9",
      "editor.inactiveSelectionBackground": "#E8EDF4",
      "editorCursor.foreground": "#404040",
      "editorIndentGuide.background1": "#E3E3E3",
      "editorIndentGuide.activeBackground1": "#BDBDBD",
    },
  });

  monacoInstance.editor.defineTheme(MONACO_THEME_DARK, {
    base: "vs-dark",
    inherit: true,
    rules: [
      { token: "comment", foreground: "A3A3A3" },
      { token: "string", foreground: "A1BFDD" },
      { token: "keyword", foreground: "B6A0D8" },
      { token: "number", foreground: "D8A373" },
      { token: "type", foreground: "D4B58D" },
      { token: "tag", foreground: "91B4DF" },
      { token: "attribute.name", foreground: "B6A0D8" },
      { token: "link", foreground: "91B4DF" },
      { token: "meta.separator", foreground: "A3A3A3" },
      { token: "emphasis", fontStyle: "italic" },
      { token: "strong", fontStyle: "bold" },
    ],
    colors: {
      "editor.background": "#1B1B1B",
      "editor.foreground": "#E5E5E5",
      "editor.lineHighlightBackground": "#242424",
      "editorLineNumber.foreground": "#787878",
      "editorLineNumber.activeForeground": "#B8B8B8",
      "editor.selectionBackground": "#293B55",
      "editor.inactiveSelectionBackground": "#29303A",
      "editorCursor.foreground": "#E5E5E5",
      "editorIndentGuide.background1": "#303030",
      "editorIndentGuide.activeBackground1": "#525252",
    },
  });

  themesRegistered = true;
}

ensureMonacoLoader();

export interface MonacoEditorHandle {
  /** Get current editor content without triggering React re-render */
  getValue(): string;
  /** Replace editor content (used when switching tabs) */
  setValue(content: string, filePath: string, language: string): void;
  /** Save current view state (scroll position, cursor, etc.) */
  saveViewState(): MonacoTypes.editor.ICodeEditorViewState | null;
  /** Restore a previously saved view state */
  restoreViewState(state: MonacoTypes.editor.ICodeEditorViewState | null): void;
}

export const MonacoEditorPane = forwardRef<
  MonacoEditorHandle,
  {
    active?: boolean;
    initialValue: string;
    filePath: string;
    language: string;
    onChange: (value: string) => void;
  }
>(function MonacoEditorPane({ active = true, initialValue, filePath, language, onChange }, ref) {
  const isDark = useIsDarkTheme();
  const editorRef = useRef<MonacoTypes.editor.IStandaloneCodeEditor | null>(null);
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;

  // Dispose listener on unmount
  const listenerRef = useRef<MonacoTypes.IDisposable | null>(null);

  useImperativeHandle(
    ref,
    () => ({
      getValue() {
        return editorRef.current?.getValue() ?? "";
      },
      setValue(content: string, newPath: string, newLang: string) {
        const editor = editorRef.current;
        if (!editor) return;

        // Dispose old model listener before switching
        listenerRef.current?.dispose();

        // Dispose old model if it's no longer referenced by any other editor
        const oldModel = editor.getModel();

        const monacoInstance = monaco;
        // Reuse existing model for same URI, or create new one
        const uri = monacoInstance.Uri.parse(`file://${newPath}`);
        let model = monacoInstance.editor.getModel(uri);
        if (model) {
          // Model exists; update content only if different
          if (model.getValue() !== content) {
            model.setValue(content);
          }
          // Ensure language matches
          monacoInstance.editor.setModelLanguage(model, newLang);
        } else {
          model = monacoInstance.editor.createModel(content, newLang, uri);
        }
        editor.setModel(model);

        // Dispose old model after setting the new one (avoid disposing a reused model)
        if (oldModel && oldModel !== model && !oldModel.isDisposed()) {
          oldModel.dispose();
        }

        // Re-attach content change listener for the new model
        listenerRef.current = editor.onDidChangeModelContent(() => {
          onChangeRef.current(editor.getValue());
        });
      },
      saveViewState() {
        return editorRef.current?.saveViewState() ?? null;
      },
      restoreViewState(state: MonacoTypes.editor.ICodeEditorViewState | null) {
        if (state) {
          editorRef.current?.restoreViewState(state);
        }
      },
    }),
    [],
  );

  const handleMount = useCallback((editor: MonacoTypes.editor.IStandaloneCodeEditor) => {
    editorRef.current = editor;

    // Guard: if Monaco mounted before container had final dimensions, force re-layout
    const { width, height } = editor.getLayoutInfo();
    if (height === 0 || width === 0) {
      requestAnimationFrame(() => editor.layout());
    }

    // Attach content change listener (uncontrolled — no React setState)
    listenerRef.current = editor.onDidChangeModelContent(() => {
      onChangeRef.current(editor.getValue());
    });
  }, []);

  // Cleanup on unmount — dispose listener and model to prevent memory leaks
  useEffect(() => {
    return () => {
      listenerRef.current?.dispose();
      const model = editorRef.current?.getModel();
      if (model && !model.isDisposed()) {
        model.dispose();
      }
    };
  }, []);

  useEffect(() => {
    if (!active || !editorRef.current) {
      return;
    }

    requestAnimationFrame(() => {
      editorRef.current?.layout();
    });
  }, [active]);

  return (
    <div className="monaco-pane">
      <Editor
        path={filePath}
        defaultValue={initialValue}
        language={language}
        theme={isDark ? MONACO_THEME_DARK : MONACO_THEME_LIGHT}
        beforeMount={(monacoInstance) => {
          ensureMonacoThemes(monacoInstance);
        }}
        onMount={handleMount}
        onChange={undefined}
        loading={<div className="monaco-loading">编辑器加载中...</div>}
        options={{
          automaticLayout: true,
          minimap: { enabled: false },
          smoothScrolling: true,
          scrollBeyondLastLine: false,
          fontFamily: "JetBrains Mono, monospace",
          fontLigatures: true,
          fontSize: 13,
          lineHeight: 22,
          wordWrap: "off",
          padding: { top: 18, bottom: 24 },
          renderWhitespace: "selection",
          cursorBlinking: "smooth",
          cursorSmoothCaretAnimation: "on",
          overviewRulerBorder: false,
          guides: {
            bracketPairs: true,
            highlightActiveBracketPair: true,
            indentation: true,
          },
          bracketPairColorization: { enabled: true },
          stickyScroll: { enabled: true },
          quickSuggestions: {
            comments: false,
            strings: false,
            other: true,
          },
          suggestOnTriggerCharacters: true,
          tabSize: 2,
          insertSpaces: true,
          scrollbar: {
            verticalScrollbarSize: 10,
            horizontalScrollbarSize: 10,
          },
        }}
        wrapperProps={{
          style: {
            display: "flex",
            flex: 1,
            minHeight: 0,
            minWidth: 0,
          },
        }}
      />
    </div>
  );
});
