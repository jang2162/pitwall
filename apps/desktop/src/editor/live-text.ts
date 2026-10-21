// 살아 있는 텍스트(12-m1-plan M1-13): 문서 하나의 편집 중인 텍스트 · undo 기록 · 디스크와 같은 텍스트를 갖고, 그 문서를 보이는
// 뷰(탭)들 사이의 트랜잭션 허브가 된다. 뷰는 탭마다 따로(커서 · 선택 · 스크롤)지만 텍스트는 여기 한 벌이다.
//
// - 허브는 화면 없는 `EditorState` 다(텍스트 + `history()`). 뷰에서 사용자가 고친 트랜잭션은 뷰의 `dispatchTransactions` 가 이리로
//   보내고(`dispatch`), 허브는 그 변경을 자기에게 쌓고 나머지 뷰에 그대로 옮긴다. 옮긴 트랜잭션에는 `synced` 를 달아 다시 돌아오지
//   않게 한다. 그래서 모든 뷰의 텍스트는 늘 허브의 텍스트와 같다.
// - undo · redo 는 허브의 기록으로 한다(`undo(view)`). 결과의 변경은 모든 뷰에, 되돌린 선택은 명령을 낸 뷰에만 간다.
// - 뷰가 사라지는 동안(탭을 다른 그룹으로 옮김 · 뷰를 다시 만듦) 그 탭의 선택 · 스크롤은 `park` 로 여기 맡긴다. 그사이 다른 탭이
//   텍스트를 고쳐도 맡긴 위치를 함께 옮기고, 같은 탭의 다음 뷰가 `createState` 로 찾아간다.
// - 디스크에서 다시 읽은 텍스트는 `replace` 로 넣는다. 통째로 바꾸지 않고 차이만 바꿔 커서 · 선택이 제자리에 남는다. 이것도 undo
//   기록에 든다(따로 한 묶음).
//
// 줄바꿈: CodeMirror 의 텍스트는 줄 목록이라 줄바꿈 글자를 갖지 않는다. 파일의 줄바꿈(`fs.read` 의 `judgment.lineEnding`)이 하나면
// 그것을 기억해 저장할 때 줄 사이에 넣는다(`serialize`). 섞였으면(`mixed`) 줄을 `\n` 으로만 나눠 `\r` 을 글자로 남긴다 — 고치지
// 않은 줄의 바이트가 그대로 돌아간다. BOM 은 `fs.read` 가 떼어 주고 `fs.write` 의 `bom` 으로 다시 붙인다.

import { history, isolateHistory, redo, undo } from "@codemirror/commands";
import { diff } from "@codemirror/merge";
import {
  Annotation,
  ChangeSet,
  type EditorSelection,
  EditorState,
  type Extension,
  type StateEffect,
  type Text,
  Transaction,
} from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import type { LineEnding } from "@pitwall/rpc-client";

/** 허브가 뷰에 옮긴 트랜잭션. 뷰는 이것을 허브로 다시 보내지 않는다. */
const synced = Annotation.define<boolean>();

/** 디스크 텍스트를 넣을 때 차이 계산에 쓰는 시간 상한(ms). 넘으면 덜 촘촘한 차이로 바꾼다. */
const DIFF_TIMEOUT_MS = 200;

/** 줄바꿈 판정 → 저장할 때 줄 사이에 넣는 글자. 줄바꿈이 없던 파일에 줄을 더하면 `\n` 이다. */
function eolOf(lineEnding: LineEnding): string {
  if (lineEnding === "crlf") return "\r\n";
  if (lineEnding === "cr") return "\r";
  return "\n";
}

/** 섞인 줄바꿈이면 `\n` 으로만 나눈다(`\r` 이 글자로 남는다). 아니면 CodeMirror 기본(모든 줄바꿈으로 나눔). */
export function lineSeparatorOf(lineEnding: LineEnding): string | undefined {
  return lineEnding === "mixed" ? "\n" : undefined;
}

/** 뷰가 사라진 동안 맡긴 탭의 선택 · 스크롤. */
interface Parked {
  selection: EditorSelection;
  scroll: StateEffect<unknown> | undefined;
}

