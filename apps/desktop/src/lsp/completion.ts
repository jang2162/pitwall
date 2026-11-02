// 자동완성(15-m4-plan M4-11, 10-ui §4.2 「코드 (LSP · 편집)」): `@codemirror/autocomplete` 의 목록 · 거르기 · 스니펫 위에 언어 서버
// 소스를 얹는다. 시그니처 도움말(./signature.ts)도 이 기능이 함께 펼친다.
//
// - 여는 때: 서버의 트리거 문자(`lsp.document.capabilities` 의 `completion.options.triggerCharacters` — 동적 등록이 바뀌면 능력 칸이
//   바뀌고 소스는 물을 때마다 지금 칸을 읽는다)와 낱말 입력에서 저절로, `⌃Space` · `⌥Space`(`editor.completion`)로 직접. 친 글자에서
//   요청까지 기다림이 없다(`activateOnTypingDelay: 0`). 직접 부를 때도 CodeMirror 의 `startCompletion`(50 ms 를 기다린다) 대신
//   입력과 같은 길로 연다(`triggerCompletion`).
// - 거르기: 목록이 완전하면(`isIncomplete` 아님) 받은 것을 창에서 거르고 다시 묻지 않는다(`validFor` — 낱말 글자). 모자라면
//   (`isIncomplete` · 200 개에서 잘림) 글자마다 다시 묻는다(`triggerKind: incomplete`). 거르기 · 순위는 CodeMirror 의 것이고 `filterText`
//   로 거르며 같은 점수는 `sortText` 순(글자 순 비교)이다.
// - 적용: `↩` 는 삽입 범위, `⇥` 는 바꾸기 범위(`InsertReplaceEdit` — 커서 뒤 낱말까지), 마우스로 고르면 삽입. `textEdit` 의 범위는 요청
//   때 문서 기준이라 그 뒤 친 글자만큼 늘인다. 스니펫(`insertTextFormat = 2`)은 CodeMirror 스니펫으로(탭 정지 · 자리 표시 — ./snippet.ts),
//   `additionalTextEdits`(자동 import)는 본 편집과 한 트랜잭션에 넣어 undo 한 번으로 함께 돌아간다. 서버가 resolve 로 자동 import 를
//   주면(TS) 받아들일 때 그 답을 기다린다(이미 받았으면 곧바로). 멀티 커서는 CodeMirror 기본 — 같은 낱말 범위의 커서에 같은 글을
//   넣는다(스니펫은 주 커서만).
// - 문서: 고른 항목만 `lsp.completion.resolve` 로 늦게 받아 옆 패널에(팝업 바탕의 `renderPopupBody` — HTML 끈 Markdown). 빠르게
//   옮기면 지난 resolve 는 요청 도우미가 취소한다(같은 `kind`).
// - 명령: 받아들인 항목의 `command` 는 `lsp.completion.execute`(Host 가 그 답으로 내준 · 서버가 알린 명령만 보낸다 — 결정 M4-D15).
//   서버가 그 명령 끝에 거는 `workspace/applyEdit` 은 M4-12 가 받는다 — 그 전에는 효과가 없다. 클라이언트 쪽 명령
//   (`editor.action.triggerParameterHints` · `triggerSuggest`)은 여기서 한다.
// - 한글 조합 중에는 목록을 열지 않고(소스가 묻지 않는다) `↩` 는 조합에 쓴다(항목을 고르지 않는다 — 10-ui 「한글 조합 중의 Enter」).
//   `Esc` 는 목록을 먼저 닫고 `preventDefault` 한다(팝업 바탕 · 멀티 커서의 Esc 보다 앞 — M4-14).
// - 미신뢰 프로젝트 · 서버 없음이면 능력 칸이 비어 소스가 조용히 아무것도 열지 않는다(결정 M4-D8).

import {
  acceptCompletion,
  autocompletion,
  closeCompletion,
  type Completion,
  type CompletionContext,
  type CompletionInfo,
  type CompletionResult,
  completionStatus,
  insertCompletionText,
  moveCompletionSelection,
  pickedCompletion,
  snippet,
} from "@codemirror/autocomplete";
import {
  ChangeSet,
  EditorSelection,
  type Extension,
  Prec,
  StateEffect,
  StateField,
  Transaction,
} from "@codemirror/state";
import { EditorView, keymap, ViewPlugin } from "@codemirror/view";
import type {
  LspCompletionItem,
  LspCompletionTrigger,
  LspMarkup,
  LspOffsetEdit,
} from "@pitwall/rpc-client";

