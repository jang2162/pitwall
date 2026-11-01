// 멀티 커서(15-m4-plan M4-14, 10-ui §4.2 「멀티 커서」 · §5.3, F4-9). 커서 여럿 · 다음 일치 · 사각(컬럼) 선택.
//
// - 키는 창의 키맵 액션이다(../multi-cursor-actions.ts, 기본 키는 extensions/keymap-default/keymaps/default.toml 「멀티 커서」 묶음):
//   다음 일치 추가 `⌃G` · 마지막 추가 취소 `⌃⇧G` · 모든 일치 `⌃⌘G` · `⌥⌥` 누른 채 `↑` `↓`(키맵 엔진의 `doubleHold`) · 컬럼 모드
//   `⌘⇧8` · `Esc` 커서 하나로. 이 모듈은 그 명령과 상태 · 마우스만 갖는다.
// - 마우스: `⌥+드래그` 사각 선택(`rectangularSelection` — `⌥` 를 누르면 십자 커서), `⌥⇧+클릭` 커서 추가. CodeMirror 기본의
//   `⌘+클릭`(Win/Linux `Ctrl+클릭`) 커서 추가는 걷는다 — 그 클릭은 「선언으로 이동」(10-ui §5.3, M4-08)의 자리다.
// - 컬럼 선택 모드: 켜 두면 `⇧+화살표` 와 드래그가 사각 선택이다. 상태는 뷰(탭)마다이고, 상태바 커서 항목이 「컬럼」 을 붙인다
//   (../editor-status.tsx). 사각형의 짧은 줄은 줄 끝 커서가 된다(CodeMirror `rectangularSelection` 과 같은 규칙).
// - 마지막 추가 취소: 다음 일치를 더하기 전의 선택을 쌓아 두고 되돌린다 — CodeMirror 의 `selectNextOccurrence` 는 새 범위를 주
//   범위로 만들지 않아(`addRange(…, false)`) 선택만 보고는 어느 것이 마지막인지 모른다. 쌓인 것이 없으면(⌥⇧+클릭 · ⌥⌥↓ 로 더한
//   커서) 주 범위를 뺀다 — 그 둘은 새 커서를 주 범위로 만든다. 범위가 하나면 아무것도 하지 않는다(첫 선택은 남는다).
// - 붙여넣기: 클립보드의 줄 수가 커서 수와 같으면 줄마다 나눠 넣는다(CodeMirror 기본 — 따로 두지 않는다).
// - 한글 조합: 커서가 여럿일 때 조합 중인 글자는 CodeMirror 가 주 커서의 DOM 바뀜을 다른 범위에 같이 넣는다(기본 동작).

import {
  closeSearchPanel,
  RegExpCursor,
  selectNextOccurrence as cmSelectNextOccurrence,
  SearchCursor,
  searchPanelOpen,
} from "@codemirror/search";
import {
  countColumn,
  EditorSelection,
  EditorState,
  findColumn,
  type SelectionRange,
  StateEffect,
  StateField,
  type StateCommand,
} from "@codemirror/state";
import {
  type Command,
  crosshairCursor,
  EditorView,
  keymap,
  rectangularSelection,
} from "@codemirror/view";

import type { EditorFeature } from ".";

/**
 * 모든 일치 선택(⌃⌘G)의 상한. 넘으면 앞에서부터 이만큼만 고르고 알린다. 커서가 많으면 글자 하나를 넣는 데도 커서 수의 제곱으로
 * 시간이 든다 — CodeMirror 의 편집 명령(입력 · 지우기 · 줄바꿈)이 범위마다 바뀜을 합성한다(`changeByRange`). 이 상한에서 한 글자
 * 넣기가 한 프레임(16 ms) 안팎이 되게 골랐다. 재는 법: multi-cursor.test.ts 의 「types one character at the cap of cursors within a frame」(상한 개수 커서에 `replaceSelection`).
 */
export const MAX_OCCURRENCES = 300;