export interface LiveTextInit {
  /** `fs.read` 의 텍스트(BOM 을 뗀 것). */
  text: string;
  lineEnding: LineEnding;
  /** 파일 앞에 UTF-8 BOM 이 있었다. */
  bom: boolean;
}

export class LiveText {
  private hub: EditorState;
  /** 디스크와 같다고 아는 텍스트. 이것과 다르면 미저장이다. */
  private clean: Text;
  private wasDirty = false;
  private readonly views = new Set<EditorView>();
  private readonly parked = new Map<string, Parked>();
  private readonly listeners = new Set<() => void>();
  private eol: string;
  /** 저장할 때 BOM 을 붙인다. */
  bom: boolean;
  /** 이 텍스트의 줄 구분(`lineSeparatorOf`). 뷰의 상태도 같은 값으로 만든다. */
  readonly separator: string | undefined;

  constructor({ text, lineEnding, bom }: LiveTextInit) {
    this.separator = lineSeparatorOf(lineEnding);
    this.eol = eolOf(lineEnding);
    this.bom = bom;
    this.hub = EditorState.create({ doc: text, extensions: [this.base(), history()] });
    this.clean = this.hub.doc;
  }

  /** 허브와 뷰가 함께 가져야 하는 설정(줄 구분). */
  private base(): Extension {
    return this.separator === undefined ? [] : EditorState.lineSeparator.of(this.separator);
  }

  /** 지금 텍스트(편집 반영). */
  get text(): Text {
    return this.hub.doc;
  }

  /** 디스크와 다르다(저장하지 않은 편집이 있다). */
  get dirty(): boolean {
    return !this.hub.doc.eq(this.clean);
  }

