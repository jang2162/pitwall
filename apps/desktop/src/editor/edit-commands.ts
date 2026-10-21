// LSP 없이 되는 편집 명령(12-m1-plan M1-18, 10-ui §4.2 「코드」)과 찾기 · 줄로 이동의 계산. 모두 CodeMirror 상태만 보는 순수 함수다 —
// 키와 실행은 창의 키맵 액션(editor-ops.ts)이 붙인다. 읽기 전용 문서는 고치지 않는다(`false`).
//
// 줄 삭제 · 줄 이동은 CodeMirror 의 명령(`deleteLine` · `moveLineUp` · `moveLineDown`)을 그대로 쓴다. 줄 복제 · 줄 주석은
// IntelliJ 식으로 여기서 짠다:
//   - 줄 복제(⌘D): 선택이 없으면 줄을 아래로 복제하고 커서가 복제본으로 간다. 선택이 있으면 선택한 글자를 바로 뒤에 복제하고
//     복제본을 고른다.
//   - 줄 주석(⌘/): 언어의 줄 주석 기호(확장의 `line_comment`, `lang.list`)로. 고른 줄 중 빈 줄이 아닌 줄이 모두 주석이면 걷고,
//     아니면 그 줄들의 가장 얕은 들여쓰기 자리에 `기호 + 공백` 을 넣는다. 선택 없이 커서 하나면 다음 줄로 내려간다(IntelliJ 와
//     같다 — ⌘/ 를 거듭 눌러 여러 줄을 주석으로 만든다).

import { copyLineDown } from "@codemirror/commands";
import type { SearchQuery } from "@codemirror/search";
import {
  type ChangeSpec,
  EditorSelection,
  type EditorState,
  type StateCommand,
} from "@codemirror/state";

export { deleteLine, moveLineDown, moveLineUp } from "@codemirror/commands";

export const duplicateLine: StateCommand = ({ state, dispatch }) => {
  if (state.readOnly) return false;
  if (state.selection.ranges.every((r) => r.empty)) return copyLineDown({ state, dispatch });
  const spec = state.changeByRange((range) => {
    if (range.empty) {
      const line = state.doc.lineAt(range.head);
      const insert = line.text + state.lineBreak;
      return {
        changes: { from: line.from, insert },
        range: EditorSelection.cursor(range.head + insert.length),
      };
    }
    const text = state.sliceDoc(range.from, range.to);
    return {
      changes: { from: range.to, insert: text },
      range: EditorSelection.range(range.to, range.to + text.length),
    };
  });
  dispatch(state.update(spec, { scrollIntoView: true, userEvent: "input.copyline" }));
  return true;
};

/** 선택이 닿는 줄 번호(겹치지 않고 오름차순). 줄 첫머리에서 끝나는 선택은 그 줄을 넣지 않는다. */
function selectedLines(state: EditorState): number[] {
  const lines = new Set<number>();
  for (const r of state.selection.ranges) {
    const first = state.doc.lineAt(r.from).number;
    let last = state.doc.lineAt(r.to).number;
    if (!r.empty && last > first && state.doc.line(last).from === r.to) last--;
    for (let n = first; n <= last; n++) lines.add(n);
  }
  return [...lines].sort((a, b) => a - b);
}

/** 줄 앞 공백의 길이. */
function indentOf(text: string): number {
  return text.length - text.trimStart().length;
}

/** `token` 으로 고른 줄을 주석으로 만들거나 걷는다. `token` 이 비면 하지 않는다(그 언어의 줄 주석을 모른다). */
export function toggleLineComment(token: string): StateCommand {
  return ({ state, dispatch }) => {
    if (state.readOnly || token === "") return false;
    const all = selectedLines(state).map((n) => state.doc.line(n));
    const filled = all.filter((l) => l.text.trim() !== "");
    const lines = filled.length > 0 ? filled : all;
    const commented =
      filled.length > 0 && filled.every((l) => l.text.slice(indentOf(l.text)).startsWith(token));
    const changes: ChangeSpec[] = [];
    if (commented) {
      for (const l of lines) {
        const at = l.from + indentOf(l.text);
        const space = l.text.slice(indentOf(l.text) + token.length).startsWith(" ") ? 1 : 0;
        changes.push({ from: at, to: at + token.length + space });
      }
    } else {
      const indent = Math.min(...lines.map((l) => indentOf(l.text)));
      for (const l of lines) changes.push({ from: l.from + indent, insert: `${token} ` });
    }
    const set = state.changes(changes);
    let selection = state.selection.map(set);
    const main = state.selection.main;
    if (state.selection.ranges.length === 1 && main.empty) {
      // 다음 줄의 같은 열로(열은 주석을 넣기 전 기준). 마지막 줄이면 그 자리에 둔다.
      const doc = set.apply(state.doc);
      const line = state.doc.lineAt(main.head);
      if (line.number < doc.lines) {
        const next = doc.line(line.number + 1);
        selection = EditorSelection.single(
          next.from + Math.min(main.head - line.from, next.length),
        );
      }
    }
    dispatch(
      state.update({ changes: set, selection, scrollIntoView: true, userEvent: "comment.line" }),
    );
    return true;
  };
}