// ── 다음 일치 · 마지막 추가 취소 ──────────────────────────────────────────────────────────────

const pushOccurrence = StateEffect.define<EditorSelection>();
const popOccurrence = StateEffect.define<null>();

/** 다음 일치를 더하기 전의 선택들(오래된 것부터). 문서가 바뀌면 따라 옮기고, 다른 길로 선택이 바뀌면 비운다. */
const occurrenceHistory = StateField.define<readonly EditorSelection[]>({
  create: () => [],
  update(stack, tr) {
    for (const e of tr.effects) {
      if (e.is(pushOccurrence)) return [...stack, e.value];
      if (e.is(popOccurrence)) return stack.slice(0, -1);
    }
    if (tr.docChanged) return stack.map((s) => s.map(tr.changes));
    return tr.selection ? [] : stack;
  },
});

/** `⌃G`: 다음 일치를 선택에 더한다. 선택이 없으면 커서의 낱말을 고른다(CodeMirror `selectNextOccurrence`). */
export const selectNextOccurrence: StateCommand = ({ state, dispatch }) => {
  const before = state.selection;
  return cmSelectNextOccurrence({
    state,
    dispatch: (tr) =>
      dispatch(
        state.update({
          selection: tr.selection,
          effects: [...tr.effects, pushOccurrence.of(before)],
          scrollIntoView: tr.scrollIntoView,
          userEvent: "select",
        }),
      ),
  });
};

/** `⌃⇧G`: 마지막으로 더한 것을 뺀다. 첫 선택은 남긴다(범위가 하나면 하지 않는다). */
export const unselectOccurrence: StateCommand = ({ state, dispatch }) => {
  const { ranges, mainIndex } = state.selection;
  if (ranges.length < 2) return false;
  const stack = state.field(occurrenceHistory, false) ?? [];
  const last = stack[stack.length - 1];
  if (last) {
    dispatch(
      state.update({
        selection: last,
        effects: popOccurrence.of(null),
        scrollIntoView: true,
        userEvent: "select",
      }),
    );
    return true;
  }
  const removed = ranges[mainIndex]!;
  const rest = ranges.filter((_, i) => i !== mainIndex);
  // 새 주 범위는 뺀 것에 가장 가까운 범위 — ⌥⌥↓ 로 더한 커서를 거꾸로 하나씩 뺀다.
  let main = 0;
  rest.forEach((r, i) => {
    if (Math.abs(r.head - removed.head) < Math.abs(rest[main]!.head - removed.head)) main = i;
  });
  dispatch(
    state.update({
      selection: EditorSelection.create(rest, main),
      scrollIntoView: true,
      userEvent: "select",
    }),
  );
  return true;
};

/** 정규식에서 글자 그대로 맞추게 한다. */
function escapeRegExp(text: string): string {
  return text.replace(/[\\^$.*+?()[\]{}|/]/g, "\\$&");
}

/**
 * `⌃⌘G`: 주 범위의 글자와 같은 곳을 모두 고른다. 주 범위가 비었으면 커서의 낱말이고 그때는 낱말 단위로만 맞춘다 — 낱말 경계를
 * 정규식으로 걸어 낱말 속 일치(`a` 를 찾을 때 모든 낱말의 `a`)는 훑지 않는다. 경계의 글자(문자 · 숫자 · `_`)와 CodeMirror 의
 * 낱말 판정(언어별 `wordChars`)이 다를 수 있어 맞은 것을 `wordAt` 으로 한 번 더 본다. 대소문자를 가린다(`⌃G` 와 같다).
 * 상한(`MAX_OCCURRENCES`)을 넘는 일치가 있으면 앞에서부터 상한까지만 고르고 `onCapped` 를 부른다.
 */
