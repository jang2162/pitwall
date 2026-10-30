// 앱 안 측정 시나리오(release 번들, `crates/pw-perf` 의 git 측정 — CONTRIBUTING.md 「성능 기준선」). 측정 모드로 뜬 앱이 프로젝트 창
// 주소에 `perfScenario=<이름>` 을 붙이면(src-tauri/src/perf.rs) 첫 프레임 뒤에 여기의 시나리오를 돌리고, 단계마다 `report` 로 잰 값을
// 표시 파일에 남긴다. 그 밖에는 이 모듈을 불러오지 않는다(main.tsx 가 동적으로 부른다).
//
// `git` 시나리오는 합성 10만 커밋 저장소(`synth_history --perf-fixture`)를 연 프로젝트 창에서 13-m2-plan.md 의 완료 기준 중 release
// 번들에서 재는 것을 잰다:
//
// 1. `git_log_open`(M2-14, 결정 M2-D10): ⌘9 keydown 을 문서에 넣은 때부터 Git 로그 목록 canvas 에 첫 짧은 해시를 그린 뒤 다음
//    프레임까지. 키는 OS 를 거치지 않는다 — 웹뷰 안에서 만든 이벤트를 키맵이 받는다(사람의 키 입력 · 포커스와 상관없다).
// 2. `git_log_rows` · `git_log_scroll`(M2-14): 행 수가 1초 동안 늘지 않으면 다 받은 것으로 보고, 처음부터 끝까지 `FLING_FRAMES`
//    프레임에(`fling`) · 한 프레임에 2행씩 `STEADY_FRAMES` 프레임(`steady`) 스크롤한다(src/perf-frames.ts). 스크롤 전에 툴 윈도우를
//    `toolWindow.stretchUp`(⌃⌥⇧↑, 웹뷰 안 이벤트)으로 더 커지지 않을 때까지 키운다 — 헤드리스 하네스가 하단 dock 을 높인 것과 같은 뜻이다.
// 3. `diff_open`(M2-15): 목록 첫 행(1만 줄을 바꾼 커밋)을 더블클릭한 때부터 커밋 상세 탭의 diff 줄(`.pw-diff-line`)이 그려진 다음
//    프레임까지 — Host 의 `git.show` · `git.diff` · `git.blob` 와 IPC 가 모두 든다.
// 4. `checkout_ready` → `branch_shown`(M2-13): 상태바 브랜치 이름을 적어 알리면 측정하는 쪽이 저장소에서 `git checkout` 을 돌리고,
//    이름이 바뀐 다음 프레임의 벽시계(`epochMs`)를 알린다. 측정하는 쪽이 체크아웃 직전 시각을 빼 「터미널 체크아웃 → 상태바」 를 낸다.
// 5. `gutter_mark`(M2-17): 3 에서 키운 Git 로그를 ⌘9 로 접고(활성이 아니면 한 번 더 — 처음 누름은 그 툴 윈도우를 활성으로 만든다),
//    프로젝트 트리에서 `TYPING_PATH`(5,000줄)를 더블클릭해 열고, 한 번 고쳐 거터 표시가 뜨는 것(HEAD 기준이 들어온
//    것)을 보고 되돌린 뒤, `EDIT_LINE` 끝에 글자를 넣은(저장 안 함) 때부터 `modified` 거터 표시가 그려진 다음 프레임까지.
// 6. `typing`(M2-17): `TYPING_LINE` 끝에 `KEY_MS` 마다 한 자씩 `TYPING_KEYS` 자(초당 10자 30초)를 넣는 동안 `watchBlocks` 탐침이 메인
//    스레드가 한 번에 막힌 시간을 잰다(WebKit 에는 Long Tasks API 가 없다). 끝나면 넣은 글자를 지워 문서를 디스크와 같게 둔다.
// 7. `changes_ready` → `commit_list_scroll`(M2-16): 알리면 측정하는 쪽이 `perf/many` 의 파일 1만 개를 고친다. ⌘0(웹뷰 안 이벤트)으로
//    커밋 툴 윈도우를 열고, 행 수가 `CHANGED_FILES` 이상에서 1초 동안 그대로면 다 받은 것으로 보고 `COMMIT_FLING_FRAMES` 프레임에 끝까지
//    (`fling`) · 한 프레임에 2행씩 `STEADY_FRAMES` 프레임(`steady`) 스크롤한다.
// 8. `stage_ready` → `staged_shown`(M2-16): `STAGE_PATH` 를 알리면 측정하는 쪽이 저장소에서 `git add` 하고, 그 파일 행이 스테이징됨
//    그룹에 그려진 다음 프레임의 벽시계를 알린다(4 와 같은 방법).
//
// 끝나면 `scenario_done`, 단계가 실패하면 `scenario_error { step, message }` 하나를 남기고 멈춘다.

