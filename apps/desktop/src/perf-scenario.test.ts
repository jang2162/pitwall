// @vitest-environment happy-dom
import { afterEach, beforeEach, expect, it } from "vitest";

import { frameStats, installFrameWork, type FrameWork } from "./perf-frames";
import {
  FLING_FRAMES,
  runGitScenario,
  type ScenarioEnv,
  settledRows,
  STEADY_FRAMES,
  stretchUp,
  watchFirstHash,
} from "./perf-scenario";

// happy-dom 에는 canvas 2D 가 없다 — 시나리오가 감싸는 `fillText` 만 가진 가짜를 둔다.
class FakeContext {
  constructor(readonly canvas: HTMLCanvasElement) {}
  fillText(_text: string, _x: number, _y: number) {}
}
const globals = globalThis as unknown as { CanvasRenderingContext2D?: unknown };
let saved: unknown;
let listeners: AbortController;

beforeEach(() => {
  saved = globals.CanvasRenderingContext2D;
  globals.CanvasRenderingContext2D = FakeContext;
  document.body.innerHTML = "";
  listeners = new AbortController();
});

afterEach(() => {
  globals.CanvasRenderingContext2D = saved;
  listeners.abort();
});

/** 프레임마다 16 ms 가는 시계. */
function fakeWork(clock: { t: number }, onFrame?: () => void): FrameWork {
  return {
    frame: async () => {
      clock.t += 16;
      onFrame?.();
      return clock.t;
    },
    start() {},
    take: () => [1, 2, 3],
    restore() {},
  };
}

it("frame stats are nearest-rank percentiles and shares of the 16.7 ms budget", () => {
  const samples = [10, 16.7, 17, 20, 16, 15, 14, 13, 12, 11];
  expect(frameStats(samples)).toEqual({
    n: 10,
    p50: 14,
    p95: 20,
    p99: 20,
    max: 20,
    within16_7: 0.8,
    over18: 0.1,
  });
  expect(frameStats([]).n).toBe(0);
});

it("the frame work wrapper records the app's callbacks only between start and take", () => {
  const queued: FrameRequestCallback[] = [];
  const raw = (cb: FrameRequestCallback) => queued.push(cb);
  const win = { requestAnimationFrame: raw } as unknown as Window;
  let now = 0;
  const work = installFrameWork(win, () => now);
  win.requestAnimationFrame(() => (now += 5));
  queued.shift()!(0);
  work.start();
  win.requestAnimationFrame(() => (now += 3));
  queued.shift()!(0);
  expect(work.take()).toEqual([3]);
  expect(work.take()).toEqual([]);
  work.restore();
  expect(win.requestAnimationFrame).toBe(raw);
});

it("rows count as settled only after they stop growing for the settle time", async () => {
  const clock = { t: 0 };
  const list = document.createElement("div");
  list.setAttribute("aria-rowcount", "0");
  const steps = [0, 0, 200, 200, 5000, 5000, 5000, 5000, 5000, 5000, 5000, 5000];
  let frame = 0;
  const work = fakeWork(clock, () => {
    list.setAttribute("aria-rowcount", String(steps[Math.min(++frame, steps.length - 1)]));
  });
  const settled = await settledRows({ work, now: () => clock.t }, list, 100);
  expect(settled.rows).toBe(5000);
  // 5000 이 처음 보인 프레임(넷째)의 시각.
  expect(settled.at).toBe(64);
});

it("only a short hash drawn on the git log canvas counts as the first row", () => {
  const list = document.createElement("div");
  list.setAttribute("data-testid", "git-log-list");
  const inside = document.createElement("canvas");
  list.append(inside);
  const outside = document.createElement("canvas");
  document.body.append(list, outside);
  let now = 0;
  const hash = watchFirstHash(() => now);
  now = 1;
  new FakeContext(outside).fillText("deadbeef", 0, 0);
  now = 2;
  new FakeContext(inside).fillText("subject", 0, 0);
  now = 3;
  new FakeContext(inside).fillText("deadbeef", 0, 0);
  now = 4;
  new FakeContext(inside).fillText("0123abcd", 0, 0);
  expect(hash.first()).toEqual({ at: 3, text: "deadbeef" });
  hash.restore();
  expect(FakeContext.prototype.fillText.name).toBe("fillText");
});