/** 줄로 이동(⌘L) 입력: `줄`, `줄:열`(`,` 도 받는다). 1부터 센다. 숫자가 아니면 `undefined`. */
export function parseLineColumn(text: string): { line: number; column: number } | undefined {
  const m = /^\s*(\d+)\s*(?:[:,]\s*(\d+)\s*)?$/.exec(text);
  if (!m) return undefined;
  const line = Number(m[1]);
  const column = m[2] === undefined ? 1 : Number(m[2]);
  if (line < 1 || column < 1) return undefined;
  return { line, column };
}

/** 찾기 막대가 세는 일치 수의 상한. 넘으면 「상한+」 로 보인다 — 일치가 아주 많은 문서에서 끝까지 세지 않게. */
export const MATCH_COUNT_CAP = 9999;

/** 한 번에 세는 조각의 크기(글자, 줄 끝까지 늘린다). 5 MiB 문서도 조각 하나는 몇 ms 다. */
export const MATCH_SCAN_CHUNK = 64 * 1024;

export interface MatchCount {
  /** 지금까지 센 일치 수(`capped` 면 상한). */
  total: number;
  /** 상한에서 멈췄다. */
  capped: boolean;
  /** 아직 세는 중이다 — `total` 은 지금까지 센 것이다. */
  counting: boolean;
  /** 주 선택이 정확히 덮는 일치의 차례(1부터). 없거나 아직 세지 않은 곳이면 `undefined`. */
  current?: number;
}

/**
 * 쿼리가 줄을 넘어 맞을 수 있는가. 그러면 줄 경계로 나눠 세지 않는다. 정규식은 `@codemirror/search` 가 여러 줄 커서를 고르는
 * 기준(`RegExpCursor`)과 같고, 리터럴은 줄바꿈(이스케이프 `\n` · `\r` 포함)이 든 것이다 — 넉넉하게 본다.
 */
function crossesLines(query: SearchQuery): boolean {
  const s = query.search;
  if (query.regexp) return /\\[sWDnr]|\n|\r|\[\^/.test(s);
  return /[\n\r]/.test(s) || (!query.literal && /\\[nr]/.test(s));
}

/**
 * 문서 하나 · 쿼리 하나의 일치를 센다. 찾기 막대는 문서나 쿼리가 바뀔 때만 새로 만들고, 선택만 바뀐 것은 센 목록에서 현재 일치를
 * 찾는다(`count`). 세기는 줄 경계로 자른 조각(`MATCH_SCAN_CHUNK`)마다 하고 `step` 이 시간 예산을 넘기면 멈춰 다음 `step` 에서
 * 잇는다 — 일치가 드문 큰 문서(5 MiB 리터럴 일치 0 이 수백 ms)에서 한 번에 끝까지 세며 창을 멈추지 않게. 줄을 넘는 쿼리는 자르지
 * 않고 한 번에 센다(상한은 본다). 틀린 정규식은 0 이다.
 */
export class MatchScan {
  private readonly from: number[] = [];
  private readonly to: number[] = [];
  /** 다음 조각이 시작하는 곳(줄 처음). */
  private pos = 0;
  private capped = false;
  /** 다 셌다(상한에서 멈춘 것 포함). */
  done: boolean;

  constructor(
    readonly state: EditorState,
    readonly query: SearchQuery,
    private readonly cap = MATCH_COUNT_CAP,
    private readonly chunk = MATCH_SCAN_CHUNK,
  ) {
    this.done = !query.valid;
  }

  /** `budgetMs` 가 지날 때까지 조각을 센다(시작한 조각은 끝낸다). 다 셌으면 참. */
  step(budgetMs: number): boolean {
    const start = performance.now();
    const doc = this.state.doc;
    const whole = crossesLines(this.query);
    while (!this.done) {
      const end = whole ? doc.length : doc.lineAt(Math.min(this.pos + this.chunk, doc.length)).to;
      const cursor = this.query.getCursor(this.state, this.pos, end);
      for (let next = cursor.next(); !next.done; next = cursor.next()) {
        if (this.from.length === this.cap) {
          this.capped = true;
          this.done = true;
          return true;
        }
        this.from.push(next.value.from);
        this.to.push(next.value.to);
      }
      if (end >= doc.length) this.done = true;
      else this.pos = end + 1;
      if (performance.now() - start >= budgetMs) break;
    }
    return this.done;
  }

  /** 지금까지 센 수와, `selection` 이 정확히 덮는 일치의 차례(위치로 이분 탐색). */
  count(selection: { from: number; to: number }): MatchCount {
    const total = this.from.length;
    let lo = 0;
    let hi = total;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (this.from[mid]! < selection.from) lo = mid + 1;
      else hi = mid;
    }
    const current =
      lo < total && this.from[lo] === selection.from && this.to[lo] === selection.to
        ? lo + 1
        : undefined;
    return { total, capped: this.capped, counting: !this.done, current };
  }
}

/** 문서 안의 `query` 일치 수를 한 번에 센다(시험 · 작은 문서용). 쿼리가 비었거나 틀린 정규식이면 0. */
export function countMatches(
  state: EditorState,
  query: SearchQuery,
  cap = MATCH_COUNT_CAP,
): MatchCount {
  const scan = new MatchScan(state, query, cap);
  scan.step(Infinity);
  return scan.count(state.selection.main);
}