import type { LspEditorFeature } from "./features";
import type { LspAt, LspDocument } from "./lsp-sessions";
import { renderPopupBody } from "./popup";
import { cycleSignature, signatureHelp, triggerSignatureHelp } from "./signature";
import { snippetTemplate } from "./snippet";

/** 낱말 글자(거르기 · 낱말 입력 판정). */
const WORD = /^[\p{L}\p{N}_$]*$/u;
const WORD_BEFORE = /[\p{L}\p{N}_$]+$/u;
const WORD_AFTER = /^[\p{L}\p{N}_$]+/u;
/** 받아들일 때 resolve 를 기다리는 한도. 넘으면 받은 것만으로 적용한다. */
export const RESOLVE_WAIT_MS = 1500;
/** 목록에 그리는 상한(Host 가 200 에서 자른다). */
const MAX_RENDERED = 200;

/** 서버의 완성 옵션(`completionProvider` · 동적 등록). */
interface CompletionOptions {
  triggerCharacters?: string[];
  resolveProvider?: boolean;
}

function completionOptions(doc: LspDocument): CompletionOptions | undefined {
  const cap = doc.capabilities.completion;
  if (!cap) return undefined;
  return typeof cap.options === "object" && cap.options !== null && !Array.isArray(cap.options)
    ? (cap.options as CompletionOptions)
    : {};
}

/** 뷰마다의 것. */
interface ViewState {
  /** 다음 질의가 직접 부른 것이다(`⌃Space`). */
  explicit: boolean;
  /** 앞 목록이 모자랐으면 그 시작 — 그 낱말을 이어 치면 `incomplete` 로 다시 묻는다. */
  incompleteFrom?: number;
  /** 받아들이는 중(resolve 를 기다림) — 두 번 넣지 않는다. */
  applying: boolean;
}

const views = new WeakMap<EditorView, ViewState>();

/** 목록 하나(한 번의 답)의 것. */
interface ListContext {
  readonly doc: LspDocument;
  readonly view: EditorView;
  /** 답을 받은 문서(`docId` · `version`). */
  readonly at: LspAt;
  /** 요청한 커서 · 목록의 시작 · 그때 문서 길이(편집 범위를 지금 문서로 옮길 때). */
  readonly requestPos: number;
  readonly from: number;
  readonly docLength: number;
  readonly resolveProvider: boolean;
  /** resolve 한 항목(받은 것 · 받는 중). */
  readonly resolved: Map<LspCompletionItem, Promise<Resolved | undefined>>;
  readonly done: Map<LspCompletionItem, Resolved>;
}

/** resolve 의 답과 그 편집이 잰 문서. */
interface Resolved {
  readonly item: LspCompletionItem;
  readonly version: number;
  readonly docLength: number;
}

// ---- 목록 ----------------------------------------------------------------------------------------------------------------------

/** 목록이 모자란가(「더 있음」 표지). 소스가 답을 돌려주기 전에 뷰에 알린다. */
const setMore = StateEffect.define<boolean>();
const moreField = StateField.define<boolean>({
  create: () => false,
  update: (value, tr) => {
    for (const e of tr.effects) if (e.is(setMore)) value = e.value;
    return value;
  },
});

/** `CompletionItemKind` → CodeMirror 아이콘 종류. */
const KIND_TYPES: Record<number, string> = {
  1: "text",
  2: "method",
  3: "function",
  4: "function",
  5: "property",
  6: "variable",
  7: "class",
  8: "interface",
  9: "namespace",
  10: "property",
  11: "constant",
  12: "constant",
  13: "enum",
  14: "keyword",
  15: "snippet",
  16: "constant",
  17: "file",
  18: "text",
  19: "folder",
  20: "enum",
  21: "constant",
  22: "class",
  23: "event",
  24: "keyword",
  25: "type",
};

function markupBody(markup: LspMarkup) {
  return markup.markdown
    ? ({ kind: "markdown", markdown: markup.value } as const)
    : ({ kind: "text", text: markup.value } as const);
}

