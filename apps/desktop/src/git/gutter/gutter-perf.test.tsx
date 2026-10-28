// @vitest-environment happy-dom
// 거터 · blame 의 성능 완료 기준(13-m2-plan M2-17)을 헤드리스로 본다 — 실기(release 번들)는 사람과 나중에 잰다.
//
// 단언은 기기 부하에 흔들리지 않는 것만 한다(리뷰 F2 — 벽시계 최댓값 하나는 선점 · GC 한 번에 넘는다):
// - 「5,000줄 파일에서 초당 10자 30초 타이핑 중 50 ms 넘는 롱태스크가 없다」: 300자(10자 × 30초)를 한 자씩 넣는 동안 키마다 다시
//   견주기가 정확히 한 번 돌고, 그 비교가 앞뒤 같은 줄을 걷고 남은 가운데 40줄 이하(고치는 줄 20개의 두 쪽) · Myers 비용 40 이하 · 닻
//   나누기 없음이다 — 키마다의 일이 줄 수에 비례하는 걷기 한 번과 고친 줄 수만큼이라는 것. 키 사이의 100 ms 는 가짜 시계로 흘린다.
// - 「한 줄을 고치면 100 ms 안에 수정 표시」: 가짜 시계로 고친 뒤 `RECOMPUTE_MS`(< 100) 만 흘리면 거터 DOM 에 표시가 있고, 그 전에는
//   없다(재계산을 미루는 것이 그 지연뿐이다).
// 벽시계 수치(키마다 트랜잭션 → 재계산 → 뷰에 hunk 넣기 → 인라인 blame 의 시간, p50 · 최대)는 콘솔에만 남긴다 — 보고에 옮긴다. happy-dom
// 은 배치 · 그리기를 하지 않으므로 그만큼은 빠진다.

import { afterEach, expect, it, vi } from "vitest";

import { toggleAnnotations } from "./blame-extension";
import { gutterLink, gutterState } from "./gutter-extension";
import { RECOMPUTE_MS } from "./gutter-model";
import { FakeSource, type Harness, openEditor, textBlob } from "./testing";

const LINES = 5000;
const KEYS = 300;
/** 키 사이(초당 10자). */
const KEY_MS = 100;

let h: Harness | undefined;
afterEach(() => {
  vi.useRealTimers();
  h?.destroy();
  h = undefined;
});

function bigFile(): string {
  const out: string[] = [];
  for (let i = 0; i < LINES; i++)
    out.push(`    let value_${i} = compute(${i}, "some text on line ${i}");`);
  return `${out.join("\n")}\n`;
}

function bigSource(text: string): FakeSource {
  const source = new FakeSource();
  source.files.set("a.txt", textBlob(text));
  const hash = "c".repeat(40);
  source.blames.set("a.txt", [
    {
      file: { commit: "a".repeat(40), lines: LINES, engine: "git", moves: true },
      commits: [
        {
          hash,
          authorName: "ann",
          authorEmail: "a@x",
          authorTime: 1767225600,
          subject: "s",
          boundary: true,
        },
      ],
      ranges: [
        { line: 1, count: LINES, commit: hash, sourcePath: "a.txt", sourceLine: 1, previous: null },
      ],
    },
  ]);
  return source;
}

async function ready(harness: Harness) {
  await vi.waitFor(() => expect(harness.view.plugin(gutterLink)?.doc?.blame.kind).toBe("done"));
}

/** 마이크로태스크(뷰에 hunk 를 넣는 트랜잭션)까지 비운다. */
async function settle() {
  for (let i = 0; i < 4; i++) await Promise.resolve();
}

it(`types ${KEYS} characters into a ${LINES}-line file with one small comparison per key`, async () => {
  const text = bigFile();
  h = openEditor({ text, source: bigSource(text) });
  h.view.focus = () => {};
  vi.spyOn(h.view, "hasFocus", "get").mockReturnValue(true);
  await ready(h);
  toggleAnnotations(h.view);
  const doc = h.view.plugin(gutterLink)!.doc!;
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  const durations: number[] = [];
  for (let i = 0; i < KEYS; i++) {
    const before = doc.recomputes;
    // 문서 가운데 줄 끝에 한 자씩(그 줄과 한 칸씩 아래 줄을 오간다).
    const line = h.view.state.doc.line(LINES / 2 + (i % 20));
    const t0 = performance.now();
    h.view.dispatch({
      changes: { from: line.to, insert: "x" },
      selection: { anchor: line.to + 1 },
      userEvent: "input.type",
    });
    vi.advanceTimersByTime(KEY_MS);
    await settle();
    durations.push(performance.now() - t0);
    expect(doc.recomputes - before).toBe(1);
    // 고친 줄이 hunk 에 이어 붙는 동안 가운데는 그 줄들(두 쪽)뿐이다 — 고친 줄이 20개라 가장 많아도 40.
    expect(doc.lastStats.middle).toBeLessThanOrEqual(40);
    expect(doc.lastStats.cost).toBeLessThanOrEqual(40);
    expect(doc.lastStats.anchored).toBe(0);
  }
  durations.sort((a, b) => a - b);
  console.info(
    `[perf] gutter typing ${LINES} lines × ${KEYS} keys (wall clock, not asserted): p50 ${durations[Math.floor(KEYS / 2)]!.toFixed(2)} ms, p99 ${durations[Math.floor(KEYS * 0.99)]!.toFixed(2)} ms, max ${durations.at(-1)!.toFixed(2)} ms`,
  );
  expect(h.view.state.field(gutterState).hunks).toHaveLength(1);
});

it(`marks an edit of a ${LINES}-line file once RECOMPUTE_MS (< 100 ms) has passed`, async () => {
  expect(RECOMPUTE_MS).toBeLessThan(100);
  const text = bigFile();
  h = openEditor({ text, source: bigSource(text) });
  h.view.focus = () => {};
  await ready(h);
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  const marked = () => h!.view.dom.querySelector(".pw-git-mark-modified") !== null;
  const line = h.view.state.doc.line(1);
  const t0 = performance.now();
  h.view.dispatch({ changes: { from: line.to, insert: "!" }, userEvent: "input.type" });
  vi.advanceTimersByTime(RECOMPUTE_MS - 1);
  await settle();
  expect(marked()).toBe(false);
  vi.advanceTimersByTime(1);
  await settle();
  expect(marked()).toBe(true);
  console.info(
    `[perf] gutter marker after an edit of ${LINES} lines (wall clock without the ${RECOMPUTE_MS} ms delay, not asserted): ${(performance.now() - t0).toFixed(1)} ms`,
  );
});