import { EditorView } from "@codemirror/view";

import { ROW_HEIGHT } from "./git/log/graph-render";
import { type FrameWork, installFrameWork, scrollFrames } from "./perf-frames";

export const SCENARIOS = ["git"] as const;
export type Scenario = (typeof SCENARIOS)[number];

export const FLING_FRAMES = 2000;
export const STEADY_FRAMES = 600;

/** 타이핑 대상(`pw_git::testing::synth::PERF_TYPING_PATH`) · 그 첫 줄 — 열린 에디터 중 이 파일을 내용으로 알아본다. */
export const TYPING_PATH = "perf/typing.rs";
export const TYPING_FIRST_LINE = "// pitwall perf: typing target";
/** 거터 기준이 들어왔는지 볼 때 고치는 줄 · 재는 줄 · 타이핑하는 줄(모두 첫 화면 안). */
export const PROBE_LINE = 10;
export const EDIT_LINE = 12;
export const TYPING_LINE = 20;
/** 초당 10자 30초. */
export const TYPING_KEYS = 300;
export const KEY_MS = 100;
/** 막힘 탐침의 간격 · 롱태스크로 볼 막힘. */
export const PROBE_MS = 5;
export const BLOCK_MS = 50;

/** 측정하는 쪽이 고치는 파일 수(`pw_git::testing::synth::PERF_CHANGES`) · 그중 `git add` 할 것. */
export const CHANGED_FILES = 10_000;
export const STAGE_PATH = "perf/many/f00000.txt";
/** 커밋 목록 한 행의 높이(src/git/commit/commit-tool-window.tsx 의 `ROW` — 시험이 같은지 본다). */
export const COMMIT_ROW = 22;
/** 1만 행을 끝까지 — 커밋 목록 하네스(perf/commit-list/commit-list-bench.tsx)의 fling 과 같다. */
export const COMMIT_FLING_FRAMES = 1500;

/** 시나리오가 쓰는 바깥. 시험은 가짜를 넘긴다. */
export interface ScenarioEnv {
  doc: Document;
  report(event: string, data: Record<string, unknown>): Promise<void>;
  /** 측정하는 쪽의 프레임(앱의 rAF 를 감싸기 전). */
  work: FrameWork;
  now(): number;
  /** 벽시계(Unix epoch, ms) — 측정하는 쪽의 시각과 같은 시계. */
  epochNow(): number;
  sleep(ms: number): Promise<void>;
  /** 메인 스레드의 막힘을 재기 시작한다(`watchBlocks`). */
  watchBlocks(): BlockProbe;
  /** 「다 받았다」 로 볼, 행 수가 늘지 않은 시간. */
  settleMs?: number;
  /** Git 로그 · diff · 체크아웃(1~4)까지만 돈다 — 헤드리스 스모크(perf/git-log/scenario-bench.tsx)의 가짜 Host 에는 거터 · 커밋 목록이 없다. */
  logOnly?: boolean;
}

export interface BlockStats {
  /** 탐침이 돈 횟수. */
  samples: number;
  /** 가장 긴 막힘(ms). */
  maxMs: number;
  /** `BLOCK_MS` 를 넘은 막힘의 수. */
  over50: number;
}

export interface BlockProbe {
  stop(): BlockStats;
}

const LIST = "[data-testid=git-log-list]";
const SHORT_HASH = /^[0-9a-f]{8}$/;

/** 상태바의 브랜치 이름(저장소를 아직 읽지 않았으면 없다). */
export function branchName(doc: Document): string | undefined {
  return doc.querySelector("[data-testid=status-branch-name]")?.textContent ?? undefined;
}