/** 프로젝트 창 흉내: ⌘9 에 목록을 그리고, 더블클릭에 diff 를 그리고, 측정하는 쪽의 체크아웃에 상태바를 바꾼다. */
function fakeWindow(clock: { t: number }) {
  const doc = document;
  const project = doc.createElement("div");
  project.setAttribute("data-tool-window", "project");
  const branch = doc.createElement("span");
  branch.setAttribute("data-testid", "status-branch-name");
  branch.textContent = "main";
  doc.body.append(project, branch);
  let dblclickAt: { x: number; y: number } | undefined;
  const signal = listeners.signal;
  doc.body.addEventListener(
    "keydown",
    (e) => {
      if (!(e.metaKey && e.code === "Digit9")) return;
      const list = doc.createElement("div");
      list.setAttribute("data-testid", "git-log-list");
      list.setAttribute("aria-rowcount", "100002");
      const canvas = doc.createElement("canvas");
      list.append(canvas);
      list.addEventListener("dblclick", (m) => {
        dblclickAt = { x: m.clientX, y: m.clientY };
        clock.t += 40;
        const diff = doc.createElement("div");
        diff.setAttribute("data-git-file-diff", "perf/big.rs");
        diff.innerHTML = '<div class="pw-diff-line"></div><div class="pw-diff-line"></div>';
        doc.body.append(diff);
      });
      doc.body.append(list);
      clock.t += 100;
      new FakeContext(canvas).fillText("abcdef01", 0, 0);
    },
    { signal },
  );
  return { branch, dblclick: () => dblclickAt };
}

it("the git scenario reports each step in order with what it measured", async () => {
  const clock = { t: 1000 };
  const win = fakeWindow(clock);
  const events: [string, Record<string, unknown>][] = [];
  // 측정하는 쪽의 체크아웃은 몇 프레임 뒤에 상태바에 닿는다.
  let checkoutIn: number | undefined;
  let checkedOutAt = 0;
  const env: ScenarioEnv = {
    doc: document,
    work: fakeWork(clock, () => {
      if (checkoutIn !== undefined && --checkoutIn === 0) win.branch.textContent = "perf-checkout";
    }),
    now: () => clock.t,
    epochNow: () => 1_700_000_000_000 + clock.t,
    sleep: async () => {},
    settleMs: 50,
    report: async (event, data) => {
      events.push([event, data]);
      if (event === "checkout_ready") {
        checkoutIn = 3;
        checkedOutAt = clock.t;
      }
    },
  };
  await runGitScenario(env);
  expect(events.map(([e]) => e)).toEqual([
    "git_log_open",
    "git_log_rows",
    "git_log_scroll",
    "diff_open",
    "checkout_ready",
    "branch_shown",
    "scenario_done",
  ]);
  const data = Object.fromEntries(events);
  // ⌘9 에서 그리기까지 100 ms, 그 다음 프레임 16 ms.
  expect(data.git_log_open).toMatchObject({ openMs: 116, firstDrawMs: 100, firstHash: "abcdef01" });
  expect(data.git_log_rows).toMatchObject({ rows: 100002 });
  const scroll = data.git_log_scroll as {
    fling: { interval: { n: number; p95: number } };
    steady: { interval: { n: number } };
  };
  expect(scroll.fling.interval).toMatchObject({ n: FLING_FRAMES, p95: 16 });
  expect(scroll.steady.interval.n).toBe(STEADY_FRAMES);
  expect(data.diff_open).toEqual({ ms: 56, path: "perf/big.rs", paintedLines: 2 });
  // 첫 행(맨 위 반 행)을 더블클릭했다.
  expect(win.dblclick()?.y).toBe(11);
  expect(data.checkout_ready).toEqual({ branch: "main" });
  // 바뀐 프레임(셋째) 다음 프레임의 벽시계.
  expect(data.branch_shown).toEqual({
    branch: "perf-checkout",
    epochMs: 1_700_000_000_000 + checkedOutAt + 4 * 16,
  });
});

it("a step that never finishes ends the scenario with an error naming the step", async () => {
  const clock = { t: 0 };
  const project = document.createElement("div");
  project.setAttribute("data-tool-window", "project");
  const branch = document.createElement("span");
  branch.setAttribute("data-testid", "status-branch-name");
  branch.textContent = "main";
  document.body.append(project, branch);
  const events: [string, Record<string, unknown>][] = [];
  await runGitScenario({
    doc: document,
    work: fakeWork(clock),
    now: () => clock.t,
    epochNow: () => clock.t,
    sleep: async () => {},
    report: async (event, data) => void events.push([event, data]),
  });
  expect(events).toEqual([
    [
      "scenario_error",
      { step: "git log open", message: "timed out waiting for the first log row" },
    ],
  ]);
});

it("the tool window is stretched up with ⌃⌥⇧↑ until its height stops growing", async () => {
  const clock = { t: 0 };
  const list = document.createElement("div");
  list.tabIndex = 0;
  document.body.append(list);
  let height = 100;
  const keys: string[] = [];
  Object.defineProperty(list, "clientHeight", { get: () => height });
  list.addEventListener("keydown", (e) => {
    keys.push(`${e.ctrlKey}${e.altKey}${e.shiftKey}${e.key}`);
    height = Math.min(400, height + 100);
  });
  await stretchUp({ work: fakeWork(clock) }, list);
  expect(height).toBe(400);
  // 100 → 400 에 셋, 그대로인 것을 세 번 본다.
  expect(keys).toHaveLength(6);
  expect(new Set(keys)).toEqual(new Set(["truetruetrueArrowUp"]));
});
