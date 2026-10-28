// @vitest-environment happy-dom
// 거터 · blame 의 성능 완료 기준(13-m2-plan M2-17)을 헤드리스로 잰다 — 실기(release 번들)는 사람과 나중에 잰다.
//
// - 「5,000줄 파일에서 초당 10자 30초 타이핑 중 50 ms 넘는 롱태스크가 없다」: 300자(10자 × 30초)를 한 자씩 넣고, 한 자마다 그
//   키 입력이 만드는 일(트랜잭션 → 다시 견주기 → 뷰에 hunk 넣기 → 인라인 blame)을 한 덩어리로 재어 가장 긴 것을 본다. 키 사이의
//   100 ms 는 기다리지 않는다 — 다시 견주기가 키마다 한 번 돌게 바로 부른다(실제로는 `RECOMPUTE_MS` 안에 모인 키가 한 번에 돈다 —
//   이것이 더 나쁜 쪽이다). 주석 칸을 켠 채로 잰다.
// - 「한 줄을 고치면 100 ms 안에 수정 표시」: 같은 5,000줄 문서에서 고친 뒤 거터 DOM 에 표시가 보일 때까지.
// 잰 값은 콘솔에 남긴다(보고에 옮긴다). happy-dom 은 배치 · 그리기를 하지 않으므로 그만큼은 빠진다.

import { afterEach, expect, it, vi } from "vitest";

import { toggleAnnotations } from "./blame-extension";
import { gutterLink, gutterState } from "./gutter-extension";
import { FakeSource, type Harness, openEditor, textBlob } from "./testing";

const LINES = 5000;
const KEYS = 300;

let h: Harness | undefined;
afterEach(() => {
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

it(`types ${KEYS} characters into a ${LINES}-line file without a task over 50 ms`, async () => {
  const text = bigFile();
  h = openEditor({ text, source: bigSource(text) });
  h.view.focus = () => {};
  vi.spyOn(h.view, "hasFocus", "get").mockReturnValue(true);
  await ready(h);
  toggleAnnotations(h.view);
  const doc = h.view.plugin(gutterLink)!.doc!;
  const durations: number[] = [];
  for (let i = 0; i < KEYS; i++) {
    // 문서 가운데 줄 끝에 한 자씩(그 줄과 한 칸씩 아래 줄을 오간다).
    const line = h.view.state.doc.line(LINES / 2 + (i % 20));
    const t0 = performance.now();
    h.view.dispatch({
      changes: { from: line.to, insert: "x" },
      selection: { anchor: line.to + 1 },
      userEvent: "input.type",
    });
    doc.recompute();
    // 뷰에 hunk 를 넣는 트랜잭션(마이크로태스크)까지 같은 덩어리로 잰다.
    await new Promise<void>((resolve) => queueMicrotask(() => queueMicrotask(resolve)));
    durations.push(performance.now() - t0);
  }
  durations.sort((a, b) => a - b);
  const max = durations.at(-1)!;
  const p50 = durations[Math.floor(durations.length / 2)]!;
  console.info(
    `[perf] gutter typing ${LINES} lines × ${KEYS} keys: p50 ${p50.toFixed(2)} ms, max ${max.toFixed(2)} ms`,
  );
  expect(h.view.state.field(gutterState).hunks).toHaveLength(1);
  expect(max).toBeLessThan(50);
});

it(`marks an edit of a ${LINES}-line file within 100 ms`, async () => {
  const text = bigFile();
  h = openEditor({ text, source: bigSource(text) });
  h.view.focus = () => {};
  await ready(h);
  const line = h.view.state.doc.line(1);
  const t0 = performance.now();
  h.view.dispatch({ changes: { from: line.to, insert: "!" }, userEvent: "input.type" });
  await vi.waitFor(
    () => expect(h!.view.dom.querySelector(".pw-git-mark-modified")).not.toBeNull(),
    {
      interval: 1,
    },
  );
  const elapsed = performance.now() - t0;
  console.info(`[perf] gutter marker after an edit of ${LINES} lines: ${elapsed.toFixed(1)} ms`);
  expect(elapsed).toBeLessThan(100);
});
