// 멀티 커서 명령(15-m4-plan M4-14)의 상태 시험 — 뷰 없이 `EditorState` 로 본다. 창을 지나는 동작은 ../multi-cursor-window.test.tsx.
import { EditorSelection, EditorState, type Transaction } from "@codemirror/state";
import { describe, expect, it, vi } from "vitest";

import { MAX_OCCURRENCES, selectAllOccurrences } from "./multi-cursor";

function stateOf(doc: string, at: number): EditorState {
  return EditorState.create({
    doc,
    selection: EditorSelection.cursor(at),
    extensions: EditorState.allowMultipleSelections.of(true),
  });
}

function run(state: EditorState, onCapped?: (cap: number) => void): EditorState | undefined {
  let next: Transaction | undefined;
  selectAllOccurrences(onCapped)({ state, dispatch: (tr) => (next = tr) });
  return next?.state;
}

describe("selectAllOccurrences", () => {
  it("does not walk the matches inside other words when it selects a word", () => {
    // 커서는 낱말 `a` 위 — 낱말 속 `a`(banana 마다 셋)는 정규식 낱말 경계가 거른다.
    const state = stateOf("a " + "banana ".repeat(2000) + "a", 0);
    const wordAt = vi.spyOn(state, "wordAt");
    const next = run(state)!;
    expect(next.selection.ranges.map((r) => [r.from, r.to])).toEqual([
      [0, 1],
      [14002, 14003],
    ]);
    // 커서의 낱말 하나 + 맞은 둘. 낱말 속 일치 6000 개를 하나씩 보지 않는다.
    expect(wordAt.mock.calls.length).toBeLessThanOrEqual(3);
  });

  it("keeps the language's word check: a match next to a non-word character still counts", () => {
    const next = run(stateOf("foo.bar foo-baz foo_x (foo)", 0))!;
    expect(next.selection.ranges.map((r) => [r.from, r.to])).toEqual([
      [0, 3],
      [8, 11],
      [23, 26],
    ]);
  });

  it("selects the first matches up to the cap and says it stopped there", () => {
    const onCapped = vi.fn();
    const next = run(stateOf("foo\n".repeat(MAX_OCCURRENCES + 5), 0), onCapped)!;
    expect(next.selection.ranges).toHaveLength(MAX_OCCURRENCES);
    expect(onCapped).toHaveBeenCalledWith(MAX_OCCURRENCES);
    const exact = vi.fn();
    expect(run(stateOf("foo\n".repeat(MAX_OCCURRENCES), 0), exact)!.selection.ranges).toHaveLength(
      MAX_OCCURRENCES,
    );
    expect(exact).not.toHaveBeenCalled();
  });

  // 상한은 한 글자 넣기가 한 프레임 안팎이 되는 수다(multi-cursor.ts `MAX_OCCURRENCES`). 부하가 있는 기기에서도 흔들리지 않게
  // 7 번 중 가장 빠른 값을 본다.
  it("types one character at the cap of cursors within a frame", () => {
    const doc = Array.from({ length: 20_000 }, (_, i) => `let foo${i % 7} = foo + ${i};`).join(
      "\n",
    );
    let state = stateOf(doc, 0);
    const lines = Array.from({ length: MAX_OCCURRENCES }, (_, i) => state.doc.line(i + 1).from + 4);
    state = state.update({
      selection: EditorSelection.create(lines.map((p) => EditorSelection.cursor(p))),
    }).state;
    const times: number[] = [];
    for (let i = 0; i < 7; i++) {
      const start = performance.now();
      state.update(state.replaceSelection("Z"));
      times.push(performance.now() - start);
    }
    expect(Math.min(...times)).toBeLessThan(16);
  });
});
