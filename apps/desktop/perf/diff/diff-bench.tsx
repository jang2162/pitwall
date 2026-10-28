// diff 첫 그림 측정 하네스(docs/design/13-m2-plan.md M2-15 「1만 줄 바뀐 파일의 diff 첫 그림이 release 에서 200 ms 안」): 앱과 같은
// diff 뷰(src/git/diff/diff-view.tsx `buildDiffView`)로 1만 줄 바뀐 파일을 그리고, 칠한 줄이 화면에 들어간 프레임까지를 잰다.
// 파일은 crates/pw-git `tests::diff::release` 의 `diff_of_ten_thousand_changed_lines_in_release` 와 같은 꼴이다 — 2만 줄 중 짝수 줄
// 1만 개를 바꿨다(묶음 1만 개, 가장 잘게 쪼개진 경우). Host 의 몫(`git.diff` 계산)은 그 Rust 시험이 release 로 재고, 여기서는 응답을
// 받은 뒤부터다: `git.diff` · `git.blob` 두 번의 응답을 IPC 처럼 JSON 문자열로 받아 읽고(`JSON.parse`) 모델 · merge 뷰를 만들어
// 그린다. 둘의 합에서 빠진 것은 IPC 전송 자체(Tauri 의 웹뷰 ↔ 코어)다.
//
// 만들기: `pnpm --filter @pitwall/desktop exec vite build perf/diff --config vite.config.ts --outDir <폴더> --emptyOutDir --base ./`
// 그 폴더를 정적 서버로 열고 `index.html?mode=<split|unified>` 를 연다. 끝나면 결과가 `window.__diffBench` 와 페이지 제목
// (`done <JSON>`)에 남는다. 회마다 뷰를 새로 만든다(`RUNS` 회, 첫 회는 JIT 이 차지 않은 값이라 따로 적는다). 헤드리스 브라우저는
// 화면 합성을 거치지 않으므로 실제 창과 같지 않다 — 보고에 어느 브라우저로 쟀는지 함께 적는다.

import "../bench.css";

import type { DiffFile } from "@pitwall/rpc-client";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { buildDiffView, type DiffMode } from "../../src/git/diff/diff-view";
import { applyTokens, parseTheme } from "../../src/theme/tokens";
import { input, LINES } from "./ten-thousand";

const RUNS = 11;

const nextFrame = () => new Promise<number>((r) => requestAnimationFrame(r));

async function measure(wire: ReturnType<typeof input>, mode: DiffMode) {
  const parent = document.getElementById("root")!;
  const started = performance.now();
  const diff = JSON.parse(wire.diff) as { files: DiffFile[] };
  const oldBlob = JSON.parse(wire.old) as { content: { text: string } };
  const newBlob = JSON.parse(wire.new) as { content: { text: string } };
  const built = buildDiffView({
    content: {
      file: diff.files[0]!,
      oldText: oldBlob.content.text,
      newText: newBlob.content.text,
      language: null,
    },
    mode,
    parent,
    phrases: {},
  });
  while (!parent.querySelector(".pw-diff-line")) await nextFrame();
  await nextFrame();
  const elapsed = performance.now() - started;
  const painted = parent.querySelectorAll(".pw-diff-line").length;
  const groups = built.controller.model.groups.length;
  built.destroy();
  return { elapsed, painted, groups };
}

async function main() {
  const theme = parseTheme(defaultTheme);
  applyTokens(document.documentElement.style, theme.variants.light!);
  const mode = (new URLSearchParams(location.search).get("mode") ?? "split") as DiffMode;
  const wire = input();
  await new Promise((r) => setTimeout(r, 500));
  const times: number[] = [];
  let last = { painted: 0, groups: 0 };
  for (let run = 0; run < RUNS; run++) {
    const m = await measure(wire, mode);
    times.push(m.elapsed);
    last = m;
    await new Promise((r) => setTimeout(r, 50));
  }
  const warm = times.slice(1).sort((a, b) => a - b);
  const result = {
    mode,
    lines: LINES,
    changedLines: LINES / 2,
    groups: last.groups,
    runs: RUNS,
    firstMs: Number(times[0]!.toFixed(1)),
    medianMs: Number(warm[Math.floor(warm.length / 2)]!.toFixed(1)),
    maxMs: Number(warm.at(-1)!.toFixed(1)),
    paintedLines: last.painted,
    userAgent: navigator.userAgent,
  };
  (window as unknown as { __diffBench: unknown }).__diffBench = result;
  document.title = `done ${JSON.stringify(result)}`;
}

void main();
