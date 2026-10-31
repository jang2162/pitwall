// SQL 콘솔의 편집기(14-m3-plan.md M3-15): CodeMirror 뷰 하나와 기존 SQL 강조(M1-17 — 구문 분석 서비스의 `SQL` 문법). 콘솔은 문서
// 모델(파일)이 아니라 콘솔 파일(`project.console.*`)의 글이라 뷰가 글을 쥐고, 바뀔 때마다 `onChange` 로 알린다(부르는 쪽이 모아
// 쓴다 — 미저장 개념 없음, D36).
//
// - 에디터 기능(src/editor/extensions)은 문서와 상관없는 것만 고른다: 기본 틀 · 테마 · 강조 · Tab 들여쓰기 · 커서 이동 키. undo ·
//   redo 의 키는 창의 키맵 액션(`editor.undo` — 포커스가 든 `.cm-editor` 의 `editorContext.history`)이 이 뷰의 CodeMirror 기록을 쓴다.
// - 강조: 창의 구문 세션은 문서 모델에 묶여 있어(syntax-sessions.ts) 콘솔은 자기 세션을 연다 — 콘솔 글의 바뀜을 문서 모델과 같은 꼴
//   (`reset` · `edit`)로 넘긴다. 세션 키(경로 자리)는 `pitwall-console:<consoleId>` 다.
// - 한글 IME: CodeMirror 의 조합 입력을 그대로 쓴다. 조합 중의 키는 창의 키맵이 보지 않는다(⌘↩ 이 조합 중에 실행되지 않는다).

import { history, redo, undo } from "@codemirror/commands";
import { type ChangeSet, EditorState, type Text } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import type { Judgment } from "@pitwall/rpc-client";
import { useEffect, useMemo, useRef, useState } from "react";

import { useEditorFont } from "../../editor/editor-font";
import type { DocumentChange } from "../../editor/documents";
import {
  type EditorFeature,
  type EditorFeatureContext,
  editorContext,
  editorExtensions,
  reconfigureEditor,
} from "../../editor/extensions";
import { base } from "../../editor/extensions/base";
import { editing } from "../../editor/extensions/editing";
import { highlight } from "../../editor/extensions/highlight";
import { navigation } from "../../editor/extensions/navigation";
import { readOnly as readOnlyFeature } from "../../editor/extensions/read-only";
import { theme } from "../../editor/extensions/theme";
import {
  createSyntaxSessions,
  type SyntaxSessions,
  type SyntaxSource,
} from "../../editor/syntax-sessions";
import { useTheme } from "../../theme/theme-switcher";

/** 콘솔 글의 판정 — 늘 고칠 수 있는 UTF-8 텍스트이고 강조한다(크기 상한은 구문 세션이 따로 본다). */
const CONSOLE_JUDGMENT: Judgment = {
  binary: false,
  encoding: "UTF-8",
  lossy: false,
  bom: null,
  lineEnding: "lf",
  longLines: false,
  sizeClass: "editable",
  editable: true,
  highlight: true,
};

/** 콘솔의 언어(Linguist 이름 — extensions/lang-sql 의 `linguist`). */
export const CONSOLE_LANGUAGE = "SQL";

const contextFeature: EditorFeature = { id: "context", extension: (c) => editorContext.of(c) };

/** 콘솔이 쓰는 에디터 기능(머리 주석). 순서는 조립 지점(extensions/index.ts)과 같다. */
const CONSOLE_FEATURES: readonly EditorFeature[] = [
  contextFeature,
  base,
  theme,
  highlight,
  editing,
  navigation,
  readOnlyFeature,
];

/** 콘솔 글의 바뀜을 구문 세션에 넘기는 자리(문서 모델의 `onChange` 와 같은 꼴). */
class ConsoleText {
  private readonly listeners = new Set<(change: DocumentChange) => void>();
  constructor(private text: Text) {}

  onChange(_path: string, listener: (change: DocumentChange) => void): () => void {
    this.listeners.add(listener);
    listener({ kind: "reset", text: this.text });
    return () => this.listeners.delete(listener);
  }

