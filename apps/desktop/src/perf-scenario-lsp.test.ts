// @vitest-environment happy-dom
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { RpcClient } from "@pitwall/rpc-client";
import { afterEach, expect, it } from "vitest";

import {
  editorStartingWith,
  MAIN_FIRST_LINE,
  SYMBOL_QUERY,
  symbolKeys,
  watchSymbolCalls,
} from "./perf-scenario-lsp";

afterEach(() => {
  document.body.replaceChildren();
});

it("times index.symbols.search round trips by query and leaves other calls alone", async () => {
  const proto = RpcClient.prototype as unknown as {
    call: (method: string, params?: unknown) => Promise<unknown>;
  };
  const original = proto.call;
  const answers: string[] = [];
  // 이 시험의 「Host」: 메서드 이름을 적고 곧바로 답한다.
  proto.call = function (method: string) {
    answers.push(method);
    return Promise.resolve({ method });
  };
  const stub = proto.call;
  try {
    let clock = 0;
    const calls = watchSymbolCalls(() => clock);
    const client = Object.create(RpcClient.prototype) as RpcClient;
    const pending = client.call("index.symbols.search", { projectId: "p", query: "ha" });
    clock = 12.5;
    await pending;
    await client.call("index.status", { projectId: "p" });
    expect(calls.rpc("ha")).toBe(12.5);
    // 질의가 없는 다른 메서드는 적지 않는다(적었다면 빈 질의 자리에 들었다).
    expect(calls.rpc("")).toBeUndefined();
    expect(answers).toEqual(["index.symbols.search", "index.status"]);
    calls.restore();
    expect(proto.call).toBe(stub);
  } finally {
    proto.call = original;
  }
});

it("finds the visible editor by its first line", () => {
  const mount = (text: string, hidden = false) => {
    const parent = document.createElement("div");
    if (hidden) parent.style.display = "none";
    document.body.append(parent);
    return new EditorView({ state: EditorState.create({ doc: text }), parent });
  };
  mount("// other file\n");
  const main = mount(`${MAIN_FIRST_LINE}\nfn main() {}\n`);
  // happy-dom 은 배치를 하지 않는다 — 보이는지는 getClientRects 로 본다.
  const rects = (n: number) => () => ({ length: n }) as DOMRectList;
  for (const el of document.querySelectorAll<HTMLElement>(".cm-editor"))
    el.getClientRects = rects(1);
  expect(editorStartingWith(document, MAIN_FIRST_LINE)).toBe(main);
  main.dom.getClientRects = rects(0);
  expect(editorStartingWith(document, MAIN_FIRST_LINE)).toBeUndefined();
});

/**
 * 가짜 시계: rAF 는 16.7 ms 마다(60 Hz), 시나리오의 쉼과 가짜 팝업의 지연은 그 시계로 돈다. 진짜 이벤트 루프(MutationObserver ·
 * MessageChannel)가 다 돈 뒤에 다음 가짜 사건으로 시계를 옮긴다.
 */
function fakeClock() {
  const FRAME = 16.7;
  let t = 1000.3;
  const queue: { at: number; seq: number; fn: () => void }[] = [];
  let seq = 0;
  const at = (when: number, fn: () => void) => {
    queue.push({ at: when, seq: seq++, fn });
    queue.sort((a, b) => a.at - b.at || a.seq - b.seq);
  };
  const env = {
    doc: document,
    now: () => t,
    epochNow: () => t,
    sleep: (d: number) => new Promise<void>((r) => at(t + d, r)),
    work: {
      frame: () =>
        new Promise<number>((r) => {
          // 프레임 머리에서 부르면 그다음 프레임이다(부동소수 나눗셈이 같은 머리를 돌려주지 않게).
          const next = (Math.floor(t / FRAME + 1e-9) + 1) * FRAME;
          at(next, () => r(next));
        }),
      start: () => undefined,
      take: () => [],
      restore: () => undefined,
    },
  };
  const settle = async () => {
    for (let i = 0; i < 5; i++) await new Promise((r) => setTimeout(r, 0));
  };
  async function drive<T>(p: Promise<T>): Promise<T> {
    let done = false;
    const out = p.finally(() => void (done = true));
    for (;;) {
      await settle();
      if (done) return out;
      const next = queue.shift();
      if (!next) throw new Error("the scenario waits on nothing");
      t = next.at;
      next.fn();
    }
  }
  return { env, at, drive, FRAME };
}

it("ends a symbol key at the frame its rows were painted in, not two frames after the start", async () => {
  const clock = fakeClock();
  const RESULT_MS = 10;
  // 가짜 팝업: ⌘O 로 입력 칸이 뜨고, 질의마다 10 ms 뒤에 그 질의의 결과 줄이 든다.
  document.body.addEventListener("keydown", (e) => {
    if (e.code !== "KeyO") return;
    const popup = document.createElement("div");
    popup.setAttribute("data-search-everywhere", "");
    const input = document.createElement("input");
    input.setAttribute("cmdk-input", "");
    input.addEventListener("input", () => {
      const q = input.value;
      clock.at(clock.env.now() + RESULT_MS, () => {
        const row = document.createElement("div");
        row.setAttribute("data-symbol-query", q);
        popup.replaceChildren(input, row);
      });
    });
    input.addEventListener("keydown", (k) => {
      if (k.key === "Escape") popup.remove();
    });
    popup.append(input);
    document.body.append(popup);
  });
  // 쉼을 한 프레임 안에 고르게 흩는다(시험이 늘 같은 값을 보도록 정해 둔 차례).
  let k = 0;
  const spread = () => ((k++ * 0.618) % 1) * 0.9 + 0.05;
  const result = await clock.drive(symbolKeys(clock.env, "symbols", { rpc: () => 4 }, spread));
  expect(result.keys.map((k) => k.query)).toEqual(
    [...SYMBOL_QUERY].map((_, i) => SYMBOL_QUERY.slice(0, i + 1)),
  );
  for (const key of result.keys) {
    // 끝은 결과가 든 때(10 ms) 뒤 첫 프레임 — 한 프레임보다 짧게 뒤다. 폴링 rAF 에 rAF 하나를 덧붙인 끝은 늘 정확히 한 프레임
    // 뒤이고(결과 줄을 본 rAF → 다음 rAF), 키 하나가 26.7 – 43.4 ms 다.
    expect(key.totalMs - key.seenMs).toBeLessThan(clock.FRAME - 0.05);
    expect(key.totalMs).toBeLessThan(RESULT_MS + clock.FRAME);
    expect(key.seenMs).toBeCloseTo(RESULT_MS, 5);
  }
  // 시작이 rAF 위상에서 떨어져 있다 — 값이 한 가지로 묶이지 않는다.
  expect(new Set(result.keys.map((k) => k.totalMs)).size).toBeGreaterThan(3);
});
