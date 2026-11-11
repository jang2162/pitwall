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

  // 재리뷰 N1: 앞에서부터 자르면 커서 자리(351행)의 일치가 빠지고 주 범위가 1행으로 가, 화면 밖 300곳만 고쳐졌다.
  it.each([
    ["the caret's word", (doc: string) => stateOf(doc, 1400)],
    [
      "a selected text",
      (doc: string) =>
        EditorState.create({
          doc,
          selection: EditorSelection.range(1400, 1403),
          extensions: EditorState.allowMultipleSelections.of(true),
        }),
    ],
  ] as const)(
    "keeps the caret's match as the main one and fills the cap around it (%s)",
    (_, make) => {
      const onCapped = vi.fn();
      // `foo` 400 줄, 커서는 351행(1400) — 가까운 순으로 채우면 뒤쪽 끝(400행)에 닿고 나머지는 앞쪽에서 101행까지.
      const next = run(make("foo\n".repeat(400)), onCapped)!;
      const { ranges, main } = next.selection;
      expect(ranges).toHaveLength(MAX_OCCURRENCES);
      expect([main.from, main.to]).toEqual([1400, 1403]);
      expect(ranges[0]!.from).toBe(400);
      expect(ranges[ranges.length - 1]!.from).toBe(1596);
      expect(onCapped).toHaveBeenCalledWith(MAX_OCCURRENCES);
    },
  );

  it("fills both sides evenly when the caret is in the middle", () => {
    const next = run(stateOf("foo\n".repeat(1000), 500 * 4))!;
    const { ranges, main } = next.selection;
    expect(ranges).toHaveLength(MAX_OCCURRENCES);
    expect(main.from).toBe(2000);
    expect(ranges[0]!.from).toBe((500 - 150) * 4);
    expect(ranges[ranges.length - 1]!.from).toBe((500 + 149) * 4);
  });

  it("does not report the cap when every match fits", () => {
    const exact = vi.fn();
    expect(run(stateOf("foo\n".repeat(MAX_OCCURRENCES), 0), exact)!.selection.ranges).toHaveLength(
      MAX_OCCURRENCES,
    );
    expect(exact).not.toHaveBeenCalled();
  });

  // 상한은 한 글자 넣기가 한 프레임 안팎이 되는 수다(multi-cursor.ts `MAX_OCCURRENCES`). 부하가 있는 기기에서도 흔들리지 않게
  // 7 번 중 가장 빠른 값을 본다.
  // CI 에서는 한도가 32 ms 다: 공유 러너는 개발 기기보다 느려 「한 프레임」 근거를 잴 수 없고(ubuntu 러너 실측 20.3 · 16.96 ms —
  // 2026-11-11, run 42916693969), CI 의 한도는 회귀가 2배를 넘는지만 잡는다. 로컬은 16 ms 그대로다.
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
    expect(Math.min(...times)).toBeLessThan(process.env.CI ? 32 : 16);
  });
});
