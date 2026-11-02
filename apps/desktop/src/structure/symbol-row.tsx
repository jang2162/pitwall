// 구조 툴 윈도우 · 파일 구조 팝업이 함께 쓰는 줄 조각: 종류 표지 · 이름 · 세부, 출처 표지, 고른 심볼로 가기.

import type { EditorWorkspace } from "../editor/editor-workspace";
import type { Outline, OutlineKind, OutlineSymbol } from "./outline";
import { cursorAt, type OutlineState } from "./outline-service";
import type { StructureStrings } from "./strings";

/** 종류 표지(글자 — 색만으로 가르지 않는다, 10-ui §6). 줄의 접근성 이름에는 종류 이름이 든다. */
const BADGES: Readonly<Record<OutlineKind, string>> = {
  module: "{}",
  class: "C",
  method: "m",
  property: "p",
  field: "f",
  constructor: "c",
  enum: "E",
  interface: "I",
  function: "ƒ",
  variable: "v",
  constant: "K",
  value: "·",
  enumMember: "e",
  struct: "S",
  event: "!",
  operator: "±",
  typeParameter: "T",
  trait: "Tr",
  typeAlias: "T=",
  macro: "#",
  impl: "⊃",
  symbol: "?",
};

/** 이름 · 세부는 서버가 준 글이다 — 글자 노드로만 그린다. */
export function SymbolLabel({
  symbol,
  strings: T,
  dim = false,
}: {
  symbol: OutlineSymbol;
  strings: StructureStrings;
  dim?: boolean;
}) {
  return (
    <>
      <span
        aria-hidden
        title={T.kinds[symbol.kind]}
        data-kind={symbol.kind}
        className="flex h-[14px] min-w-[14px] flex-none items-center justify-center rounded-sm border border-ui-border px-[2px] font-mono text-[9px] leading-none text-ui-muted-foreground"
      >
        {BADGES[symbol.kind]}
      </span>
      <span
        className={`flex-none truncate ${symbol.deprecated ? "line-through" : ""} ${dim ? "text-ui-muted-foreground" : ""}`}
        data-structure-name=""
      >
        {symbol.name}
      </span>
      {symbol.detail !== undefined && (
        <span className="min-w-0 truncate text-ui-muted-foreground">{symbol.detail}</span>
      )}
    </>
  );
}

/** 출처 표지(흐리게). 화면 읽기 프로그램에는 「심볼 출처: LSP · rust-analyzer」 로 읽힌다. */
export function SourceLabel({
  outline,
  strings: T,
}: {
  outline: Outline;
  strings: StructureStrings;
}) {
  return (
    <span className="text-[11px] text-ui-muted-foreground" title={T.sourceLabel}>
      <span className="sr-only">{T.sourceLabel}: </span>
      <span data-structure-source={outline.source}>
        {outline.source === "lsp" ? T.sourceLsp(outline.server ?? "") : T.sourceIndex}
      </span>
    </span>
  );
}

/** 심볼의 이름 자리로 위치로 가기(기록에 넣는다 — M4-09). */
export function goToSymbol(
  workspace: EditorWorkspace,
  state: OutlineState,
  symbol: OutlineSymbol,
): Promise<boolean> {
  if (state.path === undefined) return Promise.resolve(false);
  const view = state.panelId === undefined ? undefined : workspace.view(state.panelId);
  const at = view ? cursorAt(view.state.doc, symbol.selectionFrom) : { line: 1, column: 1 };
  return workspace.navigateTo(
    {
      path: state.path,
      ...(state.worktree !== undefined && { worktree: state.worktree }),
      ...at,
    },
    { record: true },
  );
}
