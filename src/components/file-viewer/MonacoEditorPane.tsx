import Editor, { loader, type Monaco } from "@monaco-editor/react";
import * as monaco from "monaco-editor";
import type * as MonacoTypes from "monaco-editor";
import editorWorker from "monaco-editor/esm/vs/editor/editor.worker?worker";
import jsonWorker from "monaco-editor/esm/vs/language/json/json.worker?worker";
import cssWorker from "monaco-editor/esm/vs/language/css/css.worker?worker";
import htmlWorker from "monaco-editor/esm/vs/language/html/html.worker?worker";
import tsWorker from "monaco-editor/esm/vs/language/typescript/ts.worker?worker";
import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useRef,
} from "react";

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
    rules: [],
    colors: {
      "editor.background": "#FCFCFD",
      "editor.foreground": "#171B24",
      "editor.lineHighlightBackground": "#F4F7FD",
      "editorLineNumber.foreground": "#A4ADBC",
      "editorLineNumber.activeForeground": "#5D7CFF",
      "editor.selectionBackground": "#D7E2FF",
      "editor.inactiveSelectionBackground": "#E7EDF9",
      "editorCursor.foreground": "#4467F5",
      "editorIndentGuide.background1": "#E6EAF3",
      "editorIndentGuide.activeBackground1": "#C8D3EC",
    },
  });

  monacoInstance.editor.defineTheme(MONACO_THEME_DARK, {
    base: "vs-dark",
    inherit: true,
    rules: [],
    colors: {
      "editor.background": "#232936",
      "editor.foreground": "#F1F4FB",
      "editor.lineHighlightBackground": "#2D3442",
      "editorLineNumber.foreground": "#69758A",
      "editorLineNumber.activeForeground": "#7F9AFF",
      "editor.selectionBackground": "#33446D",
      "editor.inactiveSelectionBackground": "#2C364F",
      "editorCursor.foreground": "#AFC0FF",
      "editorIndentGuide.background1": "#343B4A",
      "editorIndentGuide.activeBackground1": "#4A5878",
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
    isDark: boolean;
    onChange: (value: string) => void;
  }
>(function MonacoEditorPane(
  { active = true, initialValue, filePath, language, isDark, onChange },
  ref,
) {
  const editorRef = useRef<MonacoTypes.editor.IStandaloneCodeEditor | null>(
    null,
  );
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;

  // Dispose listener on unmount
  const listenerRef =
    useRef<MonacoTypes.IDisposable | null>(null);

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
      restoreViewState(
        state: MonacoTypes.editor.ICodeEditorViewState | null,
      ) {
        if (state) {
          editorRef.current?.restoreViewState(state);
        }
      },
    }),
    [],
  );

  const handleMount = useCallback(
    (editor: MonacoTypes.editor.IStandaloneCodeEditor) => {
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
    },
    [],
  );

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
        loading={<div className="monaco-loading">Loading editor...</div>}
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
