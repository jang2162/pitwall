// 에디터 첫 화면 강조 측정 하네스(docs/design/12-m1-plan.md M1-17 「5,000줄 파일을 열고 화면 강조가 release 에서 150 ms 안」):
// 앱과 같은 확장 목록(EDITOR_FEATURES — 강조 포함)으로 EditorView 를 만들고, 강조가 칠해진 프레임까지를 잰다. 구문 분석 서비스
// 대신 미리 계산한 구간을 돌려주는 가짜 세션 RPC 를 쓴다 — 요청 · 응답은 IPC 처럼 JSON 으로 한 번씩 오간다. 서버 몫(JSON 읽기 ·
// 파싱 · 강조 · JSON 쓰기)은 crates/pw-syntax tests/perf.rs 의 `measure_editor_first_screen` 이 release 로 잰다. 둘의 합에서
// 빠진 것은 IPC 전송 자체(Tauri 의 웹뷰 ↔ 코어)다.
//
// 입력: `measure_editor_first_screen` 을 `PW_HIGHLIGHT_BENCH_OUT=<빌드 폴더>/input.json` 으로 돌려 만든다(텍스트와 문서 전체의 구간).
// 만들기: `pnpm --filter @pitwall/desktop exec vite build perf/highlight --config vite.config.ts --outDir <폴더> --emptyOutDir --base ./`
// 그 폴더를 정적 서버로 열고 `index.html?mode=<mode>` 를 연다. 끝나면 결과가 `window.__highlightBench` 와 페이지 제목
// (`done <JSON>`)에 남는다.
//
// - `mode=highlight`(기본): 뷰를 만들기 시작한 때부터 첫 강조 구간(`.pw-syntax-*`)이 화면에 들어간 프레임까지.
// - `mode=plain`: 같은 문서를 강조 없이(언어 없음) 뷰를 만든 커밋을 담은 프레임까지(src/perf-mark.tsx 와 같은 두 번째 rAF).
// 회마다 뷰를 새로 만든다(`RUNS` 회, 첫 회는 JIT 이 차지 않은 값이라 따로 적는다). 헤드리스 브라우저는 화면 합성을 거치지 않으므로
// 실제 창과 같지 않다 — 보고에 어느 브라우저로 쟀는지 함께 적는다.

import "../bench.css";

import { Text } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import type { HighlightSpan, Judgment } from "@pitwall/rpc-client";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { type EditorFeatureContext, editorExtensions } from "../../src/editor/extensions";
import { createSyntaxSessions, type SyntaxSource } from "../../src/editor/syntax-sessions";
import { applyTokens, parseTheme } from "../../src/theme/tokens";

const RUNS = 11;

const judgment: Judgment = {
  binary: false,
  encoding: "UTF-8",
  lossy: false,
  bom: null,
  lineEnding: "lf",
  longLines: false,
  sizeClass: "editable",
  editable: true,
  highlight: true,
};

/** IPC 처럼 JSON 으로 한 번 오간다. */
const wire = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;

const nextFrame = () => new Promise<number>((r) => requestAnimationFrame(r));

async function measure(text: string, spans: HighlightSpan[], highlight: boolean, run: number) {
  const source: SyntaxSource = {
    open: async (params) => (wire(params), wire({ grammar: "rust", off: null })),
    edit: async (params) => (wire(params), wire({ off: null })),
    highlights: async (params) => {
      const { from, to } = wire(params).range;
      return wire({
        version: 0,
        spans: spans.filter((s) => s.to > from && s.from < to),
        off: null,
      });
    },
    close: async () => null,
  };
  const parent = document.getElementById("root")!;
  const started = performance.now();
  const doc = Text.of(text.split("\n"));
  const syntax = createSyntaxSessions(source, {
    onChange: (_path, listener) => (listener({ kind: "reset", text: doc }), () => {}),
  });
  const context: EditorFeatureContext = {
    panelId: `bench-${run}`,
    path: `/bench/${run}.rs`,
    judgment,
    readOnly: false,
    language: highlight ? "Rust" : null,
    appearance: "light",
    onCursor: () => {},
    history: { undo: () => false, redo: () => false },
    font: {},
    createFindPanel: () => ({ dom: document.createElement("div"), top: true }),
    syntax,
  };
  const view = new EditorView({ parent, doc, extensions: editorExtensions(context) });
  if (highlight) while (!view.contentDOM.querySelector(".pw-syntax-keyword")) await nextFrame();
  else await nextFrame();
  await nextFrame();
  const elapsed = performance.now() - started;
  const painted = view.contentDOM.querySelectorAll("[class*=pw-syntax-]").length;
  view.destroy();
  return { elapsed, painted };
}

async function main() {
  const theme = parseTheme(defaultTheme);
  applyTokens(document.documentElement.style, theme.variants.light!);
  const input = (await (await fetch("./input.json")).json()) as {
    text: string;
    spans: HighlightSpan[];
    serverMs: number;
  };
  const mode = new URLSearchParams(location.search).get("mode") ?? "highlight";
  await new Promise((r) => setTimeout(r, 500));
  const times: number[] = [];
  let painted = 0;
  for (let run = 0; run < RUNS; run++) {
    const m = await measure(input.text, input.spans, mode === "highlight", run);
    times.push(m.elapsed);
    painted = m.painted;
    await new Promise((r) => setTimeout(r, 50));
  }
  const warm = times.slice(1).sort((a, b) => a - b);
  const median = warm[Math.floor(warm.length / 2)]!;
  const result = {
    mode,
    lines: input.text.split("\n").length,
    runs: RUNS,
    firstMs: Number(times[0]!.toFixed(1)),
    medianMs: Number(median.toFixed(1)),
    maxMs: Number(warm.at(-1)!.toFixed(1)),
    paintedSpans: painted,
    serverMs: Number(input.serverMs.toFixed(1)),
    userAgent: navigator.userAgent,
  };
  (window as unknown as { __highlightBench: unknown }).__highlightBench = result;
  document.title = `done ${JSON.stringify(result)}`;
}

void main();
