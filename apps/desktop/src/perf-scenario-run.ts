// 앱 안 실행 · 디버그 측정 시나리오(release 번들, `crates/pw-perf` 의 `pw-perf-m6` — CONTRIBUTING.md 「성능 기준선」 의 「M6 측정」).
// perf-scenario.ts 가 측정 모드 프로젝트 창에서 첫 프레임 뒤에 부른다. 17-m6-plan.md M6-22 의 ② · ③ 을 잰다. 프로젝트에는 측정하는 쪽이
// 실행 구성 하나(`.pitwall/run/<id>.toml`)를 적어 두고 신뢰로 등록한다 — 구성이 하나라 「지금 구성」 은 그것이다(run-configs.ts
// `current`). 누르기는 모두 그 요소에 보낸 클릭이다(OS 입력이 아니다). 첫 실행 확인(run-confirm-dialog.tsx)이 뜨면 그 [실행] 을 누른다.
//
// `run-flood` 시나리오 — 출력 많은 실행 하나의 콘솔(M6-22 ②):
//
// 1. `run_started`: 타이틀바의 실행(`run-start`)을 누르고(확인이 뜨면 [실행]) 실행 탭이 생긴 때. 측정하는 쪽은 이 시각 앞의 Host
//    메모리를 기준으로 본다.
// 2. `flood_frames`: 콘솔(xterm)에 첫 출력이 쓰인 때부터 `FLOOD_MEASURE_MS` 동안 프레임 간격(`frameStats` — 판정은 스크롤과 같은
//    `withinVsync` ≥ 0.95 · `vsyncMs` ≤ 17), 앱이 그 프레임에 건 rAF 콜백 시간, 메인 스레드 막힘(`watchBlocks`), 그 사이 콘솔에 그린
//    바이트와 1초마다의 누적(`timeline`)을 잰다. 콘솔이 뒤처지면 Host 의 링(8 MiB)이 앞을 버리고 콘솔은 그만큼을 한 줄로 알린다
//    (run-stream.ts) — 그 줄은 `notes` 에 든다.
// 3. `flood_done`: 실행이 끝난(탭의 실행이 `exited`) 뒤 콘솔이 1초 동안 새 바이트를 받지 않고 그 뒤의 빈 쓰기가 그려진 때(콘솔이
//    따라잡은 때). 그린 바이트 · 알림 줄(버린 앞 · 끝남)을 싣는다.
//
// `debug-end` 시나리오 — 디버그 어댑터가 세션과 함께 끝나는지(M6-22 ③):
//
// 1. `debug_starting`: 첫 프레임 `DEBUG_SETTLE_MS` 뒤, 타이틀바의 디버그(`run-debug`)를 누르기 직전. 측정하는 쪽은 이 앞에 앱의 자손 프로세스가 없었는지 본다.
// 2. `debug_running`: 디버그 세션(windowDebug)이 생긴 때.
// 3. `debug_ended`: 세션이 `ended` 가 된 때. 대상이 `NATURAL_END_MS` 안에 스스로 끝나면 `how: "exited"`(시각은 세션이 끝남을 본 때),
//    아니면 그때 세션을 멈추고(`run.stop` — 디버그 툴 윈도우의 중지와 같은 함수) `how: "stopped"`(시각은 멈춤을 부른 때). 측정하는
//    쪽이 이 시각부터 어댑터 프로세스가 사라질 때까지를 잰다.
//
// 끝나면 `scenario_done`, 단계가 실패하면 `scenario_error { step, message }` 하나를 남기고 멈춘다.

import type { Terminal } from "@xterm/xterm";

import { windowDebug } from "./debug/debug-session";
import { frameStats } from "./perf-frames";
import type { ScenarioEnv } from "./perf-scenario";
import { windowRuns } from "./run/run-tabs";

/** 프레임을 재는 시간 — 측정하는 쪽의 출력(`pw-perf-flood`)은 이보다 길게 돈다. */
export const FLOOD_MEASURE_MS = 20_000;
/** 실행이 끝난 뒤 이만큼 새 바이트가 없으면 콘솔이 따라잡은 것으로 본다. */
export const CAUGHT_UP_MS = 1_000;
/**
 * 디버그를 누르기 전에 쉬는 시간 — 신뢰 프로젝트를 열면 런타임 환경을 읽는 로그인 셸이 잠깐 뜬다(0.1~0.2초). 그것이 「디버그를 누르기 전의
 * 자손」 으로 섞이지 않게 한다(M6-22 측정에서 17회 중 3회 겹쳤다).
 */
