// 프레임 시간 측정의 공용 부품: 표본의 분포(`frameStats`)와 프레임마다 스크롤 위치를 옮기며 잰 값(`scrollFrames`). Git 로그 하네스
// (perf/git-log/git-log-bench.tsx, 헤드리스)와 앱 안 측정 시나리오(src/perf-scenario.ts, release 번들)가 같은 방법으로 잰다 — file-tree
// 하네스(perf/file-tree-bench.tsx, M1-14)의 방법이다.
//
// 값 둘: `interval`(rAF 시각의 차이 — 16.7 ms 를 넘으면 놓친 프레임)과 `work`(앱이 그 프레임에 건 rAF 콜백이 든 시간). `work` 는
// `installFrameWork` 가 `window.requestAnimationFrame` 을 감싸 모은다 — 측정하는 쪽의 프레임은 감싸기 전의 rAF(`frame`)로 잰다.

export interface FrameStats {
  n: number;
  p50: number;
  p95: number;
  p99: number;
  max: number;
  /** 16.7 ms 안인 표본의 몫(0~1). 참고 — 판정은 `withinVsync` 다(아래). */
  within16_7: number;
  /** 18 ms 를 넘는 표본의 몫. */
  over18: number;
  /** 디스플레이 갱신 한 번의 간격으로 보는 값: 간격의 중앙값(ms). */
  vsyncMs: number;
  /**
   * 갱신을 놓치지 않은 표본의 몫: 간격이 `vsyncMs` 의 1.5배 이하. WebKit 의 rAF 시각은 1 ms 단위라 60 Hz 의 제때 그린 프레임도
   * 16 · 17 ms 로 갈리고 17 은 `within16_7` 에서 빠진다 — 그래서 60 Hz 화면에서 `within16_7` 은 렌더와 상관없이 0.5 언저리가
   * 위끝이다. 그래서 스크롤 판정(13-m2-plan M2-14 · 16)은 이것 ≥ 0.95 이고 `vsyncMs` ≤ 17 ms(60 fps 이상)다 — `within16_7` 은
   * 참고로 싣는다(하네스 `crates/pw-perf/src/git.rs`).
   */
  withinVsync: number;
}

/** 오름차순 `sorted` 의 `p` 분위(가장 가까운 순위). */
export function percentile(sorted: readonly number[], p: number): number {
  return sorted[Math.min(sorted.length - 1, Math.max(0, Math.ceil(sorted.length * p) - 1))]!;
}

const round = (x: number) => Math.round(x * 100) / 100;

/** 갱신 한 번으로 보는 간격의 폭(`vsyncMs` 의 배수). 한 번을 놓치면 간격이 두 배가 되므로 그 사이의 가운데다. */
export const VSYNC_SLACK = 1.5;

/** 표본이 없으면 `n` 이 0 이고 나머지도 0 이다(그 구간에 앱이 그리지 않았다). */
export function frameStats(samples: readonly number[]): FrameStats {
  if (samples.length === 0)
    return {
      n: 0,
      p50: 0,
      p95: 0,
      p99: 0,
      max: 0,
      within16_7: 0,
      over18: 0,
      vsyncMs: 0,
      withinVsync: 0,
    };
  const sorted = [...samples].sort((a, b) => a - b);
  const share = (ok: (d: number) => boolean) => round(sorted.filter(ok).length / sorted.length);
  const vsync = percentile(sorted, 0.5);
  return {
    n: sorted.length,
    p50: round(percentile(sorted, 0.5)),
    p95: round(percentile(sorted, 0.95)),
    p99: round(percentile(sorted, 0.99)),
    max: round(sorted.at(-1)!),
    within16_7: share((d) => d <= 16.7),
    over18: share((d) => d > 18),
    vsyncMs: round(vsync),
    withinVsync: share((d) => d <= vsync * VSYNC_SLACK),
  };
}

/** 앱의 rAF 콜백 시간을 모으는 손잡이. `take` 는 그동안 모은 것을 돌려주고 모으기를 멈춘다. */
export interface FrameWork {
  /** 감싸기 전의 rAF — 측정하는 쪽의 프레임. */
  frame(): Promise<number>;
  start(): void;
  take(): number[];
  restore(): void;
}

export function installFrameWork(win: Window = window, now = () => performance.now()): FrameWork {
  const original = win.requestAnimationFrame;
  const raw = original.bind(win);
  let work: number[] | undefined;
  win.requestAnimationFrame = (callback) =>
    raw((t) => {
      const started = now();
      callback(t);
      work?.push(now() - started);
    });
  return {
    frame: () => new Promise<number>((r) => raw(r)),
    start: () => void (work = []),
    take: () => {
      const taken = work ?? [];
      work = undefined;
      return taken;
    },
    restore: () => void (win.requestAnimationFrame = original),
  };
}

export interface ScrollResult {
  interval: FrameStats;
  work: FrameStats;
}

/** `frames` 프레임 동안 프레임마다 `list.scrollTop = step(프레임)` 하고 잰다. */
export async function scrollFrames(
  list: HTMLElement,
  frames: number,
  step: (frame: number) => number,
  work: FrameWork,
): Promise<ScrollResult> {
  const intervals: number[] = [];
  work.start();
  let last = await work.frame();
  for (let frame = 0; frame < frames; frame++) {
    list.scrollTop = step(frame);
    list.dispatchEvent(new Event("scroll"));
    const now = await work.frame();
    intervals.push(now - last);
    last = now;
  }
  const worked = work.take();
  return { interval: frameStats(intervals), work: frameStats(worked) };
}
