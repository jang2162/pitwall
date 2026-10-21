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

/** 찾기 막대가 세는 일치 수의 상한. 넘으면 「상한+」 로 보인다 — 큰 문서에서 글자를 칠 때마다 끝까지 세지 않게. */
export const MATCH_COUNT_CAP = 9999;

export interface MatchCount {
  /** 센 일치 수(`capped` 면 상한). */
  total: number;
  /** 상한에서 멈췄다. */
  capped: boolean;
  /** 주 선택이 정확히 덮는 일치의 차례(1부터). 없으면 `undefined`. */
  current?: number;
}

/** 문서 안의 `query` 일치 수. 쿼리가 비었거나 틀린 정규식이면 0. */
export function countMatches(
  state: EditorState,
  query: SearchQuery,
  cap = MATCH_COUNT_CAP,
): MatchCount {
  if (!query.valid) return { total: 0, capped: false };
  const sel = state.selection.main;
  const cursor = query.getCursor(state);
  let total = 0;
  let current: number | undefined;
  for (let next = cursor.next(); !next.done; next = cursor.next()) {
    if (total === cap) return { total, capped: true, current };
    total++;
    if (next.value.from === sel.from && next.value.to === sel.to) current = total;
  }
  return { total, capped: false, current };
}