export const DEBUG_SETTLE_MS = 3_000;
/** 디버그 대상이 이 안에 스스로 끝나지 않으면 세션을 멈춘다. */
export const NATURAL_END_MS = 4_000;

const CONFIRM = "[data-testid=run-confirm-dialog]";

const ms = (x: number) => Math.round(x * 10) / 10;

/**
 * `test` 가 참이 될 때까지 50 ms 마다 본다. rAF 가 아니라 타이머로 기다린다 — 창이 가려지면 WebKit 이 rAF 를 멈춰 기다림이 멎는다.
 * 가려진 회의 프레임 · 메모리는 측정하는 쪽이 제외하지만, 실행을 띄우고 끝내는 단계(③ 어댑터 끝 · Host 메모리)는 계속 돌아야 한다.
 */
export async function poll<T>(
  env: Pick<ScenarioEnv, "now" | "sleep">,
  what: string,
  test: () => T | undefined | null | false,
  timeoutMs = 60_000,
): Promise<T> {
  const started = env.now();
  for (;;) {
    const value = test();
    if (value) return value;
    if (env.now() - started > timeoutMs) throw new Error(`timed out waiting for ${what}`);
    await env.sleep(50);
  }
}

class StepError extends Error {
  constructor(
    readonly step: string,
    message: string,
  ) {
    super(message);
  }
}

async function step<T>(name: string, run: () => Promise<T>): Promise<T> {
  try {
    return await run();
  } catch (e) {
    if (e instanceof StepError) throw e;
    throw new StepError(name, e instanceof Error ? e.message : String(e));
  }
}

/**
 * 타이틀바 버튼 `testId` 를 누르고, 첫 실행 확인이 뜨면 그 [실행](마지막 버튼)을 누른다. `done` 이 참이 될 때까지 기다린다.
 */
export async function pressAndConfirm(
  env: Pick<ScenarioEnv, "doc" | "now" | "sleep">,
  testId: string,
  done: () => boolean,
  timeoutMs = 60_000,
): Promise<void> {
  const button = await poll(env, `the ${testId} button`, () => {
    const b = env.doc.querySelector<HTMLButtonElement>(`[data-testid=${testId}]`);
    return b && !b.disabled ? b : undefined;
  });
  button.click();
  let confirmed = false;
  await poll(
    env,
    `${testId} to start`,
    () => {
      if (done()) return true;
      const dialog = env.doc.querySelector(CONFIRM);
      if (dialog && !confirmed) {
        const buttons = dialog.querySelectorAll<HTMLButtonElement>("button");
        buttons[buttons.length - 1]?.click();
        confirmed = true;
      }
      return false;
    },
    timeoutMs,
  );
}

/** 글에서 SGR 이스케이프(`ESC [ … m`)를 걷는다. */
export function withoutSgr(text: string): string {
  return text
    .split("\u001b[")
    .map((part, i) => (i === 0 ? part : part.replace(/^[0-9;]*m/, "")))
    .join("");
}

/** 콘솔 xterm 의 쓰기를 센다: 그린 바이트(콜백이 온 것), 글로 쓴 알림(버린 앞 · 끝남 줄), 마지막으로 바이트를 그린 시각. */
export function watchConsole(term: Terminal, now: () => number) {
  const write = term.write.bind(term);
  const counts = {
    rendered: 0,
    lastAt: undefined as number | undefined,
    first: undefined as number | undefined,
  };
  const notes: string[] = [];
  term.write = (data: string | Uint8Array, callback?: () => void) => {
    if (typeof data === "string" && data !== "" && notes.length < 20)
      // 이스케이프를 걷은 앞 120자.
      notes.push(withoutSgr(data).trim().slice(0, 120));
    const bytes = typeof data === "string" ? 0 : data.length;
    write(data, () => {
      if (bytes > 0) {
        counts.rendered += bytes;
        counts.lastAt = now();
        counts.first ??= counts.lastAt;
      }
      callback?.();
    });
  };
  return {
    counts,
    notes,
    /** 지금까지 쓴 것이 다 그려진 때. */
    flushed: () => new Promise<void>((r) => write("", r)),
    restore: () => void (term.write = write),
  };
}

