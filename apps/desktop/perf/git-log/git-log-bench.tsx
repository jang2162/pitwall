// Git 로그 측정 하네스(docs/design/13-m2-plan.md M2-14 의 성능 완료 기준 — 첫 그림 500 ms · 스크롤 프레임 95% 16.7 ms): 앱(App,
// 프로젝트 창)을 그대로 띄우고 ⌘9 로 Git 로그 툴 윈도우를 연다. Host 는 메모리 속 가짜(fake-transport.ts)이고 `git.log` 는 pw-git 이
// 합성 10만 커밋 저장소에서 낸 스트림 그대로다(`log_timing --dump`) — 메시지는 IPC 처럼 JSON 으로 오간다.
//
// 만들기 · 돌리기: perf/git-log/run.mjs 머리 주석. 끝나면 결과가 `window.__gitLogBench` 와 페이지 제목(`done <JSON>`)에 남는다.
//
// 잰 것:
// - `open`: ⌘9 keydown 을 넣은 때부터 첫 행(짧은 해시)을 canvas 에 그린 뒤 다음 프레임까지(그 프레임이 화면에 나간다). Host 의 첫 청크
//   시간(pw-git `log_timing`, release)은 빠져 있다 — 06 의 500 ms 판정은 둘을 더한 것이고, 빠진 것은 Tauri IPC 전송 자체다.
// - 스크롤(10만 행을 모두 받은 뒤): `fling` = 처음부터 끝까지 `FLING_FRAMES` 프레임에(한 프레임에 약 50행 — 보이는 행이 매 프레임
//   모두 바뀐다), `steady` = 한 프레임에 2행씩 600 프레임. 값 둘 — `interval` 과 `work`(src/perf-frames.ts 머리 주석). 앱 안 측정
//   시나리오(src/perf-scenario.ts, release 번들)도 같은 부품으로 잰다.
// 헤드리스 브라우저는 화면 합성을 거치지 않으므로 실제 창의 값과 같지 않다 — 보고에 어느 브라우저로 쟀는지 함께 적는다.

import "../bench.css";

import type { LogChunk } from "@pitwall/rpc-client";
import type { DockviewApi } from "dockview-react";
import { createRoot } from "react-dom/client";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../../src/App";
import { installFrameWork, scrollFrames } from "../../src/perf-frames";
import type { KeymapNative } from "../../src/keymap/native";
import { createThemeStore } from "../../src/theme/theme-store";
import { ThemeStoreProvider } from "../../src/theme/theme-switcher";
import { applyTokens, parseTheme } from "../../src/theme/tokens";

const FLING_FRAMES = 2000;
const STEADY_FRAMES = 600;
const ROW = 22;

declare global {
  interface Window {
    __gitLogBench?: unknown;
  }
}

const native: KeymapNative = {
  syncMenu: async () => {},
  onMenuAction: () => () => {},
  openSettings: async () => {},
  closeWindow: async () => {},
};

// 하네스 자신의 프레임은 원래의 rAF 로 잰다. 앱이 거는 콜백(목록의 그리기)은 감싸 그 시간을 적는다(src/perf-frames.ts).
const work = installFrameWork();
const rawFrame = (callback: () => void) => void work.frame().then(callback);
const nextFrame = () => work.frame();

// 첫 행의 짧은 해시를 그린 때(제목은 폭에 따라 잘려 그려진다).
let firstSubject: string | undefined;
let drewFirst: number | undefined;
const fillText = CanvasRenderingContext2D.prototype.fillText;
CanvasRenderingContext2D.prototype.fillText = function (text, x, y, maxWidth) {
  if (drewFirst === undefined && text === firstSubject) drewFirst = performance.now();
  return fillText.call(this, text, x, y, maxWidth);
};