export function selectAllOccurrences(onCapped?: (cap: number) => void): StateCommand {
  return ({ state, dispatch }) => {
    const main = state.selection.main;
    let { from, to } = main;
    let wholeWord = false;
    if (main.empty) {
      const word = state.wordAt(main.head);
      if (!word) return false;
      ({ from, to } = word);
      wholeWord = true;
    }
    const text = state.sliceDoc(from, to);
    const cursor = wholeWord
      ? new RegExpCursor(state.doc, `(?<![\\p{L}\\p{N}_])${escapeRegExp(text)}(?![\\p{L}\\p{N}_])`)
      : new SearchCursor(state.doc, text);
    const ranges: SelectionRange[] = [];
    let mainIndex = 0;
    let capped = false;
    for (let cur = cursor.next(); !cur.done; cur = cursor.next()) {
      const { from: f, to: t } = cur.value;
      if (wholeWord) {
        const word = state.wordAt(f);
        if (!word || word.from !== f || word.to !== t) continue;
      }
      if (ranges.length >= MAX_OCCURRENCES) {
        capped = true;
        break;
      }
      if (f === from) mainIndex = ranges.length;
      ranges.push(EditorSelection.range(f, t));
    }
    if (ranges.length === 0) return false;
    dispatch(
      state.update({
        selection: EditorSelection.create(ranges, mainIndex),
        userEvent: "select.search.matches",
      }),
    );
    if (capped) onCapped?.(MAX_OCCURRENCES);
    return true;
  };
}

// ── 위 · 아래 줄에 커서 ─────────────────────────────────────────────────────────────────────

/**
 * `⌥⌥` 누른 채 `↑` · `↓`: 범위마다 한 줄 위 · 아래의 같은 글자 열(`Point.col`)에 커서를 더한다 — 이미 범위가 있는 자리는 건너뛰므로
 * 맨 위 · 맨 아래에 하나씩 더해진다. 새 맨 끝 커서가 주 범위가 된다. 짧은 줄은 줄 끝에 두되 열을 기억해(`goalColumn`) 다음 긴 줄에서 그 열로 돌아간다.
 * CodeMirror 의 `addCursorBelow` 는 화면 좌표로 옮기는데, 에디터는 줄바꿈(soft wrap)을 늘 끄므로 논리 줄 · 열과 같고 이쪽이
 * 그리기 전 상태에서도 맞다.
 */
function addCursorVertically(forward: boolean): StateCommand {
  return ({ state, dispatch }) => {
    const { ranges } = state.selection;
    const added: SelectionRange[] = [];
    for (const r of ranges) {
      const line = state.doc.lineAt(r.head);
      const n = line.number + (forward ? 1 : -1);
      if (n < 1 || n > state.doc.lines) continue;
      const col = r.goalColumn ?? countColumn(line.text, state.tabSize, r.head - line.from);
      const target = state.doc.line(n);
      const pos =
        target.from + Math.min(findColumn(target.text, col, state.tabSize), target.length);
      // 이미 있는 자리에는 더하지 않는다 — 겹친 범위를 합치면 CodeMirror 가 기억한 열을 버린다.
      if ([...ranges, ...added].some((o) => o.from <= pos && pos <= o.to)) continue;
      added.push(EditorSelection.cursor(pos, 0, undefined, col));
    }
    if (added.length === 0) return false;
    const main = ranges.length + (forward ? added.length - 1 : 0);
    dispatch(
      state.update({
        selection: EditorSelection.create([...ranges, ...added], main),
        scrollIntoView: true,
        userEvent: "select",
      }),
    );
    return true;
  };
}

export const addCursorAbove = addCursorVertically(false);
export const addCursorBelow = addCursorVertically(true);

// ── 컬럼 선택 모드 ─────────────────────────────────────────────────────────────────────────

interface Point {
  /** 1부터 센 줄. */
  readonly line: number;
  /**
   * 0부터 센 글자 열: 탭은 `tabSize` 칸, 그 밖의 글자는 넓은 글자(한글 · CJK)도 한 칸이다(CodeMirror `countColumn` — 마우스 사각
   * 선택 `rectangularSelection` 과 같은 규칙이라 키 · 마우스가 같은 사각형을 고른다). 등폭 글꼴의 화면 칸과는 넓은 글자에서 다르다.
   */
  readonly col: number;
}