export async function runFloodScenario(env: ScenarioEnv): Promise<void> {
  const runs = windowRuns();
  await step("run_started", async () => {
    await pressAndConfirm(env, "run-start", () => runs.store.getState().tabs.length > 0);
    await env.report("run_started", { epochMs: env.epochNow() });
  });
  const term = await step("console", () =>
    poll(env, "the run console", () => {
      const c = runs.active();
      return c?.view.isOpen ? c.term : undefined;
    }),
  );
  const watch = watchConsole(term, env.now);
  try {
    await step("flood_frames", async () => {
      await poll(env, "the first console output", () => watch.counts.first !== undefined);
      const timeline: { s: number; bytes: number }[] = [];
      const intervals: number[] = [];
      const blocks = env.watchBlocks();
      const started = env.now();
      const startBytes = watch.counts.rendered;
      env.work.start();
      // 가려진 창은 rAF 를 멈춘다 — 프레임을 250 ms 넘게 기다리면 그 틈을 `stalls` 로 세고 벽시계로 끝낸다(그 회는 측정하는 쪽이
      // 가려짐으로 제외한다).
      const frameOrNothing = () =>
        Promise.race([env.work.frame(), env.sleep(250).then(() => undefined)]);
      let last = (await frameOrNothing()) ?? env.now();
      let stalls = 0;
      let nextMark = 1000;
      while (env.now() - started < FLOOD_MEASURE_MS) {
        const t = await frameOrNothing();
        if (t === undefined) {
          stalls++;
          last = env.now();
          continue;
        }
        intervals.push(t - last);
        last = t;
        if (t - started >= nextMark) {
          timeline.push({ s: nextMark / 1000, bytes: watch.counts.rendered - startBytes });
          nextMark += 1000;
        }
      }
      const work = env.work.take();
      await env.report("flood_frames", {
        measureMs: ms(env.now() - started),
        stalls,
        interval: frameStats(intervals),
        work: frameStats(work),
        blocks: blocks.stop(),
        renderedBytes: watch.counts.rendered - startBytes,
        timeline,
      });
    });
    await step("flood_done", async () => {
      await poll(
        env,
        "the run to exit",
        () => runs.store.getState().tabs.some((t) => t.run.state === "exited"),
        180_000,
      );
      const exitedAt = env.epochNow();
      await poll(
        env,
        "the console to catch up",
        () => watch.counts.lastAt === undefined || env.now() - watch.counts.lastAt > CAUGHT_UP_MS,
        300_000,
      );
      await watch.flushed();
      await env.report("flood_done", {
        epochMs: env.epochNow(),
        exitedSeenEpochMs: exitedAt,
        renderedBytes: watch.counts.rendered,
        notes: watch.notes,
      });
    });
  } finally {
    watch.restore();
  }
}

export async function runDebugEndScenario(env: ScenarioEnv): Promise<void> {
  const debug = windowDebug();
  let endedAt: number | undefined;
  const off = debug.store.subscribe((s) => {
    if (endedAt === undefined && s.sessions.some((x) => x.state === "ended"))
      endedAt = env.epochNow();
  });
  try {
    await step("debug_running", async () => {
      await env.sleep(DEBUG_SETTLE_MS);
      await env.report("debug_starting", { epochMs: env.epochNow() });
      await pressAndConfirm(env, "run-debug", () => debug.store.getState().sessions.length > 0);
      await env.report("debug_running", {
        epochMs: env.epochNow(),
        adapter: debug.store.getState().sessions[0]?.adapter ?? null,
      });
    });
    await step("debug_ended", async () => {
      const running = env.now();
      while (endedAt === undefined && env.now() - running < NATURAL_END_MS) await env.sleep(20);
      if (endedAt !== undefined) {
        await env.report("debug_ended", { how: "exited", epochMs: endedAt });
        return;
      }
      const stopAt = env.epochNow();
      await debug.stop();
      const stopping = env.now();
      while (endedAt === undefined && env.now() - stopping < 30_000) await env.sleep(20);
      if (endedAt === undefined) throw new Error("the session did not end 30 s after stop");
      await env.report("debug_ended", { how: "stopped", epochMs: stopAt, endedEpochMs: endedAt });
    });
  } finally {
    off();
  }
}

/** `run-flood` · `debug-end` 를 돌리고 끝 표시를 남긴다. */
export async function runRunScenario(
  env: ScenarioEnv,
  scenario: "run-flood" | "debug-end",
): Promise<void> {
  try {
    await (scenario === "run-flood" ? runFloodScenario(env) : runDebugEndScenario(env));
    await env.report("scenario_done", {});
  } catch (e) {
    const err = e instanceof StepError ? e : new StepError("?", String(e));
    await env.report("scenario_error", { step: err.step, message: err.message });
  }
}