/** 항목을 resolve 한다(같은 항목은 한 번). 서버가 resolve 를 하지 않거나 답을 버렸으면 `undefined`. */
function resolveItem(item: LspCompletionItem, list: ListContext): Promise<Resolved | undefined> {
  if (!list.resolveProvider) return Promise.resolve(undefined);
  const known = list.resolved.get(item);
  if (known) return known;
  const docLength = list.view.state.doc.length;
  let version = list.at.version;
  const asked = list.doc
    .call("completionResolve", "lsp.completion.resolve", (at) => {
      version = at.version;
      return { docId: at.docId, version: at.version, item: item.raw };
    })
    .then(
      (answer) => {
        if (!answer) {
          // 취소 · 지난 답 — 다음에 다시 묻게 잊는다.
          list.resolved.delete(item);
          return undefined;
        }
        const resolved: Resolved = { item: answer.item, version, docLength };
        list.done.set(item, resolved);
        return resolved;
      },
      (e: unknown) => {
        list.resolved.delete(item);
        console.warn("[lsp] completion resolve failed", e);
        return undefined;
      },
    );
  list.resolved.set(item, asked);
  return asked;
}

/** 고른 항목의 옆 패널: 세부 · 문서(resolve 로 늦게 받는다). 둘 다 없으면 패널이 없다. */
async function itemInfo(item: LspCompletionItem, list: ListContext): Promise<CompletionInfo> {
  const resolved = (await resolveItem(item, list))?.item;
  const detail = resolved?.detail ?? item.detail;
  const documentation = resolved?.documentation ?? item.documentation;
  if (!detail && !documentation) return null;
  const dom = document.createElement("div");
  dom.className = "pw-lsp-completion-info";
  if (detail) {
    const line = document.createElement("div");
    line.className = "pw-lsp-completion-detail";
    line.textContent = detail;
    dom.append(line);
  }
  if (documentation) {
    const body = document.createElement("div");
    body.className = "pw-lsp-completion-doc";
    renderPopupBody(body, markupBody(documentation), list.view);
    dom.append(body);
  }
  return dom;
}

/** 지운(deprecated) 항목 — 목록에서 줄을 긋는다. */
const deprecated = new WeakMap<Completion, boolean>();

/** 받아들이는 키: `↩` 는 삽입, `⇥` 는 바꾸기. 마우스로 고르면 삽입이다. */
let acceptMode: "insert" | "replace" = "insert";

function toOption(item: LspCompletionItem, list: ListContext): Completion {
  const filter = item.filterText ?? item.label;
  const display = item.label + (item.labelDetail ?? "");
  const option: Completion = {
    label: filter,
    ...(display !== filter && { displayLabel: display }),
    ...((item.labelDescription ?? item.detail) && {
      detail: (item.labelDescription ?? item.detail)!,
    }),
    type: KIND_TYPES[item.kind ?? 1] ?? "text",
    sortText: item.sortText ?? item.label,
    apply: (view, completion, from, to) => applyItem(view, completion, item, list, from, to),
  };
  if (list.resolveProvider || item.detail || item.documentation)
    option.info = () => itemInfo(item, list);
  deprecated.set(option, item.deprecated);
  return option;
}

/** 낱말이 시작되는 곳(편집 범위가 있으면 그 시작 — 거르기는 그 뒤의 글을 본다). */
function listStart(items: readonly LspCompletionItem[], pos: number, word: number): number {
  for (const item of items) {
    const start = item.edit?.insert.from;
    if (start !== undefined && start <= pos) return start;
  }
  return word;
}