interface ColumnState {
  readonly on: boolean;
  /** `⇧+화살표` 로 넓히는 중인 사각형. 다른 길로 선택 · 문서가 바뀌면 버린다. */
  readonly anchor?: Point;
  readonly head?: Point;
}

const setColumnMode = StateEffect.define<boolean>();
const setRectangle = StateEffect.define<{ anchor: Point; head: Point }>();

/** 컬럼 선택 모드(뷰마다). 상태바가 `isColumnMode` 로 읽는다. */
const columnState = StateField.define<ColumnState>({
  create: () => ({ on: false }),
  update(value, tr) {
    let next = value;
    for (const e of tr.effects) {
      if (e.is(setColumnMode)) next = { on: e.value };
      if (e.is(setRectangle)) return { on: next.on, ...e.value };
    }
    if (next.anchor && (tr.selection || tr.docChanged)) next = { on: next.on };
    return next;
  },
});

export function isColumnMode(state: EditorState): boolean {
  return state.field(columnState, false)?.on ?? false;
}

/** `⌘⇧8`: 컬럼 선택 모드를 켜고 끈다. */
export const toggleColumnMode: StateCommand = ({ state, dispatch }) => {
  dispatch(state.update({ effects: setColumnMode.of(!isColumnMode(state)) }));
  return true;
};

function pointAt(state: EditorState, pos: number): Point {
  const line = state.doc.lineAt(pos);
  return { line: line.number, col: countColumn(line.text, state.tabSize, pos - line.from) };
}

/** 두 꼭짓점 사이의 사각형을 줄마다 한 범위로. 열이 줄보다 길면 그 줄은 줄 끝 커서다. 머리는 `head` 쪽에 둔다. */
export function rectangle(state: EditorState, anchor: Point, head: Point): EditorSelection {
  const ranges: SelectionRange[] = [];
  const step = head.line >= anchor.line ? 1 : -1;
  const [left, right] = [Math.min(anchor.col, head.col), Math.max(anchor.col, head.col)];
  const backward = head.col < anchor.col;
  for (let n = anchor.line; ; n += step) {
    const line = state.doc.line(n);
    const start = findColumn(line.text, left, state.tabSize, true);
    if (start < 0) {
      ranges.push(EditorSelection.cursor(line.to));
    } else {
      const end = findColumn(line.text, right, state.tabSize);
      ranges.push(
        backward
          ? EditorSelection.range(line.from + end, line.from + start)
          : EditorSelection.range(line.from + start, line.from + end),
      );
    }
    if (n === head.line) break;
  }
  // 범위는 문서 차례로 정렬되므로 주 범위(머리 줄)는 위로 넓혔으면 맨 앞, 아래로 넓혔으면 맨 끝이다.
  return EditorSelection.create(ranges, step === 1 ? ranges.length - 1 : 0);
}

type Direction = "up" | "down" | "left" | "right";

/** 컬럼 모드의 `⇧+화살표`: 사각형의 머리를 한 칸 옮긴다. 모드가 꺼져 있으면 보통 선택 키로 넘긴다. */
export function extendRectangle(direction: Direction): Command {
  return (view) => {
    const { state } = view;
    const column = state.field(columnState, false);
    if (!column?.on) return false;
    const main = state.selection.main;
    const anchor = column.anchor ?? pointAt(state, main.anchor);
    let { line, col } = column.head ?? pointAt(state, main.head);
    if (direction === "up") line = Math.max(1, line - 1);
    if (direction === "down") line = Math.min(state.doc.lines, line + 1);
    if (direction === "left") col = Math.max(0, col - 1);
    if (direction === "right") {
      // 사각형이 걸친 줄 가운데 가장 긴 줄의 끝까지만 — 줄 밖의 빈 칸(가상 공간)은 그리지 않는다.
      let widest = 0;
      for (let n = Math.min(anchor.line, line); n <= Math.max(anchor.line, line); n++) {
        widest = Math.max(widest, countColumn(state.doc.line(n).text, state.tabSize));
      }
      col = Math.min(col + 1, Math.max(widest, col));
    }
    const head = { line, col };
    view.dispatch({
      selection: rectangle(state, anchor, head),
      effects: [setRectangle.of({ anchor, head }), EditorView.scrollIntoView(lineEnd(state, head))],
      userEvent: "select",
    });
    return true;
  };
}