/** 프레임마다 `test` 를 보고 참이 되면 그 값을 돌려준다. `timeoutMs` 안에 안 되면 실패한다. */
export async function until<T>(
  env: Pick<ScenarioEnv, "work" | "now">,
  what: string,
  test: () => T | undefined | null | false,
  timeoutMs = 60_000,
): Promise<T> {
  const started = env.now();
  for (;;) {
    const value = test();
    if (value) return value;
    if (env.now() - started > timeoutMs) throw new Error(`timed out waiting for ${what}`);
    await env.work.frame();
  }
}

/** 목록의 행 수가 `settleMs` 동안 늘지 않을 때까지 기다린다. 다 받은 행 수와 그때의 시각. */
export function settledRows(
  env: Pick<ScenarioEnv, "work" | "now">,
  list: Element,
  settleMs: number,
  timeoutMs = 300_000,
): Promise<{ rows: number; at: number }> {
  return settledCount(
    env,
    "every log row",
    () => Number(list.getAttribute("aria-rowcount") ?? 0),
    1,
    settleMs,
    timeoutMs,
  );
}

/** `count` 가 `least` 이상에서 `settleMs` 동안 그대로일 때까지 기다린다. 그 값과 그 값이 처음 보인 시각. */
export async function settledCount(
  env: Pick<ScenarioEnv, "work" | "now">,
  what: string,
  count: () => number,
  least: number,
  settleMs: number,
  timeoutMs = 300_000,
): Promise<{ rows: number; at: number }> {
  const started = env.now();
  let rows = -1;
  let changed = env.now();
  for (;;) {
    const now = count();
    if (now !== rows) {
      rows = now;
      changed = env.now();
    } else if (rows >= least && env.now() - changed >= settleMs) return { rows, at: changed };
    if (env.now() - started > timeoutMs) throw new Error(`timed out waiting for ${what}`);
    await env.work.frame();
  }
}

/**
 * 메인 스레드의 막힘 탐침: `interval` 마다 setTimeout 을 다시 걸고, 콜백 사이가 `interval` 보다 늦은 만큼을 그동안 막힌 시간으로
 * 본다(그 사이에 돈 작업 · 그리기가 한 번에 든 시간). WebKit 에는 Long Tasks API 가 없어 이것으로 「롱태스크」 를 갈음한다.
 */
export function watchBlocks(now: () => number, interval = PROBE_MS): BlockProbe {
  const stats: BlockStats = { samples: 0, maxMs: 0, over50: 0 };
  let last = now();
  let timer: ReturnType<typeof setTimeout> | undefined;
  const tick = () => {
    const t = now();
    const block = Math.max(0, t - last - interval);
    stats.samples++;
    stats.maxMs = Math.max(stats.maxMs, block);
    if (block > BLOCK_MS) stats.over50++;
    last = t;
    timer = setTimeout(tick, interval);
  };
  timer = setTimeout(tick, interval);
  return {
    stop: () => {
      clearTimeout(timer);
      return { ...stats, maxMs: ms(stats.maxMs) };
    },
  };
}

/** Git 로그 목록 canvas 에 처음으로 짧은 해시를 그린 시각을 적는다. `restore` 로 걷는다. */
export function watchFirstHash(now: () => number) {
  const proto = CanvasRenderingContext2D.prototype;
  const fillText = proto.fillText;
  let first: { at: number; text: string } | undefined;
  proto.fillText = function (this: CanvasRenderingContext2D, text, x, y, maxWidth) {
    if (first === undefined && SHORT_HASH.test(text) && this.canvas.closest?.(LIST))
      first = { at: now(), text };
    return fillText.call(this, text, x, y, maxWidth);
  };
  return {
    first: () => first,
    restore: () => void (proto.fillText = fillText),
  };
}

const ms = (x: number) => Math.round(x * 10) / 10;

const cmd9 = () =>
  new KeyboardEvent("keydown", {
    code: "Digit9",
    key: "9",
    metaKey: true,
    bubbles: true,
    cancelable: true,
  });

async function gitLogOpen(env: ScenarioEnv) {
  const { doc } = env;
  const hash = watchFirstHash(env.now);
  try {
    const started = env.now();
    doc.body.dispatchEvent(cmd9());
    const drawn = await until(env, "the first log row", hash.first, 30_000);
    await env.work.frame();
    const list = doc.querySelector<HTMLElement>(LIST)!;
    await env.report("git_log_open", {
      openMs: ms(env.now() - started),
      firstDrawMs: ms(drawn.at - started),
      firstHash: drawn.text,
      viewport: { width: list.clientWidth, height: list.clientHeight },
    });
    return { list, started };
  } finally {
    hash.restore();
  }
}