function lspSource(doc: LspDocument) {
  return async (context: CompletionContext): Promise<CompletionResult | null> => {
    const view = context.view;
    const options = completionOptions(doc);
    if (!view || !options || view.compositionStarted) return null;
    const vs = viewState(view);
    const explicit = context.explicit || vs.explicit;
    vs.explicit = false;
    const pos = context.pos;
    const before = context.state.sliceDoc(Math.max(0, pos - 1), pos);
    const word = context.matchBefore(WORD_BEFORE);
    let trigger: LspCompletionTrigger;
    if (explicit) trigger = { kind: "invoked" };
    else if (
      vs.incompleteFrom !== undefined &&
      vs.incompleteFrom < pos &&
      WORD.test(context.state.sliceDoc(vs.incompleteFrom, pos))
    )
      trigger = { kind: "incomplete" };
    else if (before !== "" && options.triggerCharacters?.includes(before))
      trigger = { kind: "character", character: before };
    else if (word) trigger = { kind: "invoked" };
    else {
      vs.incompleteFrom = undefined;
      return null;
    }
    let at: LspAt | undefined;
    const docLength = context.state.doc.length;
    let list;
    try {
      list = await doc.call("completion", "lsp.completion", (a) => {
        at = a;
        return { docId: a.docId, version: a.version, offset: pos, trigger };
      });
    } catch (e) {
      console.warn("[lsp] completion failed", e);
      return null;
    }
    if (!list || !at || context.aborted) return null;
    const from = listStart(list.items, pos, word?.from ?? pos);
    const more = list.isIncomplete || list.truncated;
    vs.incompleteFrom = more ? from : undefined;
    const shared: ListContext = {
      doc,
      view,
      at,
      requestPos: pos,
      from,
      docLength,
      resolveProvider: options.resolveProvider === true,
      resolved: new Map(),
      done: new Map(),
    };
    if (view.state.field(moreField, false) !== more) view.dispatch({ effects: setMore.of(more) });
    return {
      from,
      options: list.items.map((item) => toOption(item, shared)),
      // 표시 이름(`이름(매개변수)`)이 거르는 글로 시작하면 일치 강조가 그대로 맞는다.
      getMatch: (c, matched) =>
        c.displayLabel === undefined || c.displayLabel.startsWith(c.label) ? (matched ?? []) : [],
      ...(!more && { validFor: WORD }),
    };
  };
}

function viewState(view: EditorView): ViewState {
  let vs = views.get(view);
  if (!vs) views.set(view, (vs = { explicit: false, applying: false }));
  return vs;
}

// ---- 적용 ----------------------------------------------------------------------------------------------------------------------

/** 요청 때 문서 기준의 편집을 지금 문서로: 목록 시작 앞은 그대로, 뒤는 그사이 친 글자만큼 민다. */
function shifted(
  edits: readonly LspOffsetEdit[],
  listFrom: number,
  delta: number,
): LspOffsetEdit[] {
  return edits.map((e) =>
    e.to <= listFrom ? e : { ...e, from: e.from + delta, to: e.to + delta },
  );
}

function applyItem(
  view: EditorView,
  completion: Completion,
  item: LspCompletionItem,
  list: ListContext,
  from: number,
  to: number,
) {
  const mode = acceptMode;
  const vs = viewState(view);
  if (vs.applying) return;
  const ready = list.done.get(item);
  // 자동 import 를 resolve 로 주는 서버(TS)면 그 답을 기다린다 — 이미 받았으면 곧바로.
  if (list.resolveProvider && item.additionalEdits.length === 0 && !ready) {
    vs.applying = true;
    const before = view.state.doc;
    const timeout = new Promise<undefined>((r) => setTimeout(() => r(undefined), RESOLVE_WAIT_MS));
    void Promise.race([resolveItem(item, list), timeout]).then((resolved) => {
      vs.applying = false;
      // 기다리는 사이 사람이 고쳤으면 넣지 않는다(위치가 그 뒤의 문서에 맞지 않는다).
      if (view.state.doc !== before) return;
      commit(view, completion, item, list, from, to, mode, resolved);
    });
    return;
  }
  commit(view, completion, item, list, from, to, mode, ready);
}

