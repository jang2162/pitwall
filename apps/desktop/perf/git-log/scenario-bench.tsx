// Git 시나리오 스모크: 앱 안 측정 시나리오(src/perf-scenario.ts 의 `runGitScenario`)를 그대로 앱(App, 프로젝트 창) 위에서 헤드리스 WebKit
// 으로 끝까지 돌린다 — release 번들의 실기 측정(`mise run perf-git`) 전에 시나리오가 실제 DOM(⌘9 → 목록, 더블클릭 → 커밋 탭의 diff,
// 상태바 브랜치)을 찾아 가는지 본다. Host 는 가짜(fake-transport.ts): `git.log` 는 덤프한 스트림, 커밋은 1만 줄 바뀐 파일 하나
// (perf/diff/ten-thousand.ts), 체크아웃은 `checkout_ready` 를 받은 이 페이지가 가짜 HEAD 를 바꾸고 `git.refsChanged` 를 보내는 것이다.
// 잰 값은 헤드리스라 실제 창의 것이 아니다 — 판정은 실기 측정이 한다.
//
// 만들기 · 돌리기: run.mjs 머리 주석에 `PAGE=scenario.html` 을 더한다. 끝나면 남긴 표시 목록이 페이지 제목(`done <JSON>`)에 남는다.

import "../bench.css";

import type { LogChunk } from "@pitwall/rpc-client";
import { createRoot } from "react-dom/client";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../../src/App";
import type { KeymapNative } from "../../src/keymap/native";
import { installFrameWork } from "../../src/perf-frames";
import { runGitScenario } from "../../src/perf-scenario";
import { createThemeStore } from "../../src/theme/theme-store";
import { ThemeStoreProvider } from "../../src/theme/theme-switcher";
import { applyTokens, parseTheme } from "../../src/theme/tokens";
import { input } from "../diff/ten-thousand";

const native: KeymapNative = {
  syncMenu: async () => {},
  onMenuAction: () => () => {},
  openSettings: async () => {},
  closeWindow: async () => {},
};

async function main() {
  const params = new URLSearchParams(location.search);
  const data = params.get("data") ?? "log-100k.json";
  window.__gitLogChunks = (await (await fetch(data)).json()) as LogChunk[];
  window.__gitShowFixture = input("perf/big.txt");
  history.replaceState(null, "", `${location.pathname}?window=project&project=p1`);
  const theme = parseTheme(defaultTheme);
  const store = createThemeStore({
    system: { current: () => "light", subscribe: () => () => {} },
    storage: { read: () => undefined, write: () => {} },
    theme,
    apply: (t, appearance) =>
      applyTokens(document.documentElement.style, t.variants[appearance] ?? t.variants.light!),
  });
  createRoot(document.getElementById("root")!).render(
    <ThemeStoreProvider store={store}>
      <App kind="project" native={native} />
    </ThemeStoreProvider>,
  );
  const events: { event: string; data: Record<string, unknown> }[] = [];
  const work = installFrameWork();
  await runGitScenario({
    doc: document,
    work,
    now: () => performance.now(),
    epochNow: () => performance.timeOrigin + performance.now(),
    sleep: (t) => new Promise((r) => setTimeout(r, t)),
    report: async (event, data) => {
      events.push({ event, data });
      if (event === "checkout_ready") window.__fakeCheckout?.("perf-checkout");
    },
  });
  work.restore();
  document.title = `done ${JSON.stringify(events)}`;
}

main().catch((e: unknown) => {
  document.title = `error ${e instanceof Error ? e.message : String(e)}`;
});
