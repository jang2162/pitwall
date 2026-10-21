// 상태바의 에디터 항목(12-m1-plan M1-18, 10-ui §2.4 「상태바」): 커서 줄:열 · 선택 길이, 줄바꿈, 인코딩, 언어. 모두 「활성 탭」
// (editor-workspace.tsx)의 것이고, 활성 탭이 텍스트 뷰어가 아니면 그리지 않는다. 자리는 status-bar.tsx 의 `STATUS_SLOTS`.
//
// - 커서: 누르면 줄로 이동(⌘L 과 같은 대화상자). 선택이 있으면 고른 글자 수와 줄바꿈 수를 덧붙인다(IntelliJ 와 같다).
// - 언어: 누르면 언어 고르기 팝업(F4-12). 고른 값은 문서 단위다(language.ts — 같은 파일의 다른 탭에도, 탭을 다 닫으면 잊는다).
//   맨 위 항목 「자동 판별로 되돌리기」 가 사람이 고른 값을 지운다. 언어 목록과 줄 주석은 `lang.list`.
//
// 활성 탭 · 커서가 바뀌면(도킹 영역의 활성 패널 바뀜 · 작업 공간의 `onCursor`) 창 단위 store 를 다시 채운다. 뷰가 처음 생길 때의
// `onCursor` 는 뷰를 등록하기 전에 오므로(extensions/operations.ts) 다음 마이크로태스크에 읽는다.

import { Command } from "cmdk";
import type { DockviewApi } from "dockview-react";
import { useEffect, useMemo, useState } from "react";
import { createStore, useStore } from "zustand";

import { useKeymapPopup } from "../keymap/keymap-provider";
import { useProjectStrings } from "../layout/strings";
import { useRpcQuery } from "../rpc/host";
import { STATUS_SLOTS, useStatusBarItem } from "../status-bar/status-bar";
import { SEARCH_ITEM_CLASS, SearchPopup } from "../ui/search-popup";
import { tabParams } from "./editor-tabs";
import { cursorOf, type EditorWorkspace, useEditorWorkspace } from "./editor-workspace";
import { PLAIN_TEXT, setDocLanguageOverride, useDocLanguage } from "./language";

interface ActiveEditor {
  /** 활성 탭의 문서 경로. 뷰가 없으면(읽는 중 · 이미지) `undefined`. */
  path?: string;
  line: number;
  column: number;
  /** 고른 글자 수(모든 선택 범위의 합). */
  chars: number;
  /** 고른 범위 안의 줄바꿈 수. */
  breaks: number;
}

const NONE: ActiveEditor = { line: 1, column: 1, chars: 0, breaks: 0 };

// 창 하나 = 웹뷰 하나라 모듈 store 가 창 단위다. `useEditorStatusItems` 를 부른 창이 채운다.
const activeEditor = createStore<ActiveEditor>(() => NONE);
/** 줄로 이동 대화상자를 여는 함수(창이 `useEditorStatusItems` 로 넘긴다). */
const gotoLine = createStore<{ open?: () => void }>(() => ({}));

function readActive(workspace: EditorWorkspace): ActiveEditor {
  const panel = workspace.activeTab();
  const view = panel && workspace.view(panel.id);
  if (!panel || !view) return NONE;
  const { state } = view;
  let chars = 0;
  let breaks = 0;
  for (const r of state.selection.ranges) {
    chars += r.to - r.from;
    breaks += state.doc.lineAt(r.to).number - state.doc.lineAt(r.from).number;
  }
  return { path: tabParams(panel).path, ...cursorOf(view), chars, breaks };
}

function useTextDoc(path: string | undefined) {
  const workspace = useEditorWorkspace();
  return useStore(workspace.documents.store, (s) => {
    const doc = path === undefined ? undefined : s.docs[path];
    return doc?.status === "text" ? doc : undefined;
  });
}

const ITEM_CLASS = "rounded px-1 hover:bg-ui-hover";

function CursorStatus() {
  const T = useProjectStrings().editor.status;
  const { path, line, column, chars, breaks } = useStore(activeEditor);
  const open = useStore(gotoLine, (s) => s.open);
  if (path === undefined) return null;
  const text = T.cursor(line, column) + (chars > 0 ? ` (${T.selection(chars, breaks)})` : "");
  return (
    <button
      type="button"
      data-testid="status-cursor"
      title={T.cursorHint}
      onClick={() => open?.()}
      className={ITEM_CLASS}
    >
      {text}
    </button>
  );
}

function LineEndingStatus() {
  const T = useProjectStrings().editor.status;
  const path = useStore(activeEditor, (s) => s.path);
  const ending = useTextDoc(path)?.judgment.lineEnding;
  if (ending === undefined || ending === "none") return null;
  return (
    <span data-testid="status-line-ending" title={T.lineEndingHint} className="px-1">
      {T.lineEnding(ending)}
    </span>
  );
}