function until(what: string, test: () => boolean, timeoutMs = 60_000): Promise<number> {
  return new Promise((resolve, reject) => {
    const started = performance.now();
    const tick = () => {
      if (test()) return resolve(performance.now());
      if (performance.now() - started > timeoutMs) return reject(new Error(`timed out: ${what}`));
      rawFrame(tick);
    };
    tick();
  });
}

const scroll = (list: HTMLElement, frames: number, step: (frame: number) => number) =>
  scrollFrames(list, frames, step, work);

async function main() {
  const params = new URLSearchParams(location.search);
  const data = params.get("data") ?? "log-100k.json";
  const chunks = (await (await fetch(data)).json()) as LogChunk[];
  window.__gitLogChunks = chunks;
  firstSubject = chunks[0]?.rows[0]?.hash.slice(0, 8);
  const total = chunks.reduce((n, c) => n + c.rows.length, 0);
  history.replaceState(null, "", `${location.pathname}?window=project&project=p1`);
  const theme = parseTheme(defaultTheme);
  const store = createThemeStore({
    system: { current: () => "light", subscribe: () => () => {} },
    storage: { read: () => undefined, write: () => {} },
    theme,
    apply: (t, appearance) =>
      applyTokens(document.documentElement.style, t.variants[appearance] ?? t.variants.light!),
  });
  let dock: DockviewApi | undefined;
  createRoot(document.getElementById("root")!).render(
    <ThemeStoreProvider store={store}>
      <App kind="project" native={native} onDockApi={(api) => (dock = api)} />
    </ThemeStoreProvider>,
  );
  // 프로젝트 창이 뜨고 키맵이 붙을 때까지.
  await until(
    "the project window",
    () => document.querySelector("[data-tool-window=project]") !== null,
  );
  await new Promise((r) => setTimeout(r, 300));
  const started = performance.now();
  document.body.dispatchEvent(
    new KeyboardEvent("keydown", {
      code: "Digit9",
      key: "9",
      metaKey: true,
      bubbles: true,
      cancelable: true,
    }),
  );
  await until("the first row", () => drewFirst !== undefined);
  await nextFrame();
  const open = performance.now() - started;
  const drawn = drewFirst! - started;

  const list = document.querySelector<HTMLElement>("[data-testid=git-log-list]")!;
  const loaded = await until(
    "every row",
    () => list.getAttribute("aria-rowcount") === String(total),
  );
  const small = { width: list.clientWidth, height: list.clientHeight };
  // 스크롤은 툴 윈도우를 키운 모양으로 잰다: 하단 dock 을 높이고 왼쪽 · 오른쪽 dock 을 접는다(가운데로 끌어 크게 본 것과 비슷한 폭).
  dock?.getEdgeGroup("left")?.collapse();
  dock?.getEdgeGroup("right")?.collapse();
  dock?.getPanel("git")?.group.api.setSize({ height: 600 });
  await nextFrame();
  await nextFrame();
  const height = total * ROW - list.clientHeight;
  const fling = await scroll(list, FLING_FRAMES, (f) =>
    Math.round((height * (f + 1)) / FLING_FRAMES),
  );
  list.scrollTop = 0;
  await nextFrame();
  const steady = await scroll(list, STEADY_FRAMES, (f) => (f + 1) * 2 * ROW);
  const idle = await scroll(list, STEADY_FRAMES, () => (STEADY_FRAMES + 1) * 2 * ROW);
  const result = {
    userAgent: navigator.userAgent,
    rows: total,
    openViewport: small,
    scrollViewport: { width: list.clientWidth, height: list.clientHeight },
    openMs: Math.round(open * 10) / 10,
    firstDrawMs: Math.round(drawn * 10) / 10,
    allRowsMs: Math.round((loaded - started) * 10) / 10,
    fling,
    steady,
    idle,
  };
  window.__gitLogBench = result;
  document.title = `done ${JSON.stringify(result)}`;
}

main().catch((e: unknown) => {
  document.title = `error ${e instanceof Error ? e.message : String(e)}`;
});