/** 툴 윈도우를 위로 키운다: 목록에 포커스를 두고 ⌃⌥⇧↑ 를 높이가 세 번 연달아 그대로일 때까지(최대 `STRETCH_MAX` 번) 넣는다. */
export async function stretchUp(env: Pick<ScenarioEnv, "work">, list: HTMLElement) {
  list.focus();
  let height = list.clientHeight;
  let still = 0;
  for (let i = 0; i < STRETCH_MAX && still < 3; i++) {
    (list.ownerDocument.activeElement ?? list).dispatchEvent(
      new KeyboardEvent("keydown", {
        code: "ArrowUp",
        key: "ArrowUp",
        ctrlKey: true,
        altKey: true,
        shiftKey: true,
        bubbles: true,
        cancelable: true,
      }),
    );
    await env.work.frame();
    await env.work.frame();
    still = list.clientHeight === height ? still + 1 : 0;
    height = list.clientHeight;
  }
}

export const STRETCH_MAX = 80;

async function gitLogScroll(env: ScenarioEnv, list: HTMLElement, started: number) {
  const { rows, at } = await settledRows(env, list, env.settleMs ?? 1000);
  await env.report("git_log_rows", { rows, allRowsMs: ms(at - started) });
  await stretchUp(env, list);
  const height = rows * ROW_HEIGHT - list.clientHeight;
  const fling = await scrollFrames(
    list,
    FLING_FRAMES,
    (f) => Math.round((height * (f + 1)) / FLING_FRAMES),
    env.work,
  );
  list.scrollTop = 0;
  await env.work.frame();
  const steady = await scrollFrames(list, STEADY_FRAMES, (f) => (f + 1) * 2 * ROW_HEIGHT, env.work);
  await env.report("git_log_scroll", {
    rows,
    viewport: { width: list.clientWidth, height: list.clientHeight },
    fling,
    steady,
  });
}

async function diffOpen(env: ScenarioEnv, list: HTMLElement) {
  const { doc } = env;
  list.scrollTop = 0;
  list.dispatchEvent(new Event("scroll"));
  await env.work.frame();
  await env.work.frame();
  const rect = list.getBoundingClientRect();
  const started = env.now();
  list.dispatchEvent(
    new MouseEvent("dblclick", {
      bubbles: true,
      cancelable: true,
      clientX: rect.left + rect.width / 2,
      clientY: rect.top + ROW_HEIGHT / 2,
    }),
  );
  const host = await until(
    env,
    "the diff lines",
    () => doc.querySelector(".pw-diff-line")?.closest("[data-git-file-diff]"),
    60_000,
  );
  await env.work.frame();
  await env.report("diff_open", {
    ms: ms(env.now() - started),
    path: host.getAttribute("data-git-file-diff"),
    paintedLines: host.querySelectorAll(".pw-diff-line").length,
  });
}

async function checkout(env: ScenarioEnv) {
  const from = branchName(env.doc);
  await env.report("checkout_ready", { branch: from ?? null });
  const to = await until(
    env,
    "the status bar branch to change",
    () => {
      const now = branchName(env.doc);
      return now !== undefined && now !== from ? now : undefined;
    },
    30_000,
  );
  await env.work.frame();
  await env.report("branch_shown", { branch: to, epochMs: env.epochNow() });
}

/** 프로젝트 트리에서 `path`(작업 트리 기준 `/` 경로)의 행. 트리의 `data-path` 는 절대 경로다. */
export function treeRow(doc: Document, path: string): HTMLElement | null {
  for (const row of doc.querySelectorAll<HTMLElement>("[data-tool-window=project] [data-path]")) {
    const at = (row.getAttribute("data-path") ?? "").replaceAll("\\", "/");
    if (at === path || at.endsWith(`/${path}`)) return row;
  }
  return null;
}