  /** 미저장 여부가 바뀔 때 부른다. 해제 함수를 돌려준다. */
  onDirtyChange(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private changed() {
    const dirty = this.dirty;
    if (dirty === this.wasDirty) return;
    this.wasDirty = dirty;
    for (const listener of this.listeners) listener();
  }

  /** 지금 텍스트가 디스크에 있다(저장 · 다시 읽음). `text` 는 저장을 시작할 때의 텍스트다 — 그사이 고친 것은 미저장으로 남는다. */
  markClean(text: Text = this.hub.doc) {
    this.clean = text;
    this.changed();
  }

  /** 저장할 내용: 줄 사이에 파일의 줄바꿈을 넣은 텍스트(BOM 은 `fs.write` 의 `bom` 으로 붙는다). */
  serialize(text: Text = this.hub.doc): string {
    return text.sliceString(0, text.length, this.eol);
  }

  /**
   * 탭의 뷰가 쓸 상태. 그 탭이 맡겨 둔 선택이 있으면 그것으로 시작하고, 함께 맡긴 스크롤을 돌려준다 — 뷰를 만든 뒤
   * dispatch 한다.
   */
  createState(
    panelId: string,
    extensions: Extension,
  ): { state: EditorState; scroll: StateEffect<unknown> | undefined } {
    const parked = this.parked.get(panelId);
    this.parked.delete(panelId);
    const state = EditorState.create({
      doc: this.hub.doc,
      selection: parked?.selection,
      extensions: [this.base(), extensions],
    });
    return { state, scroll: parked?.scroll };
  }

  /** 뷰를 허브에 붙인다. 해제 함수는 그 탭의 선택 · 스크롤을 맡기고 뷰를 뗀다(뷰를 없애기 전에 부른다). */
  attach(panelId: string, view: EditorView, scroll: () => StateEffect<unknown> | undefined) {
    this.views.add(view);
    return () => {
      if (!this.views.delete(view)) return;
      this.parked.set(panelId, { selection: view.state.selection, scroll: scroll() });
    };
  }

  /** 탭이 닫혔다. 맡긴 선택을 버린다. */
  forget(panelId: string) {
    this.parked.delete(panelId);
  }

  /** 뷰의 `dispatchTransactions`. 사용자가 고친 것은 허브에 쌓고 다른 뷰에 옮긴다. */
  dispatch(transactions: readonly Transaction[], view: EditorView) {
    for (const tr of transactions) {
      if (!tr.docChanged || tr.annotation(synced) || !this.views.has(view)) {
        view.update([tr]);
        continue;
      }
      // 허브의 선택을 이 뷰의 것으로 맞춘 뒤 쌓는다 — undo 가 이 뷰의 선택을 되돌린다.
      let hub = this.hub;
      if (!hub.selection.eq(tr.startState.selection))
        hub = hub.update({
          selection: tr.startState.selection,
          annotations: Transaction.addToHistory.of(false),
        }).state;
      const userEvent = tr.annotation(Transaction.userEvent);
      const addToHistory = tr.annotation(Transaction.addToHistory);
      const isolate = tr.annotation(isolateHistory);
      this.hub = hub.update({
        changes: tr.changes,
        selection: tr.selection,
        ...(userEvent !== undefined && { userEvent }),
        annotations: [
          ...(addToHistory !== undefined ? [Transaction.addToHistory.of(addToHistory)] : []),
          ...(isolate !== undefined ? [isolateHistory.of(isolate)] : []),
        ],
      }).state;
      view.update([tr]);
      this.spread(tr.changes, view, userEvent);
    }
  }

  /** 허브에서 바뀐 것을 `except` 가 아닌 뷰에 옮기고 맡긴 위치를 옮긴다. */
  private spread(changes: ChangeSet, except: EditorView | undefined, userEvent?: string) {
    for (const [id, parked] of this.parked)
      this.parked.set(id, {
        selection: parked.selection.map(changes),
        scroll: parked.scroll?.map(changes),
      });
    for (const other of this.views)
      if (other !== except)
        other.dispatch({
          changes,
          annotations: synced.of(true),
          ...(userEvent !== undefined && { userEvent }),
        });
    this.changed();
  }

  /** 허브에서 만든 트랜잭션(undo · redo)을 반영한다. 선택은 명령을 낸 뷰에만. */
  private commit(tr: Transaction, origin: EditorView) {
    this.hub = tr.state;
    const userEvent = tr.annotation(Transaction.userEvent);
    if (this.views.has(origin))
      origin.dispatch({
        changes: tr.changes,
        selection: tr.selection,
        scrollIntoView: true,
        annotations: synced.of(true),
        ...(userEvent !== undefined && { userEvent }),
      });
    this.spread(tr.changes, origin, userEvent);
  }

  /** 이 문서의 undo 를 `view` 에서 했다. 되돌릴 것이 없으면 `false`. */
  undo(view: EditorView): boolean {
    return undo({ state: this.hub, dispatch: (tr) => this.commit(tr, view) });
  }

  redo(view: EditorView): boolean {
    return redo({ state: this.hub, dispatch: (tr) => this.commit(tr, view) });
  }

  /**
   * 디스크에서 읽은 텍스트로 바꾼다. 다른 곳만 바꾸므로 그 밖의 커서 · 선택 · 스크롤은 남는다. 바꾼 뒤 이 텍스트가 디스크와
   * 같은 것으로 본다. 섞인 줄바꿈 여부가 달라져 줄 구분을 바꿔야 하면 `false` 를 돌려주고 아무것도 하지 않는다 — 문서를 새로
   * 읽어야 한다.
   */
  replace({ text, lineEnding, bom }: LiveTextInit): boolean {
    if (lineSeparatorOf(lineEnding) !== this.separator) return false;
    const current = this.hub.doc.toString();
    // 비교는 허브와 같은 줄 구분으로 나눈 문자열끼리 한다.
    const next = EditorState.create({ doc: text, extensions: this.base() }).doc.toString();
    const changes = ChangeSet.of(
      diff(current, next, { timeout: DIFF_TIMEOUT_MS }).map((c) => ({
        from: c.fromA,
        to: c.toA,
        insert: next.slice(c.fromB, c.toB),
      })),
      current.length,
      this.hub.lineBreak,
    );
    this.eol = eolOf(lineEnding);
    this.bom = bom;
    if (!changes.empty) {
      this.hub = this.hub.update({
        changes,
        annotations: isolateHistory.of("full"),
      }).state;
      this.spread(changes, undefined);
    }
    this.markClean();
    return true;
  }
}
