// 파일로 이동 측정 하네스(docs/design/12-m1-plan.md M1-19 「합성 10만 파일 프로젝트에서 ⌘⇧O 에 타이핑할 때 키 입력 → 결과 그림 p95
// 50 ms」, M1-26 이 같은 방법으로 다시 잰다): 앱(App, 프로젝트 창)을 그대로 띄우고 ⌘⇧O 팝업의 입력 칸에 글자를 하나씩 넣어, 입력
// 이벤트를 넣은 때부터 그 질의의 결과가 목록에 들어온 때(`commit`)와 그것을 담은 프레임(`frame` — 다음 requestAnimationFrame)까지를
// 잰다. Host 는 메모리 속 가짜(fake-transport.ts)라 `search.files` 에 결과 50개(팝업의 상한)를 바로 답한다 — 메시지는 IPC 처럼 JSON
// 으로 오간다. Host 의 `search.files` 시간(합성 10만 파일, release)은 crates/pw-fs/examples/search_files_bench.rs 가 따로 잰다.
// 키 입력 → 결과 그림은 둘을 더한 것이고, 빠진 것은 Tauri IPC 전송 자체(웹뷰 ↔ 코어 한 번 왕복)다.
//
// 만들기: `pnpm --filter @pitwall/desktop exec vite build perf/goto-file --config perf/goto-file/vite.config.ts --outDir <폴더>
// --emptyOutDir --base ./` (release 처럼 React 프로덕션 빌드다). 그 폴더를 정적 서버로 열고(`python3 -m http.server -d <폴더>`)
// 헤드리스 WebKit 에서 `index.html?runs=<회수>` 를 연다 — 앱 창을 띄우지 않는다. 끝나면 결과가 `window.__gotoFileBench` 와 페이지
// 제목(`done <JSON>`)에 남는다.
//
// 질의는 search_files_bench.rs 의 기본 질의와 같은 낱말의 앞 1 글자 · 2 글자 · …(키 입력 하나 = 질의 하나)다. 다음 글자는 앞
// 글자의 결과가 그려진 뒤에 넣는다. 첫 회는 JIT 이 차지 않은 값이라 따로 적는다. 헤드리스 브라우저는 화면 합성을 거치지 않으므로
// 실제 창의 값과 같지 않다 — 보고에 어느 브라우저로 쟀는지 함께 적는다.

import "../bench.css";

import { createRoot } from "react-dom/client";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../../src/App";
import type { KeymapNative } from "../../src/keymap/native";
import { createThemeStore } from "../../src/theme/theme-store";
import { ThemeStoreProvider } from "../../src/theme/theme-switcher";
import { applyTokens, parseTheme } from "../../src/theme/tokens";

const QUERIES = [
  "handler",
  "config.ts",
  "src/router",
  "parser_lexer",
  "editor buffer",
  "pkgsrvc",
  "tokenrs",
  "watch-3/src/event",
];

declare global {
  interface Window {
    __gotoFileBench?: unknown;
  }
}

const native: KeymapNative = {
  syncMenu: async () => {},
  onMenuAction: () => () => {},
  openSettings: async () => {},
  closeWindow: async () => {},
};

const nextFrame = () => new Promise<number>((r) => requestAnimationFrame(r));

/** `test` 가 참이 될 때까지 DOM 변화를 지켜본다. 참이 된 시각을 돌려준다. */
function until(test: () => boolean, timeoutMs = 10_000): Promise<number> {
  if (test()) return Promise.resolve(performance.now());
  return new Promise((resolve, reject) => {
    const observer = new MutationObserver(() => {
      if (!test()) return;
      observer.disconnect();
      clearTimeout(timer);
      resolve(performance.now());
    });
    const timer = setTimeout(() => {
      observer.disconnect();
      reject(new Error("timed out"));
    }, timeoutMs);
    observer.observe(document.body, { childList: true, subtree: true, attributes: true });
  });
}

function percentile(sorted: number[], p: number): number {
  return sorted[Math.min(sorted.length - 1, Math.ceil(sorted.length * p) - 1)]!;
}

function stats(samples: number[]) {
  const sorted = [...samples].sort((a, b) => a - b);
  const round = (x: number) => Math.round(x * 100) / 100;
  return {
    n: sorted.length,
    p50: round(percentile(sorted, 0.5)),
    p95: round(percentile(sorted, 0.95)),
    max: round(sorted.at(-1)!),
  };
}

const input = () =>
  document.querySelector<HTMLInputElement>("[data-search-everywhere] [cmdk-input]")!;
const firstValue = () =>
  document.querySelector("[data-search-everywhere] [cmdk-item]")?.getAttribute("data-value");
const setValue = (value: string) => {
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input(), value);
  input().dispatchEvent(new Event("input", { bubbles: true }));
};

async function main() {
  const runs = Number(new URLSearchParams(location.search).get("runs") ?? 5);
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
  // 찾는 곳은 파일 트리의 루트다 — 루트를 읽을 때까지 기다린다.
  await until(() => document.querySelector('[data-path="/work/demo/src"]') !== null);
  document.body.dispatchEvent(
    new KeyboardEvent("keydown", {
      code: "KeyO",
      key: "O",
      metaKey: true,
      shiftKey: true,
      bubbles: true,
      cancelable: true,
    }),
  );
  await until(() => document.querySelector('[data-search-everywhere="files"]') !== null);

  const commit: number[][] = [];
  const frame: number[][] = [];
  for (let run = 0; run < runs; run++) {
    commit.push([]);
    frame.push([]);
    for (const word of QUERIES) {
      for (let n = 1; n <= word.length; n++) {
        const query = word.slice(0, n);
        const want = `file:/work/demo/apps/pkg-0/src/module-0/${query.replace(/\s+/g, "_")}_0.rs`;
        const started = performance.now();
        setValue(query);
        const committed = await until(() => firstValue() === want);
        await nextFrame();
        const painted = performance.now();
        commit[run]!.push(committed - started);
        frame[run]!.push(painted - started);
      }
      setValue("");
      await until(() => firstValue() === undefined);
      await nextFrame();
    }
  }
  const warm = (xs: number[][]) => xs.slice(1).flat();
  const result = {
    userAgent: navigator.userAgent,
    runs,
    keystrokesPerRun: commit[0]!.length,
    first: { commit: stats(commit[0]!), frame: stats(frame[0]!) },
    warm: runs > 1 ? { commit: stats(warm(commit)), frame: stats(warm(frame)) } : null,
  };
  window.__gotoFileBench = result;
  document.title = `done ${JSON.stringify(result)}`;
}

main().catch((e: unknown) => {
  document.title = `failed ${String(e)}`;
});