/** 열린 에디터 중 `TYPING_PATH` 의 것(첫 줄로 알아본다). */
export function typingView(doc: Document): EditorView | undefined {
  for (const dom of doc.querySelectorAll<HTMLElement>(".cm-editor")) {
    const view = EditorView.findFromDOM(dom);
    if (view && view.state.doc.line(1).text === TYPING_FIRST_LINE) return view;
  }
  return undefined;
}

const hasModifiedMark = (view: EditorView) =>
  view.dom.querySelector('[data-git-change="modified"]') !== null;

/** 줄 `line` 끝에 `text` 를 넣는다(저장 안 함). 되돌리는 함수를 돌려준다. */
function appendToLine(view: EditorView, line: number, text: string): () => void {
  const at = view.state.doc.line(line).to;
  view.dispatch({ changes: { from: at, insert: text }, userEvent: "input.type" });
  return () => view.dispatch({ changes: { from: at, to: at + text.length }, userEvent: "delete" });
}

function dblclick(el: Element) {
  el.dispatchEvent(new MouseEvent("dblclick", { bubbles: true, cancelable: true }));
}

/** 키운 Git 로그 dock 을 접는다 — 에디터 · 프로젝트 트리가 다시 자리를 받는다. ⌘9 는 활성인 툴 윈도우면 접고, 아니면 활성으로 만든다. */
export async function hideGitLog(env: Pick<ScenarioEnv, "doc" | "work">, list: HTMLElement) {
  for (let i = 0; i < 4 && list.isConnected && list.clientHeight > 0; i++) {
    env.doc.body.dispatchEvent(cmd9());
    await env.work.frame();
    await env.work.frame();
  }
  if (list.isConnected && list.clientHeight > 0) throw new Error("the git log did not collapse");
}

async function gutterMark(env: ScenarioEnv, log: HTMLElement): Promise<EditorView> {
  const { doc } = env;
  await hideGitLog(env, log);
  if (!treeRow(doc, TYPING_PATH))
    dblclick(await until(env, "the perf folder in the project tree", () => treeRow(doc, "perf")));
  dblclick(await until(env, `${TYPING_PATH} in the project tree`, () => treeRow(doc, TYPING_PATH)));
  const view = await until(env, `the editor of ${TYPING_PATH}`, () => typingView(doc), 30_000);
  // 거터의 기준(HEAD 의 그 파일)이 들어오기 전에 고치면 기준을 받는 시간까지 재게 된다 — 한 번 고쳐 표시를 보고 되돌린다.
  const probe = appendToLine(view, PROBE_LINE, " ");
  await until(env, "the first gutter mark (the HEAD base)", () => hasModifiedMark(view), 30_000);
  probe();
  await until(env, "the probe's gutter mark to clear", () => !hasModifiedMark(view), 10_000);
  await env.sleep(300);
  const started = env.now();
  const revert = appendToLine(view, EDIT_LINE, " // edited");
  await until(env, "the gutter mark of the edited line", () => hasModifiedMark(view), 10_000);
  await env.work.frame();
  const elapsed = env.now() - started;
  revert();
  await until(env, "the edit's gutter mark to clear", () => !hasModifiedMark(view), 10_000);
  await env.report("gutter_mark", { ms: ms(elapsed), line: EDIT_LINE });
  return view;
}

async function typing(env: ScenarioEnv, view: EditorView) {
  const start = view.state.doc.line(TYPING_LINE).to;
  view.dispatch({ selection: { anchor: start } });
  await env.sleep(300);
  const dispatch: number[] = [];
  const probe = env.watchBlocks();
  for (let key = 0; key < TYPING_KEYS; key++) {
    const at = start + key;
    const t = env.now();
    view.dispatch({
      changes: { from: at, insert: "x" },
      selection: { anchor: at + 1 },
      userEvent: "input.type",
    });
    dispatch.push(env.now() - t);
    await env.sleep(KEY_MS);
  }
  const blocks = probe.stop();
  // 넣은 글자를 지워 디스크와 같게 둔다 — 끌 때 미저장 확인이 뜨지 않게.
  view.dispatch({ changes: { from: start, to: start + TYPING_KEYS }, userEvent: "delete" });
  dispatch.sort((a, b) => a - b);
  await env.report("typing", {
    keys: TYPING_KEYS,
    lines: view.state.doc.lines,
    blocks,
    dispatchP50Ms: ms(dispatch[Math.floor(dispatch.length / 2)]!),
    dispatchMaxMs: ms(dispatch.at(-1)!),
  });
}