function commit(
  view: EditorView,
  completion: Completion,
  item: LspCompletionItem,
  list: ListContext,
  from: number,
  to: number,
  mode: "insert" | "replace",
  resolved: Resolved | undefined,
) {
  const { state } = view;
  const length = state.doc.length;
  const text = item.edit?.newText ?? item.insertText ?? item.label;
  // 본 편집 범위: 요청 때 범위를 지금 목록 자리(`from` — CodeMirror 가 편집에 따라 옮긴 것)와 커서(`to`)로.
  let start = from;
  let end = to;
  if (item.edit) {
    const range = mode === "replace" ? item.edit.replace : item.edit.insert;
    start = from + (range.from - list.from);
    end = to + Math.max(0, range.to - list.requestPos);
  } else if (mode === "replace") {
    end = to + (WORD_AFTER.exec(state.sliceDoc(to, state.doc.lineAt(to).to))?.[0].length ?? 0);
  }
  start = Math.max(0, Math.min(start, length));
  end = Math.max(start, Math.min(end, length));
  // 자동 import: 같은 문서의 편집만 온다(LSP — `additionalTextEdits` 는 그 문서의 것이다). 본 편집과 겹치거나 문서 밖이면 버린다.
  const extra = resolved?.item.additionalEdits.length
    ? shifted(resolved.item.additionalEdits, list.from, length - resolved.docLength)
    : shifted(item.additionalEdits, list.from, length - list.docLength);
  const accepted: LspOffsetEdit[] = [];
  for (const e of [...extra].sort((a, b) => a.from - b.from)) {
    const prev = accepted.at(-1);
    const overlapsMain = e.from < end && e.to > start;
    if (
      e.from < 0 ||
      e.to > length ||
      e.from > e.to ||
      overlapsMain ||
      (prev && e.from < prev.to)
    ) {
      console.warn("[lsp] dropped an additional completion edit that does not fit", e);
      continue;
    }
    accepted.push(e);
  }
  const additional = ChangeSet.of(
    accepted.map((e) => ({ from: e.from, to: e.to, insert: e.newText })),
    length,
  );
  let main: Transaction;
  if (item.snippet) {
    let captured: Transaction | undefined;
    snippet(snippetTemplate(text))(
      { state, dispatch: (tr) => void (captured = tr) },
      completion,
      start,
      end,
    );
    main = captured!;
  } else main = state.update(insertCompletionText(state, text, start, end));
  const after = additional.map(main.changes);
  const changes = main.changes.compose(after);
  const selection =
    main.selection?.map(after) ??
    EditorSelection.cursor(after.mapPos(main.changes.mapPos(end, 1), 1));
  view.dispatch({
    changes,
    selection,
    effects: StateEffect.mapEffects(main.effects, after),
    annotations: [pickedCompletion.of(completion), Transaction.userEvent.of("input.complete")],
    scrollIntoView: true,
  });
  const command = item.command ?? resolved?.item.command;
  if (command) runCommand(view, list, command, item.command ? list.at.version : resolved!.version);
}

/** 받아들인 항목의 명령. 클라이언트 쪽 명령은 여기서, 나머지는 Host 로(그 답으로 내준 · 서버가 알린 것만 간다). */
function runCommand(view: EditorView, list: ListContext, command: unknown, version: number) {
  const name = (command as { command?: unknown } | null)?.command;
  if (name === "editor.action.triggerParameterHints") {
    triggerSignatureHelp(view);
    return;
  }
  if (name === "editor.action.triggerSuggest") {
    queueMicrotask(() => triggerCompletion(view));
    return;
  }
  list.doc.services
    .call("lsp.completion.execute", { docId: list.at.docId, version, command: command as never })
    .catch((e: unknown) => console.warn("[lsp] completion command failed", name, e));
}

// ---- 키 · 액션 -----------------------------------------------------------------------------------------------------------------

const completionViews = new WeakSet<EditorView>();

/** 자동완성을 직접 연다(`⌃Space` · `⌥Space`). 이 뷰에 LSP 자동완성이 없으면 거짓. */
export function triggerCompletion(view: EditorView): boolean {
  if (!completionViews.has(view)) return false;
  closeCompletion(view);
  viewState(view).explicit = true;
  // 입력과 같은 길(`activateOnTypingDelay`)로 연다 — `startCompletion` 은 50 ms 를 기다린다.
  view.dispatch({ userEvent: "input.type" });
  return true;
}

function accept(mode: "insert" | "replace") {
  return (view: EditorView) => {
    // 한글 조합 중의 ↩ 는 조합을 확정한다 — 항목을 고르지 않는다.
    if (view.compositionStarted) return false;
    acceptMode = mode;
    try {
      return acceptCompletion(view);
    } finally {
      acceptMode = "insert";
    }
  };
}