  edit(changes: ChangeSet, after: Text) {
    const before = this.text;
    this.text = after;
    for (const l of this.listeners) l({ kind: "edit", changes, before, after });
  }
}

export interface ConsoleEditorProps {
  /** 탭(패널) id. */
  panelId: string;
  /** 콘솔 id — 구문 세션의 키. */
  consoleId: string;
  /** 처음 글. `undefined` 면 아직 읽는 중(뷰를 만들지 않는다). */
  initial: string | undefined;
  /** 사람이 고칠 수 없다(콘솔 파일을 읽지 못했다). */
  readOnly?: boolean;
  onChange(text: string): void;
  /** 구문 분석 서비스(창의 Host 연결). 없으면 강조하지 않는다. */
  syntax?: SyntaxSource;
  /** 뷰가 생기고 사라질 때(실행 범위 · 액션이 뷰를 읽는다). */
  onView(view: EditorView | undefined): void;
}

export function ConsoleEditor({
  panelId,
  consoleId,
  initial,
  readOnly = false,
  onChange,
  syntax,
  onView,
}: ConsoleEditorProps) {
  const host = useRef<HTMLDivElement>(null);
  const appearance = useTheme((s) => s.appearance);
  const font = useEditorFont();
  const latest = useRef({ onChange, onView });
  latest.current = { onChange, onView };
  const [view, setView] = useState<EditorView>();
  // 구문 세션: 뷰 하나에 하나(콘솔 글은 이 뷰만 쓴다).
  const [sessions, setSessions] = useState<{ sessions: SyntaxSessions; text: ConsoleText }>();
  const path = `pitwall-console:${consoleId}`;

  const context = useMemo<EditorFeatureContext>(
    () => ({
      panelId,
      path,
      judgment: CONSOLE_JUDGMENT,
      readOnly,
      language: CONSOLE_LANGUAGE,
      appearance,
      onCursor: () => {},
      history: { undo, redo },
      ...(sessions && { syntax: sessions.sessions }),
      font,
      createFindPanel: () => ({ dom: document.createElement("div") }),
    }),
    [panelId, path, readOnly, appearance, sessions, font],
  );
  const contextRef = useRef(context);
  contextRef.current = context;

  const ready = initial !== undefined;
  const firstText = useRef(initial);
  firstText.current ??= initial;
  const textRef = useRef<ConsoleText>(undefined);
  // 글은 처음 읽은 것으로 한 번 만든다 — 그 뒤의 글은 뷰가 쥔다.
  useEffect(() => {
    const parent = host.current;
    if (!ready || !parent) return;
    const state = EditorState.create({
      doc: firstText.current ?? "",
      extensions: [history(), editorExtensions(contextRef.current, CONSOLE_FEATURES)],
    });
    const text = new ConsoleText(state.doc);
    textRef.current = text;
    const made = new EditorView({
      parent,
      state,
      dispatchTransactions: (transactions, v) => {
        const before = v.state.doc;
        v.update(transactions);
        if (v.state.doc === before) return;
        for (const tr of transactions) if (tr.docChanged) text.edit(tr.changes, tr.newDoc);
        latest.current.onChange(v.state.doc.toString());
      },
    });
    setView(made);
    latest.current.onView(made);
    return () => {
      latest.current.onView(undefined);
      setView(undefined);
      made.destroy();
    };
  }, [ready]);

  // 구문 세션은 Host 연결이 생기면 연다(뷰를 다시 만들지 않는다).
  useEffect(() => {
    const text = textRef.current;
    if (!view || !syntax || !text) return;
    setSessions({ sessions: createSyntaxSessions(syntax, text), text });
    return () => setSessions(undefined);
  }, [view, syntax]);

  // 외관 · 글꼴 · 구문 세션이 바뀌면 같은 뷰를 다시 구성한다(뷰 · undo 기록 · 커서가 남는다).
  useEffect(() => {
    view?.dispatch(reconfigureEditor(context, CONSOLE_FEATURES));
  }, [view, context]);

  return (
    <div
      ref={host}
      data-testid="db-console-editor"
      className="h-full min-h-0 overflow-hidden [&_.cm-editor]:h-full"
    />
  );
}