const COMMIT_LIST = "[data-commit-list]";

async function commitList(env: ScenarioEnv): Promise<HTMLElement> {
  const { doc } = env;
  if (doc.activeElement instanceof HTMLElement) doc.activeElement.blur();
  const started = env.now();
  await env.report("changes_ready", {});
  doc.body.dispatchEvent(
    new KeyboardEvent("keydown", {
      code: "Digit0",
      key: "0",
      metaKey: true,
      bubbles: true,
      cancelable: true,
    }),
  );
  const list = await until(env, "the commit list", () =>
    doc.querySelector<HTMLElement>(COMMIT_LIST),
  );
  const { rows, at } = await settledCount(
    env,
    `${CHANGED_FILES} changed files in the commit list`,
    () => Math.floor(list.scrollHeight / COMMIT_ROW),
    CHANGED_FILES,
    env.settleMs ?? 1000,
  );
  const height = list.scrollHeight - list.clientHeight;
  list.scrollTop = 0;
  await env.work.frame();
  const fling = await scrollFrames(
    list,
    COMMIT_FLING_FRAMES,
    (f) => Math.round((height * (f + 1)) / COMMIT_FLING_FRAMES),
    env.work,
  );
  list.scrollTop = 0;
  await env.work.frame();
  const steady = await scrollFrames(list, STEADY_FRAMES, (f) => (f + 1) * 2 * COMMIT_ROW, env.work);
  await env.report("commit_list_scroll", {
    rows,
    allRowsMs: ms(at - started),
    viewport: { width: list.clientWidth, height: list.clientHeight },
    fling,
    steady,
  });
  return list;
}

async function stage(env: ScenarioEnv, list: HTMLElement) {
  // 스테이징됨 그룹은 목록 맨 위다.
  list.scrollTop = 0;
  list.dispatchEvent(new Event("scroll"));
  await env.work.frame();
  await env.work.frame();
  await env.report("stage_ready", { path: STAGE_PATH });
  await until(
    env,
    `${STAGE_PATH} in the staged group`,
    () => list.querySelector(`[data-commit-item="staged:${STAGE_PATH}"]`),
    30_000,
  );
  await env.work.frame();
  await env.report("staged_shown", { path: STAGE_PATH, epochMs: env.epochNow() });
}

export async function runGitScenario(env: ScenarioEnv): Promise<void> {
  let step = "start";
  try {
    step = "project window";
    await until(env, "the project window", () =>
      env.doc.querySelector("[data-tool-window=project]"),
    );
    step = "status bar branch";
    await until(env, "the status bar branch", () => branchName(env.doc));
    // 키맵이 붙고 첫 그림의 뒤처리가 끝나도록(헤드리스 하네스와 같다).
    await env.sleep(300);
    step = "git log open";
    const { list, started } = await gitLogOpen(env);
    step = "git log scroll";
    await gitLogScroll(env, list, started);
    step = "diff open";
    await diffOpen(env, list);
    step = "checkout";
    await checkout(env);
    if (!env.logOnly) {
      step = "gutter";
      const view = await gutterMark(env, list);
      step = "typing";
      await typing(env, view);
      step = "commit list";
      const commits = await commitList(env);
      step = "stage";
      await stage(env, commits);
    }
    await env.report("scenario_done", {});
  } catch (e) {
    await env.report("scenario_error", {
      step,
      message: e instanceof Error ? e.message : String(e),
    });
  }
}

/** 웹뷰의 실제 바깥으로 `scenario` 를 돌린다. */
export async function runScenario(
  scenario: Scenario,
  report: ScenarioEnv["report"],
): Promise<void> {
  const work = installFrameWork();
  try {
    if (scenario === "git")
      await runGitScenario({
        doc: document,
        report,
        work,
        now: () => performance.now(),
        // 측정하는 쪽의 벽시계와 같은 시계 — `timeOrigin + performance.now()` 는 단조 시계라 몇 분 지나면 벽시계와 어긋날 수 있다.
        epochNow: () => Date.now(),
        sleep: (t) => new Promise((r) => setTimeout(r, t)),
        watchBlocks: () => watchBlocks(() => performance.now()),
      });
  } finally {
    work.restore();
  }
}
