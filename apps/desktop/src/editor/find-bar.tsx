// 파일 안 찾기 · 바꾸기 막대(⌘F / ⌘R, 10-ui §4.2 「검색 · 이동」). 쿼리 · 일치 강조 · 다음 찾기 · 바꾸기는 `@codemirror/search` 이고,
// 이 막대는 그 상태(`getSearchQuery` · `findMode`)를 그리고 바꾸기만 한다 — 상태는 뷰(EditorState)에 있어 막대를 닫았다 열어도
// 마지막 쿼리가 남는다.
//
// 막대는 CodeMirror 의 위쪽 패널 자리(`findPanel`)에 텍스트 뷰어가 portal 로 그린다(text-viewer.tsx). 패널은 뷰가 바뀔 때마다
// `FIND_UPDATE` 를 자기 DOM 에 쏘고 막대가 그때 다시 그린다. 패널의 뿌리가 키맵 컨텍스트 `editorFind` 를 낸다 — ⌘F · ⌘R 는 막대
// 안에서도 돌고(찾기 칸을 다시 고른다 · 바꾸기 줄을 연다), 편집 키(⌘⌫ 등)는 돌지 않아 칸의 글자를 지운다.
//
// 칸의 키: ↩ 다음 · ⇧↩ 이전(바꿀 칸의 ↩ 는 바꾸기), Esc 닫고 에디터로. 한글을 조합하는 중의 ↩ · Esc 는 조합에 둔다(ui/ime.ts).
//
// 일치 수는 문서나 쿼리가 바뀔 때만 센다 — 커서만 움직이면 센 목록에서 현재 일치를 찾는다(`MatchScan`). 큰 문서는 조각마다
// `SCAN_BUDGET_MS` 만 세고 나머지는 다음 틱에 이어 세며, 그동안 「N+」 로 보인다.

import {
  closeSearchPanel,
  findNext,
  findPrevious,
  getSearchQuery,
  openSearchPanel,
  replaceAll,
  replaceNext,
  SearchQuery,
  setSearchQuery,
} from "@codemirror/search";
import type { EditorState } from "@codemirror/state";
import type { EditorView, Panel, ViewUpdate } from "@codemirror/view";
import {
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
  useEffect,
  useMemo,
  useReducer,
  useRef,
} from "react";

import { useProjectStrings } from "../layout/strings";
import { isComposingKey } from "../ui/ime";
import { type MatchCount, MatchScan } from "./edit-commands";
import { EDITOR_FIND, type FindMode, findMode, setFindMode } from "./extensions/operations";

/** 일치를 한 번에 세는 시간. 넘으면 나머지는 다음 틱에 센다 — 큰 문서에서 50 ms 넘는 멈춤을 만들지 않게. */
const SCAN_BUDGET_MS = 8;

/** 패널이 뷰의 바뀜을 막대에 알리는 DOM 이벤트. */
export const FIND_UPDATE = "pitwall-find-update";

/** 찾기 막대 자리. 뷰어가 `mount` 에서 그 DOM 에 막대를 그리고 `destroy` 에서 걷는다. */
export function findPanel(
  view: EditorView,
  shown: (find: { view: EditorView; dom: HTMLElement }) => void,
  hidden: (dom: HTMLElement) => void,
): Panel {
  const dom = document.createElement("div");
  dom.setAttribute("data-keymap-context", EDITOR_FIND);
  return {
    dom,
    top: true,
    mount: () => shown({ view, dom }),
    update: (update: ViewUpdate) => {
      if (update.docChanged || update.selectionSet || update.transactions.length > 0)
        dom.dispatchEvent(new Event(FIND_UPDATE));
    },
    destroy: () => hidden(dom),
  };
}

/** 찾기 막대를 연다(이미 열려 있으면 찾기 칸을 고른다). `replace` 면 바꾸기 줄도. */
export function openFind(view: EditorView, mode: FindMode) {
  view.dispatch({ effects: setFindMode.of(mode) });
  openSearchPanel(view);
}

function withQuery(view: EditorView, patch: Partial<ConstructorParameters<typeof SearchQuery>[0]>) {
  const q = getSearchQuery(view.state);
  view.dispatch({
    effects: setSearchQuery.of(
      new SearchQuery({
        search: q.search,
        caseSensitive: q.caseSensitive,
        literal: q.literal,
        regexp: q.regexp,
        replace: q.replace,
        wholeWord: q.wholeWord,
        ...patch,
      }),
    ),
  });
}

function Toggle({
  pressed,
  label,
  onToggle,
  children,
}: {
  pressed: boolean;
  label: string;
  onToggle(): void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      aria-pressed={pressed}
      aria-label={label}
      title={label}
      onClick={onToggle}
      className={`h-[20px] min-w-[22px] rounded px-1 font-mono text-[11px] ${pressed ? "bg-ui-active text-ui-foreground" : "text-ui-muted-foreground hover:bg-ui-hover"}`}
    >
      {children}
    </button>
  );
}

