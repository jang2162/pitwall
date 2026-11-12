// @vitest-environment happy-dom
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { RpcClient } from "@pitwall/rpc-client";
import { afterEach, expect, it } from "vitest";

import {
  countSources,
  domSource,
  editorStartingWith,
  MAIN_FIRST_LINE,
  shownThenPainted,
  SYMBOL_QUERY,
  symbolKeys,
  TYPING_KEYS,
  type TypingKey,
  typingBreakdown,
  typingKeyRows,
  typingKeySpec,
  typingPause,
  typingStartSpec,
  KEY_MS,
  PHASE_JITTER_MS,
  watchCallCost,
  watchFrameSources,
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

it("times lsp.workspaceSymbol round trips by query, a refusal included", async () => {
  const proto = RpcClient.prototype as unknown as {
    call: (method: string, params?: unknown) => Promise<unknown>;
  };
  const original = proto.call;
  // 미신뢰 프로젝트의 서버 쪽은 거절로 답한다 — 그것도 첫 키가 기다리는 답이다.
  proto.call = function (method: string) {
    return method === "lsp.workspaceSymbol"
      ? Promise.reject(new Error("no language servers"))
      : Promise.resolve({});
  };
  try {
    let clock = 0;
    const calls = watchSymbolCalls(() => clock);
    const client = Object.create(RpcClient.prototype) as RpcClient;
    const asked = client.call("lsp.workspaceSymbol", { projectId: "p", query: "h" } as never);
    clock = 7;
    await asked.catch(() => undefined);
    await client.call("index.symbols.search", { projectId: "p", query: "h" } as never);
    expect(calls.lsp("h")).toBe(7);
    expect(calls.rpc("h")).toBe(0);
    expect(calls.lsp("x")).toBeUndefined();
    calls.restore();
  } finally {
    proto.call = original;
  }
});

it("adds up the synchronous cost of the watched calls only, and empties on take", () => {
  const proto = RpcClient.prototype as unknown as {
    call: (method: string, params?: unknown) => Promise<unknown>;
  };
  const original = proto.call;
  let clock = 0;
  // 이 시험의 「Host」: 보내는 데 메서드마다 정해 둔 시간이 든다.
  const cost: Record<string, number> = { "lsp.document.change": 0.4, "fs.read": 3 };
  proto.call = function (method: string) {
    clock += cost[method] ?? 0;
    return Promise.resolve({});
  };
  try {
    const watch = watchCallCost(() => clock, ["lsp.document.change", "lsp.completion"]);
    const client = Object.create(RpcClient.prototype) as RpcClient;
    void client.call("lsp.document.change", {} as never);
    void client.call("lsp.document.change", {} as never);
    void client.call("fs.read", {} as never);
    void client.call("lsp.completion", {} as never);
    const taken = watch.take();
    expect(taken.ms.get("lsp.document.change")).toBeCloseTo(0.8, 5);
    expect(taken.count.get("lsp.document.change")).toBe(2);
    expect(taken.count.get("lsp.completion")).toBe(1);
    expect(taken.ms.has("fs.read")).toBe(false);
    expect(watch.take().count.size).toBe(0);
    watch.restore();
    expect(proto.call).not.toBe(original);
  } finally {
    proto.call = original;
  }
});

it("splits typing keys into those with the popup drawn and those without", () => {
  const key = (paintedMs: number, popupRows: number, dispatchMs = 1): TypingKey => ({
    ch: "a",
    dispatchMs,
    stateMs: dispatchMs / 2,
    layoutMs: 0,
    changeMs: 0.2,
    updateMs: dispatchMs,
    updates: 1,
    rafMs: 0.1,
    frameMs: paintedMs - 2,
    paintedMs,
    popupRows,
    completions: 0,
  });
  const keys = [key(10, 0), key(12, 0), key(17, 50, 4), key(16, 11, 3)];
  const b = typingBreakdown(keys);
  expect(b.all.keys).toBe(4);
  expect(b.popup.keys).toBe(2);
  expect(b.noPopup.keys).toBe(2);
  expect(b.popup.painted.max).toBe(17);
  expect(b.noPopup.painted.max).toBe(12);
  expect(b.popup.dispatch.max).toBe(4);
  // 프레임 대기는 dispatch 가 끝난 뒤부터, 그리기는 rAF 뒤부터다.
  expect(b.all.wait.max).toBe(14 - 3);
  expect(b.all.render.max).toBe(2);
});

it("keeps the per-key typing mark under the app's size limit for a mark", () => {
  // 앱은 16 KB 를 넘는 표지 하나를 버린다(src-tauri perf.rs `SCENARIO_DATA_MAX`) — 키마다 몇 자리 소수가 든 값으로 채운다.
  const key: TypingKey = {
    ch: "i",
    dispatchMs: 12.3,
    stateMs: 3.1,
    layoutMs: 2.4,
    changeMs: 0.4,
    updateMs: 11.8,
    updates: 2,
    rafMs: 1.2,
    frameMs: 15.6,
    paintedMs: 17.9,
    popupRows: 50,
    completions: 1,
  };
  const rows = typingKeyRows(Array.from({ length: TYPING_KEYS }, () => key));
  expect(rows.rows).toHaveLength(TYPING_KEYS);
  expect(rows.rows[0]![rows.columns.indexOf("paintedMs")]).toBe(17.9);
  expect(JSON.stringify(rows).length).toBeLessThan(16 * 1024);
});

it("spreads the pause between typing keys over one frame around the key interval", () => {
  // 고정 쉼(60 Hz 의 정확히 3프레임)이면 키가 앞 키의 프레임 위상에 묶인다 — 쉼이 한 프레임 폭에 고르게 흩어진다.
  const pauses = Array.from({ length: 1000 }, (_, i) => typingPause(() => (i + 0.5) / 1000));
  const mean = pauses.reduce((a, b) => a + b, 0) / pauses.length;
  expect(mean).toBeCloseTo(KEY_MS, 5);
  expect(Math.min(...pauses)).toBeLessThan(KEY_MS - PHASE_JITTER_MS / 2 + 0.1);
  expect(Math.max(...pauses)).toBeGreaterThan(KEY_MS + PHASE_JITTER_MS / 2 - 0.1);
  // 60 Hz 프레임 안의 위상(50 ms 뒤 시작이 프레임 머리에서 얼마나 떨어졌나)이 한 곳에 몰리지 않는다.
  const FRAME = 1000 / 60;
  const bins = new Set(pauses.map((p) => Math.floor(((p % FRAME) / FRAME) * 8)));
  expect(bins.size).toBe(8);
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

it("ends at the first frame after the DOM change and the task after it, wherever the change lands in the frame", async () => {
  const clock = fakeClock();
  // DOM 변경이 프레임 안 여러 자리(머리 가까이 · 가운데 · 끝 가까이 · 여러 프레임 뒤)에 든다 — 지연이 한 값이면 「값이 몇 종인가」
  // 로는 끝 표지가 프레임을 건너뛰었는지 가르지 못한다.
  for (const delay of [0.4, 5.3, 9.9, 16.4, 23.2, 41.0]) {
    const started = clock.env.now();
    const id = `mark-${delay}`;
    clock.at(started + delay, () => {
      const el = document.createElement("div");
      el.id = id;
      document.body.append(el);
    });
    const shown = await clock.drive(
      shownThenPainted(clock.env, id, () => document.getElementById(id), 1000),
    );
    expect(shown.seen).toBeCloseTo(started + delay, 5);
    // 하한: 끝은 seen 뒤 첫 rAF 경계를 지난다(프레임을 기다리지 않고 seen 에서 끝내면 여기서 걸린다).
    const firstFrame = (Math.floor(shown.seen / clock.FRAME + 1e-9) + 1) * clock.FRAME;
    expect(shown.frame).toBeCloseTo(firstFrame, 5);
    expect(shown.painted).toBeGreaterThanOrEqual(shown.frame);
    // 상한: 한 프레임 안이다(rAF 를 하나 덧붙이면 여기서 걸린다).
    expect(shown.painted - shown.seen).toBeLessThan(clock.FRAME);
    await clock.drive(clock.env.sleep(3.1));
  }
});

it("ends every frame row of the lsp and symbol scenarios through shownThenPainted", () => {
  const src = readFileSync(
    join(dirname(fileURLToPath(import.meta.url)), "perf-scenario-lsp.ts"),
    "utf8",
  );
  const body = (name: string) => {
    const from = src.search(new RegExp(`function ${name}\\b`));
    expect(from, name).toBeGreaterThanOrEqual(0);
    return src.slice(from, src.indexOf("\n}\n", from));
  };
  // 옛 끝 표지(`until` 폴링 뒤 `env.work.frame()`)로 되돌린 호출 자리가 없다 — rAF 는 `afterPaint` 한 곳에서만 건다.
  expect(src.match(/\.frame\(/g)?.length).toBe(1);
  expect(body("afterPaint")).toMatch(/\.frame\(/);
  for (const name of ["completion", "diagnostics", "definition", "problems", "symbolKeys"])
    expect(body(name), name).toContain("shownThenPainted(");
  // 타이핑 키 사이의 쉼은 위상에서 뗀 것이다(고정 `KEY_MS` 로 되돌리면 키가 프레임 위상에 묶인다).
  expect(body("typing")).toContain("env.sleep(typingPause())");
  expect(body("typing")).not.toMatch(/sleep\(KEY_MS\)/);
});

it("types where the editor shows it: the line is centred first and every key keeps the cursor in view", () => {
  // 하네스가 커서를 따라가지 않으면 줄 50 · 300자 줄의 끝이 보이는 영역 밖이라 자동완성 팝업이 키마다 「에디터 밖」 으로 숨겨진다.
  const state = EditorState.create({ doc: "x\n".repeat(60) });
  const start = state.doc.line(50).to;
  const first = state.update(typingStartSpec(start));
  expect(first.selection?.main.head).toBe(start);
  expect(first.effects).toHaveLength(1);
  const key = first.state.update(typingKeySpec(start, "a"));
  expect(key.scrollIntoView).toBe(true);
  expect(key.isUserEvent("input.type")).toBe(true);
  expect(key.state.doc.line(50).text).toBe("xa");
  expect(key.state.selection.main.head).toBe(start + 1);
});

it("names the screen part a change happened in and counts frame sources by kind and part", () => {
  document.body.innerHTML =
    '<div data-testid="status-lsp"><span>x</span></div><div class="cm-editor"><div class="cm-tooltip cm-tooltip-autocomplete"><ul><li>a</li></ul></div><div class="cm-content">t</div></div><p>o</p>';
  expect(domSource(document.querySelector("[data-testid=status-lsp] span")!.firstChild!)).toBe(
    "status-lsp",
  );
  expect(domSource(document.querySelector("li")!)).toBe("cm-tooltip-autocomplete");
  expect(domSource(document.querySelector(".cm-content")!)).toBe("cm-content");
  expect(domSource(document.querySelector("p")!)).toBe("other");
  expect(
    countSources([
      { kind: "dom", src: "a" },
      { kind: "raf", src: "b" },
      { kind: "dom", src: "a" },
    ]),
  ).toEqual({ "dom:a": 2, "raf:b": 1 });
  expect(
    Object.keys(
      countSources(
        [
          { kind: "dom", src: "a" },
          { kind: "dom", src: "b" },
        ],
        1,
      ),
    ),
  ).toHaveLength(1);
});

it("records DOM changes and rAF requests between keys and gives the window back", async () => {
  const raf = window.requestAnimationFrame;
  const sources = watchFrameSources(window, document, () => performance.now());
  document.body.innerHTML = '<div data-testid="status-lsp"></div>';
  window.requestAnimationFrame(() => {});
  await Promise.resolve();
  await new Promise((r) => setTimeout(r, 0));
  sources.restore();
  expect(window.requestAnimationFrame).toBe(raf);
  const kinds = sources.events.map((e) => e.kind);
  expect(kinds).toContain("dom");
  expect(kinds).toContain("raf");
});
