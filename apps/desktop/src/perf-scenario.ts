// 앱 안 측정 시나리오(release 번들, `crates/pw-perf` 의 git 측정 — CONTRIBUTING.md 「성능 기준선」). 측정 모드로 뜬 앱이 프로젝트 창
// 주소에 `perfScenario=<이름>` 을 붙이면(src-tauri/src/perf.rs) 첫 프레임 뒤에 여기의 시나리오를 돌리고, 단계마다 `report` 로 잰 값을
// 표시 파일에 남긴다. 그 밖에는 이 모듈을 불러오지 않는다(main.tsx 가 동적으로 부른다).
//
// `git` 시나리오는 합성 10만 커밋 저장소(`synth_history --perf-fixture`)를 연 프로젝트 창에서 13-m2-plan.md 의 세 완료 기준을 잰다:
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
//
// 끝나면 `scenario_done`, 단계가 실패하면 `scenario_error { step, message }` 하나를 남기고 멈춘다.

import { ROW_HEIGHT } from "./git/log/graph-render";
import { type FrameWork, installFrameWork, scrollFrames } from "./perf-frames";

export const SCENARIOS = ["git"] as const;
export type Scenario = (typeof SCENARIOS)[number];

export const FLING_FRAMES = 2000;
export const STEADY_FRAMES = 600;

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
  /** 「다 받았다」 로 볼, 행 수가 늘지 않은 시간. */
  settleMs?: number;
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
export async function settledRows(
  env: Pick<ScenarioEnv, "work" | "now">,
  list: Element,
  settleMs: number,
  timeoutMs = 300_000,
): Promise<{ rows: number; at: number }> {
  const started = env.now();
  let rows = -1;
  let changed = env.now();
  for (;;) {
    const now = Number(list.getAttribute("aria-rowcount") ?? 0);
    if (now !== rows) {
      rows = now;
      changed = env.now();
    } else if (rows > 0 && env.now() - changed >= settleMs) return { rows, at: changed };
    if (env.now() - started > timeoutMs) throw new Error("timed out waiting for every log row");
    await env.work.frame();
  }
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

async function gitLogOpen(env: ScenarioEnv) {
  const { doc } = env;
  const hash = watchFirstHash(env.now);
  try {
    const started = env.now();
    doc.body.dispatchEvent(
      new KeyboardEvent("keydown", {
        code: "Digit9",
        key: "9",
        metaKey: true,
        bubbles: true,
        cancelable: true,
      }),
    );
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
        epochNow: () => performance.timeOrigin + performance.now(),
        sleep: (t) => new Promise((r) => setTimeout(r, t)),
      });
  } finally {
    work.restore();
  }
}