/** 일치 수. 쿼리가 비었으면 `undefined`. 문서 · 쿼리가 바뀌면 새로 세고, 다 셀 때까지 다음 틱마다 이어 센다. */
function useMatchCount(state: EditorState, query: SearchQuery): MatchCount | undefined {
  const { doc } = state;
  // 선택만 바뀐 상태로는 다시 세지 않는다 — 센 목록은 문서와 쿼리로만 정해진다. 상태는 세기에 문서와 함께 넘길 뿐이라 의존에
  // 넣지 않고 ref 로 읽는다.
  const latest = useRef(state);
  latest.current = state;
  const scan = useMemo(() => {
    if (query.search === "") return undefined;
    const s = new MatchScan(latest.current, query);
    s.step(SCAN_BUDGET_MS);
    return s;
  }, [doc, query]);
  const [, progressed] = useReducer((n: number) => n + 1, 0);
  useEffect(() => {
    if (!scan || scan.done) return;
    let timer: ReturnType<typeof setTimeout>;
    const next = () => {
      const done = scan.step(SCAN_BUDGET_MS);
      progressed();
      if (!done) timer = setTimeout(next, 0);
    };
    timer = setTimeout(next, 0);
    return () => clearTimeout(timer);
  }, [scan]);
  return scan?.count(state.selection.main);
}

const FIELD_CLASS =
  "h-[22px] w-[260px] min-w-0 rounded border border-ui-border bg-editor-background px-1.5 text-[12px] text-ui-foreground outline-none focus:border-ui-accent";
const BUTTON_CLASS =
  "h-[20px] rounded px-1.5 text-[12px] text-ui-foreground hover:bg-ui-hover disabled:text-ui-muted-foreground disabled:hover:bg-transparent";

export function FindBar({ view, host }: { view: EditorView; host: HTMLElement }) {
  const T = useProjectStrings().editor.find;
  const [, redraw] = useReducer((n: number) => n + 1, 0);
  useEffect(() => {
    host.addEventListener(FIND_UPDATE, redraw);
    return () => host.removeEventListener(FIND_UPDATE, redraw);
  }, [host]);
  const searchField = useRef<HTMLInputElement>(null);
  useEffect(() => {
    searchField.current?.focus();
    searchField.current?.select();
  }, []);

  const { state } = view;
  const query = getSearchQuery(state);
  const mode = state.field(findMode, false) ?? "find";
  const count = useMatchCount(state, query);
  const total = count && (count.capped || count.counting ? `${count.total}+` : count.total);
  const status =
    count === undefined
      ? ""
      : !query.valid
        ? T.invalid
        : count.total === 0
          ? count.counting
            ? T.counting
            : T.none
          : count.current !== undefined
            ? T.countCurrent(count.current, total ?? 0)
            : T.count(total ?? 0);

  const close = () => {
    closeSearchPanel(view);
    view.focus();
  };
  const keys =
    (enter: () => void, shiftEnter?: () => void) => (e: ReactKeyboardEvent<HTMLInputElement>) => {
      if (isComposingKey(e.nativeEvent)) return;
      if (e.key === "Enter") {
        e.preventDefault();
        (e.shiftKey && shiftEnter ? shiftEnter : enter)();
      } else if (e.key === "Escape") {
        e.preventDefault();
        close();
      }
    };
  const readOnly = state.readOnly;

  return (
    <div
      role="search"
      aria-label={T.label}
      className="flex flex-col gap-1 border-b border-ui-border bg-ui-panel px-2 py-1 text-[12px]"
    >
      <div className="flex items-center gap-1">
        <input
          ref={searchField}
          // `openSearchPanel` 이 열린 막대에서 이 칸을 다시 고른다.
          main-field="true"
          aria-label={T.search}
          placeholder={T.search}
          value={query.search}
          spellCheck={false}
          onChange={(e) => withQuery(view, { search: e.target.value })}
          onKeyDown={keys(
            () => findNext(view),
            () => findPrevious(view),
          )}
          className={FIELD_CLASS}
        />
        <Toggle
          pressed={query.caseSensitive}
          label={T.caseSensitive}
          onToggle={() => withQuery(view, { caseSensitive: !query.caseSensitive })}
        >
          Aa
        </Toggle>
        <Toggle
          pressed={query.wholeWord}
          label={T.wholeWord}
          onToggle={() => withQuery(view, { wholeWord: !query.wholeWord })}
        >
          W
        </Toggle>
        <Toggle
          pressed={query.regexp}
          label={T.regexp}
          onToggle={() => withQuery(view, { regexp: !query.regexp })}
        >
          .*
        </Toggle>
        <span
          role="status"
          data-testid="find-count"
          className={`min-w-[64px] px-1 text-[11px] ${query.valid || count === undefined ? "text-ui-muted-foreground" : "text-ui-error"}`}
        >
          {status}
        </span>
        <button
          type="button"
          aria-label={T.previous}
          title={T.previous}
          onClick={() => findPrevious(view)}
          className={BUTTON_CLASS}
        >
          ↑
        </button>
        <button
          type="button"
          aria-label={T.next}
          title={T.next}
          onClick={() => findNext(view)}
          className={BUTTON_CLASS}
        >
          ↓
        </button>
        <span className="flex-1" />
        <button
          type="button"
          aria-label={T.close}
          title={T.close}
          onClick={close}
          className={BUTTON_CLASS}
        >
          ×
        </button>
      </div>
      {mode === "replace" && (
        <div className="flex items-center gap-1">
          <input
            aria-label={T.replace}
            placeholder={T.replace}
            value={query.replace}
            spellCheck={false}
            disabled={readOnly}
            onChange={(e) => withQuery(view, { replace: e.target.value })}
            onKeyDown={keys(() => replaceNext(view))}
            className={FIELD_CLASS}
          />
          <button
            type="button"
            disabled={readOnly || !query.valid}
            onClick={() => replaceNext(view)}
            className={BUTTON_CLASS}
          >
            {T.replaceOne}
          </button>
          <button
            type="button"
            disabled={readOnly || !query.valid}
            onClick={() => replaceAll(view)}
            className={BUTTON_CLASS}
          >
            {T.replaceAll}
          </button>
        </div>
      )}
    </div>
  );
}
