// 커밋 툴 윈도우 목록 스크롤 측정 하네스(docs/design/13-m2-plan.md M2-16 완료 기준 — 1만 개 바뀐 파일의 목록 스크롤이 M2-14 와 같은
// 판정 — 화면 갱신을 놓치지 않은 프레임 95% · 간격 중앙값 17 ms 이하, release 번들의 판정은 `crates/pw-perf`): 앱과 같은 컴포넌트(CommitView)를 창의 git 상태 저장소에 1만 항목을 넣고 그린다(스테이징됨 2,500 · 변경
// 5,000 · 버전 관리 안 됨 2,500 — 그룹 머리 넷과 stash 머리를 합쳐 1만 5행). Host 없이 메모리에서 돈다. 방법은 파일 트리 하네스
// (perf/file-tree-bench.tsx, M1-14)와 같다: 화면 갱신마다(rAF) 스크롤을 옮기며 프레임 간격(`interval`)과 스크롤 한 번의 처리
// (`work` — 가상화의 scroll 처리 + React 그리기 + 강제 배치)를 잰다.
//
// 만들기: `pnpm --filter @pitwall/desktop exec vite build perf/commit-list --config vite.config.ts --outDir <폴더> --emptyOutDir --base ./`
// 그 폴더를 정적 서버로 열고 `node apps/desktop/perf/run-headless.cjs webkit <url>/index.html?mode=<mode> __commitBench <playwright-core>`.
// `mode` 는 `steady`(기본, 한 프레임에 2행 · 600 프레임) · `fling`(1,500 프레임에 처음부터 끝까지) · `idle`(굴리지 않는 기준선).
// 헤드리스 브라우저는 화면 합성을 거치지 않으므로 실제 창의 값과 같지 않다 — 보고에 어느 브라우저 · 어느 모드로 쟀는지 함께 적는다.

import "../bench.css";

import { LocalizationProvider } from "@fluent/react";
import type { GitStatusEntry } from "@pitwall/rpc-client";
import { ActionRegistry } from "@pitwall/keymap";
import { createRoot } from "react-dom/client";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { createCommitOps } from "../../src/git/commit/commit-ops";
import { commitFeature } from "../../src/git/commit/commit-state";
import { CommitView } from "../../src/git/commit/commit-tool-window";
import { gitCommitStrings } from "../../src/git/commit/strings";
import { windowGitStatus } from "../../src/git/git-status";
import { bundledLocalization, bundledTranslator } from "../../src/i18n/testing";
import { applyTokens, parseTheme } from "../../src/theme/tokens";

const FILES = 10_000;

declare global {
  interface Window {
    __commitBench?: unknown;
  }
}

function entries(): GitStatusEntry[] {
  return Array.from({ length: FILES }, (_, i) => {
    const path = `module_${String(i % 100).padStart(3, "0")}/file_${String(i).padStart(5, "0")}.rs`;
    const base = { path, origPath: null, staged: null, unstaged: null, conflict: null };
    if (i % 4 === 0) return { ...base, staged: "modified" };
    if (i % 4 === 3) return { ...base, unstaged: "untracked" };
    return { ...base, unstaged: i % 7 === 0 ? "deleted" : "modified" };
  });
}

function percentile(sorted: number[], p: number): number {
  return sorted[Math.min(sorted.length - 1, Math.floor((sorted.length * p) / 100))]!;
}

async function main() {
  const theme = parseTheme(defaultTheme);
  applyTokens(document.documentElement.style, theme.variants.light ?? theme.variants.dark!);
  windowGitStatus().apply({ workTree: "/bench", entries: entries(), unsupported: null });
  const strings = gitCommitStrings(bundledTranslator("ko"));
  const ops = createCommitOps({
    client: () => undefined,
    projectId: "bench",
    worktree: () => undefined,
    confirm: async () => false,
    askName: async () => undefined,
    notify: () => {},
    strings: () => strings,
  });
  commitFeature.setState({
    current: {
      ops: {
        ...ops,
        stashList: async () => undefined,
        template: async () => ({ template: undefined, commentChar: "#" }),
      },
      registry: new ActionRegistry(),
      strings: () => strings,
      notify: () => {},
      history: () => [],
      remember: () => {},
      hasStaged: () => true,
    },
  });
  createRoot(document.getElementById("root")!).render(
    <LocalizationProvider l10n={bundledLocalization("ko")}>
      <CommitView />
    </LocalizationProvider>,
  );
  await new Promise((r) => setTimeout(r, 1000));
  const scroller = document.querySelector<HTMLElement>("[data-commit-list]")!;
  const mode = new URLSearchParams(location.search).get("mode") ?? "steady";
  const frames = mode === "fling" ? 1500 : 600;
  const max = scroller.scrollHeight - scroller.clientHeight;
  const step = mode === "fling" ? max / frames : mode === "idle" ? 0 : 44;

  const work: number[] = [];
  let began = 0;
  scroller.addEventListener("scroll", () => (began = performance.now()), { capture: true });
  scroller.addEventListener("scroll", () => {
    void scroller.firstElementChild!.getBoundingClientRect();
    work.push(performance.now() - began);
  });

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
    files: FILES,
    frames: intervals.length,
    domRows: document.querySelectorAll("[role=treeitem]").length,
    scrolledTo: scroller.scrollTop,
    scrollHeight: scroller.scrollHeight,
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
  window.__commitBench = result;
  document.title = "done";
  console.info("[commit-bench]", JSON.stringify(result));
}

void main();
