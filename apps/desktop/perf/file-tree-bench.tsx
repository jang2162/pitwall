// 파일 트리 스크롤 측정 하네스(docs/design/12-m1-plan.md M1-14 · M1-26): 펼친 행 5만 개의 트리를 앱과 같은 컴포넌트(FileTree)로
// 그리고, 화면 갱신마다(requestAnimationFrame) 스크롤을 옮기며 프레임 간격을 잰다. Host 없이 메모리 목록으로 돈다.
//
// 만들기: `pnpm --filter @pitwall/desktop exec vite build perf --config vite.config.ts --outDir <폴더> --emptyOutDir --base ./`
// 그 폴더를 정적 서버로 열고(`python3 -m http.server -d <폴더>`) 브라우저 · 웹뷰에서 `index.html?mode=<mode>` 를 연다.
// 끝나면 결과가 `window.__treeBench` 와 페이지 제목(`done`)에 남는다.
//
// - `mode=steady`(기본): 한 프레임에 2행(44 px)씩 600 프레임 — 손으로 굴리는 속도.
// - `mode=fling`: 1,500 프레임 동안 처음부터 끝까지(한 프레임에 약 33행) — 모든 행이 매 프레임 바뀌는 최악.
// - `mode=idle`: 같은 트리를 굴리지 않고 600 프레임 — 그 브라우저의 rAF 간격 기준선.
// 두 값을 낸다.
// - `interval`: rAF 시각의 차이. 16.7 ms 를 넘은 간격이 놓친 프레임이다. WebKit 은 시각을 1 ms 로 잘라 60 Hz 한 프레임이 16 · 17 로
//   번갈아 읽힌다 — `idle` 과 같은 분포면 스크롤이 프레임을 놓치지 않은 것이고, `over18` (한 단위 넘게 늦은 간격)의 비율을 함께 본다.
// - `work`: 스크롤 한 번의 처리(가상화의 scroll 처리 + React 그리기 + 강제 배치)에 든 시간. 프레임 예산(16.7 ms) 중 트리가 쓴 몫이다.
// 헤드리스 브라우저는 화면 합성(compositor)을 거치지 않으므로 실제 창의 값과 같지 않다 — 보고에 어느 브라우저 · 어느 모드로 쟀는지
// 함께 적는다.

import "./bench.css";

import { LocalizationProvider } from "@fluent/react";
import type { FsEntry } from "@pitwall/rpc-client";
import { createRoot } from "react-dom/client";

import defaultTheme from "../../../extensions/theme-default/themes/default.json";
import { FileTree } from "../src/file-tree/file-tree";
import { FileTreeModel } from "../src/file-tree/tree-model";
import { bundledLocalization } from "../src/i18n/testing";
import { applyTokens, parseTheme } from "../src/theme/tokens";

const FOLDERS = 100;
const FILES_PER_FOLDER = 499;
const EXTENSIONS = ["rs", "ts", "tsx", "json", "md", "toml"];

declare global {
  interface Window {
    __treeBench?: unknown;
  }
}

function entry(name: string, kind: "file" | "dir", i: number): FsEntry {
  return {
    name,
    kind,
    target: null,
    outside: false,
    size: 0,
    mtimeMs: null,
    ignored: i % 17 === 0,
  };
}

const model = new FileTreeModel({
  list: async (path) => {
    if (path === "/bench")
      return {
        path,
        entries: Array.from({ length: FOLDERS }, (_, i) =>
          entry(`module_${String(i).padStart(3, "0")}`, "dir", i),
        ),
      };
    return {
      path,
      entries: Array.from({ length: FILES_PER_FOLDER }, (_, i) =>
        entry(
          `file_${String(i).padStart(4, "0")}_name.${EXTENSIONS[i % EXTENSIONS.length]}`,
          "file",
          i,
        ),
      ),
    };
  },
  dimmed: async (paths) => paths.map((_, i) => i % 23 === 0),
});

function percentile(sorted: number[], p: number): number {
  return sorted[Math.min(sorted.length - 1, Math.floor((sorted.length * p) / 100))]!;
}

async function main() {
  const theme = parseTheme(defaultTheme);
  applyTokens(document.documentElement.style, theme.variants.light ?? theme.variants.dark!);
  await model.setRoot("/bench");
  await Promise.all(
    model
      .rows()
      .filter((r) => r.depth === 1)
      .map((r) => model.expand(r.entry.path)),
  );
  const rows = model.rows().length;
  createRoot(document.getElementById("root")!).render(
    <LocalizationProvider l10n={bundledLocalization("ko")}>
      <FileTree model={model} handlers={{ open() {}, copy() {} }} platform="mac" />
    </LocalizationProvider>,
  );
  // 첫 그림과 글꼴이 자리 잡을 때까지 기다린다.
  await new Promise((r) => setTimeout(r, 1000));
  const scroller = document.querySelector<HTMLElement>("[role=tree]")!;
  const mode = new URLSearchParams(location.search).get("mode") ?? "steady";
  const frames = mode === "fling" ? 1500 : 600;
  const max = scroller.scrollHeight - scroller.clientHeight;
  const step = mode === "fling" ? max / frames : mode === "idle" ? 0 : 44;

  // 가상화가 단 scroll 처리기 앞뒤에서 잰다(같은 요소의 처리기는 단 순서대로 돈다). 뒤쪽에서 배치를 강제해 그 값도 넣는다.
  const work: number[] = [];
  let began = 0;
  const before = () => (began = performance.now());
  const after = () => {
    void scroller.firstElementChild!.getBoundingClientRect();
    work.push(performance.now() - began);
  };
  scroller.addEventListener("scroll", before, { capture: true });
  scroller.addEventListener("scroll", after);

  const intervals: number[] = [];
  await new Promise<void>((resolve) => {
    let last: number | undefined;
    let n = 0;
    const tick = (now: number) => {
      if (last !== undefined) intervals.push(now - last);
      last = now;
      if (n++ >= frames) return resolve();
      scroller.scrollTop = Math.min(max, scroller.scrollTop + step);
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  });

  const sorted = [...intervals].sort((a, b) => a - b);
  const worked = [...work].sort((a, b) => a - b);
  const share = (pass: (d: number) => boolean) => intervals.filter(pass).length / intervals.length;
  const result = {
    mode,
    rows,
    frames: intervals.length,
    domRows: document.querySelectorAll("[role=treeitem]").length,
    scrolledTo: scroller.scrollTop,
    interval: {
      within16_7: share((d) => d <= 16.7),
      over18: share((d) => d > 18),
      p50: percentile(sorted, 50),
      p95: percentile(sorted, 95),
      p99: percentile(sorted, 99),
      max: sorted.at(-1),
    },
    work: worked.length
      ? {
          count: worked.length,
          within16_7: worked.filter((d) => d <= 16.7).length / worked.length,
          p50: percentile(worked, 50),
          p95: percentile(worked, 95),
          max: worked.at(-1),
        }
      : null,
    userAgent: navigator.userAgent,
  };
  window.__treeBench = result;
  document.title = "done";
  console.info("[tree-bench]", JSON.stringify(result));
}

void main();