function EncodingStatus() {
  const T = useProjectStrings().editor.status;
  const path = useStore(activeEditor, (s) => s.path);
  const judgment = useTextDoc(path)?.judgment;
  if (!judgment?.encoding) return null;
  const bom = judgment.bom !== null;
  return (
    <span data-testid="status-encoding" title={T.encodingHint} className="px-1">
      {judgment.lossy
        ? T.encodingLossy(judgment.encoding)
        : bom
          ? `${judgment.encoding} BOM`
          : judgment.encoding}
    </span>
  );
}

function LanguagePopup({ path, onClose }: { path: string; onClose(): void }) {
  const T = useProjectStrings().editor.language;
  useKeymapPopup(true);
  const list = useRpcQuery("lang.list", null, { staleTime: Infinity });
  const workspace = useEditorWorkspace();
  const detected = useTextDoc(path)?.language ?? PLAIN_TEXT;
  const label = (name: string) => (name === PLAIN_TEXT ? T.plainText : name);
  const [popular, rest] = useMemo(() => {
    const all = list.data?.languages ?? [];
    return [all.filter((l) => l.popular), all.filter((l) => !l.popular)];
  }, [list.data]);
  const choose = (lang: string | null) => {
    setDocLanguageOverride(path, lang);
    onClose();
    const panel = workspace.activeTab();
    if (panel) workspace.focus(panel.id);
  };
  const item = (name: string, group: string) => (
    <Command.Item
      key={`${group}:${name}`}
      value={`${group}:${name}`}
      keywords={[name, label(name)]}
      onSelect={() => choose(name)}
      className={SEARCH_ITEM_CLASS}
    >
      {label(name)}
    </Command.Item>
  );
  return (
    <SearchPopup
      open
      onOpenChange={(open) => !open && onClose()}
      label={T.label}
      placeholder={T.placeholder}
      empty={T.empty}
    >
      <Command.Item
        value="detect"
        keywords={[T.detect(label(detected))]}
        onSelect={() => choose(null)}
        className={SEARCH_ITEM_CLASS}
      >
        {T.detect(label(detected))}
      </Command.Item>
      {item(PLAIN_TEXT, "plain")}
      <Command.Group heading={T.popular}>
        {popular.map((l) => item(l.name, "popular"))}
      </Command.Group>
      <Command.Group heading={T.all}>{rest.map((l) => item(l.name, "all"))}</Command.Group>
    </SearchPopup>
  );
}

function LanguageStatus() {
  const path = useStore(activeEditor, (s) => s.path);
  return path === undefined ? null : <LanguageButton path={path} />;
}

function LanguageButton({ path }: { path: string }) {
  const T = useProjectStrings().editor;
  const language = useDocLanguage(path);
  const [choosing, setChoosing] = useState<string>();
  return (
    <>
      <button
        type="button"
        data-testid="status-language"
        title={T.status.languageHint}
        onClick={() => setChoosing(path)}
        className={ITEM_CLASS}
      >
        {language === PLAIN_TEXT ? T.language.plainText : language}
      </button>
      {choosing !== undefined && (
        <LanguagePopup path={choosing} onClose={() => setChoosing(undefined)} />
      )}
    </>
  );
}

/**
 * 창의 상태바에 에디터 항목을 둔다. `openGotoLine` 은 커서 항목을 누를 때 활성 탭의 줄로 이동을 연다. 도킹 영역(`api`)이 붙기
 * 전에는 비어 있다.
 */
export function useEditorStatusItems(
  workspace: EditorWorkspace,
  api: DockviewApi | undefined,
  openGotoLine: () => void,
) {
  useStatusBarItem({ id: "cursor", ...STATUS_SLOTS.cursor, component: CursorStatus });
  useStatusBarItem({ id: "lineEnding", ...STATUS_SLOTS.lineEnding, component: LineEndingStatus });
  useStatusBarItem({ id: "encoding", ...STATUS_SLOTS.encoding, component: EncodingStatus });
  useStatusBarItem({ id: "language", ...STATUS_SLOTS.language, component: LanguageStatus });
  useEffect(() => {
    gotoLine.setState({ open: openGotoLine });
    return () => gotoLine.setState({ open: undefined });
  }, [openGotoLine]);
  useEffect(() => {
    let live = true;
    const refresh = () => activeEditor.setState(readActive(workspace), true);
    const later = () =>
      queueMicrotask(() => {
        if (live) refresh();
      });
    refresh();
    const offCursor = workspace.onCursor(later);
    const active = api?.onDidActivePanelChange(later);
    const removed = api?.onDidRemovePanel(later);
    return () => {
      live = false;
      offCursor();
      active?.dispose();
      removed?.dispose();
      activeEditor.setState(NONE, true);
    };
  }, [workspace, api]);
}
