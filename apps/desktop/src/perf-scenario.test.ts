// @vitest-environment happy-dom
import { EditorView } from "@codemirror/view";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { ROW } from "./git/commit/commit-tool-window";
import { frameStats, installFrameWork, type FrameWork } from "./perf-frames";
import {
  CHANGED_FILES,
  COMMIT_FLING_FRAMES,
  COMMIT_ROW,
  EDIT_LINE,
  FLING_FRAMES,
  hideGitLog,
  KEY_MS,
  runGitScenario,
  type ScenarioEnv,
  settledRows,
  STAGE_PATH,
  STEADY_FRAMES,
  stretchUp,
  TYPING_FIRST_LINE,
  TYPING_KEYS,
  TYPING_LINE,
  typingView,
  treeRow,
  watchBlocks,
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

let views: EditorView[] = [];

afterEach(() => {
  globals.CanvasRenderingContext2D = saved;
  listeners.abort();
  for (const v of views) v.destroy();
  views = [];
  vi.useRealTimers();
});

const quietBlocks = () => ({ stop: () => ({ samples: 0, maxMs: 0, over50: 0 }) });

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

/** 타이핑 대상 파일의 흉내(첫 줄로 알아본다). */
function typingText(lines = 40): string {
  return [TYPING_FIRST_LINE, ...Array.from({ length: lines - 1 }, (_, i) => `line ${i + 1}`)].join(
    "\n",
  );
}

/**
 * 프로젝트 창 흉내: ⌘9 에 목록을 그리고(다시 누르면 처음엔 활성, 그다음엔 접는다), 더블클릭에 diff 를 그리고, 측정하는 쪽의 체크아웃에
 * 상태바를 바꾼다. 트리의 `perf` 를 더블클릭하면 펼치고, 타이핑 대상을 더블클릭하면 진짜 EditorView 로 연다 — 거터는 문서가 디스크와
 * 다르면 `MARK_FRAMES` 프레임 뒤에 `modified` 표시를 단다. ⌘0 은 커밋 목록을 그리고, 측정하는 쪽이 파일을 고치면 행이 늘고 `git add`
 * 하면 그 파일 행이 스테이징됨 그룹에 생긴다(`onFrame` 이 프레임마다 맞춘다).
 */
function fakeWindow(clock: { t: number }) {
  const doc = document;
  const project = doc.createElement("div");
  project.setAttribute("data-tool-window", "project");
  const perfRow = doc.createElement("div");
  perfRow.setAttribute("data-path", "/work/demo/perf");
  project.append(perfRow);
  const branch = doc.createElement("span");
  branch.setAttribute("data-testid", "status-branch-name");
  branch.textContent = "main";
  doc.body.append(project, branch);
  let dblclickAt: { x: number; y: number } | undefined;
  const signal = listeners.signal;
  let log: HTMLElement | undefined;
  let logHeight = 0;
  let presses = 0;
  let view: EditorView | undefined;
  let original = "";
  let changedSince: number | undefined;
  let commitRows = 0;
  let commit: HTMLElement | undefined;
  const state = { changed: false, staged: false };
  perfRow.addEventListener("dblclick", () => {
    const file = doc.createElement("div");
    file.setAttribute("data-path", "/work/demo/perf/typing.rs");
    file.addEventListener("dblclick", () => {
      original = typingText();
      view = new EditorView({ doc: original, parent: doc.body });
      views.push(view);
    });
    project.append(file);
  });
  doc.body.addEventListener(
    "keydown",
    (e) => {
      if (e.metaKey && e.code === "Digit0") {
        commit = doc.createElement("div");
        commit.setAttribute("data-commit-list", "");
        Object.defineProperty(commit, "scrollHeight", { get: () => commitRows * COMMIT_ROW });
        Object.defineProperty(commit, "clientHeight", { get: () => 700 });
        doc.body.append(commit);
        return;
      }
      if (!(e.metaKey && e.code === "Digit9")) return;
      if (log) {
        // 첫 누름은 활성으로, 그다음은 접는다.
        if (++presses >= 2) logHeight = 0;
        return;
      }
      const list = doc.createElement("div");
      log = list;
      logHeight = 300;
      Object.defineProperty(list, "clientHeight", { get: () => logHeight });
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
  const MARK_FRAMES = 3;
  const onFrame = () => {
    if (view) {
      const dirty = view.state.doc.toString() !== original;
      const mark = view.dom.querySelector("[data-git-change]");
      if (!dirty) {
        changedSince = undefined;
        mark?.remove();
      } else if (changedSince === undefined) changedSince = 0;
      else if (++changedSince >= MARK_FRAMES && !mark) {
        const el = doc.createElement("div");
        el.dataset.gitChange = "modified";
        view.dom.append(el);
      }
    }
    // 고친 파일이 두 번에 나뉘어 들어온다(감시 · 상태 새로 읽기).
    if (state.changed && commitRows < CHANGED_FILES + 3)
      commitRows = commitRows === 0 ? 4000 : CHANGED_FILES + 3;
    if (state.staged && commit && !commit.querySelector("[data-commit-item]")) {
      const row = doc.createElement("div");
      row.setAttribute("data-commit-item", `staged:${STAGE_PATH}`);
      commit.append(row);
    }
  };
  return {
    branch,
    state,
    onFrame,
    dblclick: () => dblclickAt,
    view: () => view,
    original: () => original,
    presses: () => presses,
  };
}

it("the git scenario reports each step in order with what it measured", async () => {
  const clock = { t: 1000 };
  const win = fakeWindow(clock);
  const events: [string, Record<string, unknown>][] = [];
  // 측정하는 쪽의 체크아웃 · git add 는 몇 프레임 뒤에 화면에 닿는다.
  let checkoutIn: number | undefined;
  let checkedOutAt = 0;
  let stageIn: number | undefined;
  let stagedAt = 0;
  const sleeps: number[] = [];
  const env: ScenarioEnv = {
    doc: document,
    work: fakeWork(clock, () => {
      if (checkoutIn !== undefined && --checkoutIn === 0) win.branch.textContent = "perf-checkout";
      if (stageIn !== undefined && --stageIn === 0) win.state.staged = true;
      win.onFrame();
    }),
    now: () => clock.t,
    epochNow: () => 1_700_000_000_000 + clock.t,
    sleep: async (t) => {
      sleeps.push(t);
      clock.t += t;
    },
    watchBlocks: () => ({ stop: () => ({ samples: 6000, maxMs: 12.5, over50: 0 }) }),
    settleMs: 50,
    report: async (event, data) => {
      events.push([event, data]);
      if (event === "checkout_ready") {
        checkoutIn = 3;
        checkedOutAt = clock.t;
      }
      if (event === "changes_ready") win.state.changed = true;
      if (event === "stage_ready") {
        stageIn = 2;
        stagedAt = clock.t;
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
    "gutter_mark",
    "typing",
    "changes_ready",
    "commit_list_scroll",
    "stage_ready",
    "staged_shown",
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
  // 접으려고 ⌘9 를 두 번 더 눌렀다(활성 → 접기).
  expect(win.presses()).toBe(2);
  // 고친 뒤 넷째 프레임에 표시가 생기고(첫 프레임이 고친 것을 보고 `MARK_FRAMES` 를 센다), 그 다음 프레임까지.
  expect(data.gutter_mark).toEqual({ ms: 5 * 16, line: EDIT_LINE });
  expect(data.typing).toEqual({
    keys: TYPING_KEYS,
    lines: 40,
    blocks: { samples: 6000, maxMs: 12.5, over50: 0 },
    dispatchP50Ms: 0,
    dispatchMaxMs: 0,
  });
  expect(sleeps.filter((t) => t === KEY_MS)).toHaveLength(TYPING_KEYS);
  // 고친 줄 · 넣은 글자를 모두 되돌려 디스크와 같다(끌 때 미저장 확인이 뜨지 않는다).
  expect(win.view()!.state.doc.toString()).toBe(win.original());
  const commit = data.commit_list_scroll as {
    rows: number;
    fling: { interval: { n: number } };
    steady: { interval: { n: number } };
    viewport: { height: number };
  };
  // 4,000 행에서 멈추지 않고 1만 행 이상이 된 뒤에 다 받은 것으로 본다.
  expect(commit.rows).toBe(CHANGED_FILES + 3);
  expect(commit.fling.interval.n).toBe(COMMIT_FLING_FRAMES);
  expect(commit.steady.interval.n).toBe(STEADY_FRAMES);
  expect(commit.viewport.height).toBe(700);
  expect(data.stage_ready).toEqual({ path: STAGE_PATH });
  // git add 가 둘째 프레임에 닿고, 행이 생긴 그 프레임 · 다음 프레임.
  expect(data.staged_shown).toEqual({
    path: STAGE_PATH,
    epochMs: 1_700_000_000_000 + stagedAt + 3 * 16,
  });
});

it("the gutter step waits for the HEAD base before it times the edit", async () => {
  // 기준이 늦게 들어와 첫 표시가 30 프레임 뒤에야 생겨도, 잰 값은 그 뒤에 고친 줄의 것이다.
  const clock = { t: 0 };
  const win = fakeWindow(clock);
  let baseIn = 30;
  const events: [string, Record<string, unknown>][] = [];
  let checkoutIn: number | undefined;
  await runGitScenario({
    doc: document,
    work: fakeWork(clock, () => {
      if (checkoutIn !== undefined && --checkoutIn === 0) win.branch.textContent = "perf-checkout";
      if (win.view() && baseIn > 0) baseIn--;
      else win.onFrame();
    }),
    now: () => clock.t,
    epochNow: () => clock.t,
    sleep: async (t) => void (clock.t += t),
    watchBlocks: quietBlocks,
    settleMs: 50,
    report: async (event, data) => {
      events.push([event, data]);
      if (event === "checkout_ready") checkoutIn = 1;
      if (event === "changes_ready") throw new Error("stop after the gutter");
    },
  }).catch(() => {});
  const gutter = events.find(([e]) => e === "gutter_mark")?.[1];
  expect(gutter).toEqual({ ms: 5 * 16, line: EDIT_LINE });
});

it("the scenario's DOM hooks match the app's components", () => {
  // 커밋 목록의 행 높이.
  expect(COMMIT_ROW).toBe(ROW);
  // 트리의 `data-path` 는 절대 경로다 — 끝 조각으로 고르고, 이름이 같은 다른 폴더의 것은 고르지 않는다.
  const project = document.createElement("div");
  project.setAttribute("data-tool-window", "project");
  project.innerHTML =
    '<div data-path="/w/xperf"></div><div data-path="/w/perf"></div><div data-path="C:\\w\\perf\\typing.rs"></div>';
  document.body.append(project);
  expect(treeRow(document, "perf")?.getAttribute("data-path")).toBe("/w/perf");
  expect(treeRow(document, "perf/typing.rs")).not.toBeNull();
  expect(treeRow(document, "perf/many")).toBeNull();
  // 열린 에디터 중 첫 줄이 맞는 것.
  const other = new EditorView({ doc: "fn main() {}\n", parent: document.body });
  const typing = new EditorView({ doc: typingText(), parent: document.body });
  views.push(other, typing);
  expect(typingView(document)).toBe(typing);
  expect(TYPING_LINE).toBeLessThan(40);
});

it("the block probe counts the time its timer came late as a block", () => {
  vi.useFakeTimers();
  let now = 0;
  const probe = watchBlocks(() => now, 5);
  now = 5;
  vi.advanceTimersByTime(5);
  // 다음 콜백이 70 ms 뒤에 왔다 — 간격 5 ms 를 뺀 65 ms 막힘. 그 다음 45 ms 는 40 ms 라 롱태스크가 아니다.
  now = 75;
  vi.advanceTimersByTime(5);
  now = 80;
  vi.advanceTimersByTime(5);
  now = 125;
  vi.advanceTimersByTime(5);
  expect(probe.stop()).toEqual({ samples: 4, maxMs: 65, over50: 1 });
  // 멈춘 뒤에는 돌지 않는다.
  now = 1000;
  vi.advanceTimersByTime(50);
  expect(probe.stop().samples).toBe(4);
});

it("the git log is collapsed with ⌘9 until it has no height, or the step fails", async () => {
  const clock = { t: 0 };
  const list = document.createElement("div");
  document.body.append(list);
  let height = 300;
  let presses = 0;
  Object.defineProperty(list, "clientHeight", { get: () => height });
  const onKey = (e: KeyboardEvent) => {
    if (e.metaKey && e.code === "Digit9" && ++presses === 2) height = 0;
  };
  document.body.addEventListener("keydown", onKey, { signal: listeners.signal });
  await hideGitLog({ doc: document, work: fakeWork(clock) }, list);
  expect(presses).toBe(2);
  height = 300;
  presses = -100;
  await expect(hideGitLog({ doc: document, work: fakeWork(clock) }, list)).rejects.toThrow(
    "the git log did not collapse",
  );
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
    watchBlocks: quietBlocks,
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