const completionKeys = Prec.highest([
  keymap.of([
    { key: "ArrowDown", run: (v) => moveCompletionSelection(true)(v) || cycleSignature(v, 1) },
    { key: "ArrowUp", run: (v) => moveCompletionSelection(false)(v) || cycleSignature(v, -1) },
    { key: "PageDown", run: moveCompletionSelection(true, "page") },
    { key: "PageUp", run: moveCompletionSelection(false, "page") },
    { key: "Enter", run: accept("insert") },
    { key: "Tab", run: accept("replace") },
  ]),
  // 목록의 Esc 는 팝업 바탕(시그니처 등)보다 먼저 — 키맵보다 앞에 도는 DOM 처리기로 받는다(처리했다고 답해 `preventDefault`).
  EditorView.domEventHandlers({
    keydown(event, view) {
      if (event.key !== "Escape" || event.isComposing || view.compositionStarted) return false;
      return completionStatus(view.state) !== null && closeCompletion(view);
    },
  }),
]);

const registerView = ViewPlugin.define((view) => {
  completionViews.add(view);
  return { destroy: () => completionViews.delete(view) };
});

const completionTheme = EditorView.theme({
  ".cm-tooltip.cm-tooltip-autocomplete": {
    border: "1px solid var(--pw-popup-border)",
    backgroundColor: "var(--pw-popup-background)",
    color: "var(--pw-popup-foreground)",
    borderRadius: "6px",
    fontSize: "12px",
  },
  ".cm-tooltip.cm-tooltip-autocomplete > ul": {
    fontFamily: "var(--pw-editor-font-family, ui-monospace, monospace)",
    maxHeight: "min(320px, 45vh)",
  },
  ".cm-tooltip.cm-tooltip-autocomplete > ul > li[aria-selected]": {
    backgroundColor: "var(--pw-popup-selectionBackground)",
    color: "var(--pw-popup-foreground)",
  },
  ".cm-tooltip-autocomplete .cm-completionDetail": {
    color: "var(--pw-popup-mutedForeground)",
    fontStyle: "normal",
    marginLeft: "1.5em",
  },
  ".cm-tooltip-autocomplete .pw-lsp-completion-deprecated .cm-completionLabel": {
    textDecoration: "line-through",
  },
  // 「더 있음」: 서버가 목록이 모자란다고 했거나 200 개에서 잘렸다 — 더 치면 다시 묻는다.
  ".cm-tooltip-autocomplete.pw-lsp-completion-more > ul::after": {
    content: '"…"',
    display: "block",
    padding: "0 8px",
    color: "var(--pw-popup-mutedForeground)",
  },
  ".cm-tooltip.cm-completionInfo": {
    border: "1px solid var(--pw-popup-border)",
    backgroundColor: "var(--pw-popup-background)",
    color: "var(--pw-popup-foreground)",
    borderRadius: "6px",
    fontSize: "12px",
    maxWidth: "min(480px, 50vw)",
  },
  ".pw-lsp-completion-detail": {
    fontFamily: "var(--pw-editor-font-family, ui-monospace, monospace)",
    color: "var(--pw-popup-mutedForeground)",
  },
  ".cm-completionIcon-snippet:after": { content: '"⧉"' },
  ".cm-completionIcon-file:after": { content: '"🗎"' },
  ".cm-completionIcon-folder:after": { content: '"🗀"' },
  ".cm-completionIcon-event:after": { content: '"ϟ"' },
});

/** LSP 자동완성 · 시그니처 도움말(`LSP_FEATURES` 의 한 줄). */
export const completion: LspEditorFeature = {
  id: "completion",
  extension: (_context, doc): Extension => [
    moreField,
    autocompletion({
      override: [lspSource(doc)],
      activateOnTyping: true,
      activateOnTypingDelay: 0,
      defaultKeymap: false,
      maxRenderedOptions: MAX_RENDERED,
      icons: true,
      // 같은 점수의 순서는 서버의 `sortText`(글자 순 — 로캘 비교가 아니다).
      compareCompletions: (a, b) => {
        const x = a.sortText ?? a.label;
        const y = b.sortText ?? b.label;
        return x < y ? -1 : x > y ? 1 : 0;
      },
      optionClass: (c) => (deprecated.get(c) ? "pw-lsp-completion-deprecated" : ""),
      tooltipClass: (state) => (state.field(moreField, false) ? "pw-lsp-completion-more" : ""),
    }),
    completionKeys,
    registerView,
    completionTheme,
    signatureHelp(doc),
  ],
};