function lineEnd(state: EditorState, point: Point): number {
  const line = state.doc.line(point.line);
  const at = findColumn(line.text, point.col, state.tabSize);
  return line.from + Math.min(at, line.length);
}

// ── 커서 하나로 ──────────────────────────────────────────────────────────────────────────

/**
 * `Esc`: 찾기 막대가 열려 있으면 그것을 먼저 닫고, 아니면 주 커서 하나만 남긴다(선택이 하나면 그 머리로 접는다 — IntelliJ 와
 * 같다). 할 것이 없으면 거짓. 다른 처리기(찾기 칸 · 에디터 팝업)가 먼저 받은 Esc 에서는 돌지 않는다(../multi-cursor-actions.ts
 * 머리 주석).
 */
export const singleCursor: Command = (view) => {
  if (searchPanelOpen(view.state)) return closeSearchPanel(view);
  const { selection } = view.state;
  if (selection.ranges.length > 1) {
    view.dispatch({ selection: EditorSelection.create([selection.main]), userEvent: "select" });
    return true;
  }
  if (!selection.main.empty) {
    view.dispatch({ selection: EditorSelection.cursor(selection.main.head), userEvent: "select" });
    return true;
  }
  return false;
};

/** `Esc` 가 할 것이 있는가(찾기 막대가 열렸거나 커서 · 선택이 있다). */
export function hasExtraCursors(state: EditorState): boolean {
  return searchPanelOpen(state) || state.selection.ranges.length > 1 || !state.selection.main.empty;
}

// ── 확장 ────────────────────────────────────────────────────────────────────────────────

/** 이 마우스 누름이 컬럼 모드인 에디터의 것인가. */
function inColumnMode(event: MouseEvent): boolean {
  const dom = event.target instanceof Element ? event.target.closest(".cm-editor") : null;
  const view = dom instanceof HTMLElement ? EditorView.findFromDOM(dom) : null;
  return view ? isColumnMode(view.state) : false;
}

export const multiCursor: EditorFeature = {
  id: "multiCursor",
  extension: (c) => [
    EditorState.allowMultipleSelections.of(true),
    occurrenceHistory,
    columnState,
    // ⌥⇧+클릭 은 커서 추가라 사각 선택에서 뺀다.
    rectangularSelection({
      eventFilter: (e) => e.button === 0 && ((e.altKey && !e.shiftKey) || inColumnMode(e)),
    }),
    crosshairCursor(),
    EditorView.clickAddsSelectionRange.of((e) => e.altKey && e.shiftKey),
    keymap.of([
      { key: "Shift-ArrowUp", run: extendRectangle("up") },
      { key: "Shift-ArrowDown", run: extendRectangle("down") },
      { key: "Shift-ArrowLeft", run: extendRectangle("left") },
      { key: "Shift-ArrowRight", run: extendRectangle("right") },
    ]),
    // 컬럼 모드를 켜고 끄면 상태바(커서 항목의 「컬럼」)가 다시 읽는다 — 선택은 그대로라 `cursorReport` 가 알리지 않는다.
    EditorView.updateListener.of((u) => {
      if (u.selectionSet) return;
      if (isColumnMode(u.startState) !== isColumnMode(u.state)) {
        const head = u.state.selection.main.head;
        const line = u.state.doc.lineAt(head);
        c.onCursor({ line: line.number, column: head - line.from + 1 });
      }
    }),
  ],
};
