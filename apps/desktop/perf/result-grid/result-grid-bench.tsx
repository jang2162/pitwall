// 결과 그리드 측정 하네스(docs/design/14-m3-plan.md M3-16 의 완료 기준 — 실행부터 첫 행이 그려진 프레임까지 500 ms, 10만 행 연속
// 스크롤과 스트리밍 중 스크롤이 06 「스크롤 60fps」 판정, 상한에서 「상한에서 멈춤」, 라이트 ↔ 다크 전환에 다시 칠함). 결과
// 화면(`ResultView`)을 그대로 띄우고, Host 는 메모리 속 가짜다 — `db.query.execute` 스트림을 M3-05 의 꼴(`accepted` → `started` →
// `columns` → `rows` 500행씩 … → `done`)으로 보내고, 메시지는 IPC 처럼 JSON 문자열로 오간다(memory transport). 청크의 JSON 은
// 재기 전에 만들어 둔다(Host 쪽 직렬화는 Rust 몫 — M3-05 의 첫 청크 50 ms 완료 기준이 잰다). 청크 사이에 `CHUNK_GAP_MS` 씩 쉰다
// (Host 가 서버 커서에서 이어 읽는 시간 — 스트리밍 중 스크롤을 잴 만큼 길게).
//
// 만들기 · 돌리기: perf/result-grid/run.mjs 머리 주석. 끝나면 결과가 `window.__resultGridBench` 와 페이지 제목(`done <JSON>`)에 남는다.
//
// 잰 것(헤드리스 — 실제 창은 M3-21 이 release 번들에서 다시 잰다):
// - `firstFrameMs`: 실행(스트림 요청)부터 첫 행의 셀 글을 canvas 에 그린 뒤 다음 프레임까지.
// - `allRowsMs`: 실행부터 10만 행을 다 받아 상태 줄이 끝난 때까지.
// - `fling`: 10만 행을 다 받은 뒤 처음부터 끝까지 `FLING_FRAMES` 프레임에(한 프레임에 약 50행). `streaming`: 새 실행을 걸자마자
//   스트림이 끝날 때까지 프레임마다 아래로 `STREAM_STEP_ROWS` 행씩(받은 끝을 넘지 않게) 스크롤. 값은 src/perf-frames.ts 의
//   `interval`(판정: `withinVsync` ≥ 0.95 · `vsyncMs` ≤ 17) · `work`.
// - `capped`: 상한 + 1 행(Host 가 상한에서 커서를 버리고 `truncated` 로 끝냄)의 상태 줄 글.
// - `theme`: 라이트 → 다크 전환 앞뒤에 셀 배경 화소의 색.
// - `hostile`: `<img onerror>` · `<script>` 가 든 값을 그린 뒤 문서에 img · script 가 생겼는지, 불러온 스크립트 청크 이름(오버레이
//   편집기 청크를 불러오지 않는다). 셀 상세 패널이 글자로만 그리는 것은 창 시험(result-view-window.test.tsx)이 본다.
// - `stubErrors`: 번들에서 뺀 `marked` · 캐러셀의 던지는 자리(src/db/grid/glide-stubs/)가 불린 횟수(0 이어야 한다).

import "../bench.css";

import { LocalizationProvider } from "@fluent/react";
import {
  createMemoryTransportPair,
  type DbQueryEvent,
  type DbValue,
  RpcClient,
} from "@pitwall/rpc-client";
import { createRoot } from "react-dom/client";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { QueryRun } from "../../src/db/grid/query-stream";
import type { ResultBuffer } from "../../src/db/grid/result-buffer";
import { ResultView } from "../../src/db/grid/result-view";
import { bundledLocalization } from "../../src/i18n/testing";
import { installFrameWork, scrollFrames } from "../../src/perf-frames";
import { createThemeStore } from "../../src/theme/theme-store";
import { ThemeStoreProvider } from "../../src/theme/theme-switcher";
import { applyTokens, parseTheme } from "../../src/theme/tokens";

const ROWS = 100_000;
const COLS = 10;
const CHUNK = 500;
const CHUNK_GAP_MS = 4;
const FLING_FRAMES = 2000;
const STREAM_STEP_ROWS = 20;
const ROW_HEIGHT = 24;

declare global {
  interface Window {
    __resultGridBench?: unknown;
  }
}

const work = installFrameWork();

// 던지는 자리(src/db/grid/glide-stubs/)가 불렸는지: 그 오류가 잡히지 않고 올라오면 센다.
let stubErrors = 0;
const countStub = (message: string) => {
  if (/is not bundled/.test(message)) stubErrors++;
};
window.addEventListener("error", (e) => countStub(String(e.message)));
window.addEventListener("unhandledrejection", (e) =>
  countStub(String((e.reason as Error)?.message)),
);
const consoleError = console.error;
console.error = (...args: unknown[]) => {
  countStub(args.map(String).join(" "));
  consoleError(...args);
};
const nextFrame = () => work.frame();

function until(what: string, test: () => boolean, timeoutMs = 120_000): Promise<number> {
  return new Promise((resolve, reject) => {
    const started = performance.now();
    const tick = () => {
      if (test()) return resolve(performance.now());
      if (performance.now() - started > timeoutMs) return reject(new Error(`timed out: ${what}`));
      void work.frame().then(tick);
    };
    tick();
  });
}

// 첫 행의 둘째 열 글을 그린 때.
const FIRST = "row-000000-alpha";
let drewFirst: number | undefined;
const fillText = CanvasRenderingContext2D.prototype.fillText;
CanvasRenderingContext2D.prototype.fillText = function (text, x, y, maxWidth) {
  if (drewFirst === undefined && text === FIRST) drewFirst = performance.now();
  return fillText.call(this, text, x, y, maxWidth);
};

const COLUMNS = [
  ["id", "int8"],
  ["name", "text"],
  ["amount", "numeric"],
  ["big", "int8"],
  ["ratio", "float8"],
  ["active", "bool"],
  ["created_at", "timestamptz"],
  ["uid", "uuid"],
  ["payload", "jsonb"],
  ["note", "text"],
].map(([name, dbType]) => ({ name: name!, dbType: dbType!, nullable: true }));

function rowAt(i: number): DbValue[] {
  return [
    { int: i },
    { text: `row-${String(i).padStart(6, "0")}-alpha` },
    { decimal: `${i}.${String(i % 100).padStart(2, "0")}` },
    { int: (9007199254740993n + BigInt(i)).toString() },
    { float: i / 7 },
    { bool: i % 2 === 0 },
    { datetime: `2026-10-31 12:${String(i % 60).padStart(2, "0")}:00+09` },
    { uuid: `00000000-0000-4000-8000-${String(i).padStart(12, "0")}` },
    { json: `{"i":${i},"tags":["a","b"]}` },
    i % 7 === 0 ? "null" : { text: `note ${i % 1000}` },
  ];
}

/** 문장 하나의 스트림 항목(JSON 문자열로 미리 만든다). */
function events(
  rows: number,
  truncated: boolean,
  values: (i: number) => DbValue[] = rowAt,
): string[] {
  const out: DbQueryEvent[] = [
    { kind: "accepted", executionId: "e1", statements: 1 },
    { kind: "started", index: 0, range: { from: 0, to: 20 }, grade: "read" },
    { kind: "columns", index: 0, columns: COLUMNS.slice(0, values(0).length) },
  ];
  for (let from = 0; from < rows; from += CHUNK) {
    const n = Math.min(CHUNK, rows - from);
    out.push({
      kind: "rows",
      index: 0,
      rows: Array.from({ length: n }, (_, k) => values(from + k)),
    });
  }
  out.push({ kind: "done", index: 0, rowsAffected: null, elapsedMs: 0, truncated });
  return out.map((e) => JSON.stringify(e));
}

/** `db.query.execute` 에 `script` 의 항목을 보내는 가짜 Host. */
function fakeHost(): { client: RpcClient; setScript: (s: string[]) => void } {
  const [clientSide, server] = createMemoryTransportPair();
  let next = 1;
  let script: string[] = [];
  const send = (m: object) => server.send(JSON.stringify({ jsonrpc: "2.0", ...m }));
  server.onMessage((text) => {
    const m = JSON.parse(text) as { id?: number; method: string };
    if (m.method !== "db.query.execute") {
      if (m.id !== undefined)
        send({ id: m.id, error: { code: -32601, message: `no ${m.method}` } });
      return;
    }
    const streamId = next++;
    send({ id: m.id, result: { streamId } });
    const items = script;
    void (async () => {
      for (const data of items) {
        server.send(
          `{"jsonrpc":"2.0","method":"stream.chunk","params":{"streamId":${streamId},"data":${data}}}`,
        );
        await new Promise((r) => setTimeout(r, CHUNK_GAP_MS));
      }
      send({ method: "stream.end", params: { streamId, status: "completed" } });
    })();
  });
  return { client: new RpcClient(clientSide), setScript: (s) => (script = s) };
}

const theme = parseTheme(defaultTheme);
const store = createThemeStore({
  system: { current: () => "light", subscribe: () => () => {} },
  storage: { read: () => undefined, write: () => {} },
  theme,
  apply: (t, appearance) =>
    applyTokens(document.documentElement.style, t.variants[appearance] ?? t.variants.light!),
});
const l10n = bundledLocalization("ko");
const root = createRoot(document.getElementById("root")!);

function show(buffer: ResultBuffer, client: RpcClient) {
  root.render(
    <ThemeStoreProvider store={store}>
      <LocalizationProvider l10n={l10n}>
        <ResultView buffer={buffer} insert={{ client, dialect: "postgres", table: null }} />
      </LocalizationProvider>
    </ThemeStoreProvider>,
  );
}

/** 한 번 실행: 화면에 새 결과를 걸고 스트림을 받기 시작한다. */
function execute(host: ReturnType<typeof fakeHost>, script: string[]) {
  host.setScript(script);
  const q = new QueryRun();
  let shown = false;
  q.subscribe(() => {
    const r = q.results[0];
    if (r && !shown) {
      shown = true;
      show(r.buffer, host.client);
    }
  });
  const done = q.run(host.client, {
    sessionId: "s1",
    sql: "SELECT * FROM t",
    statements: null,
    confirmed: null,
  });
  return { q, done };
}

let phase = "start";

function scroller(): HTMLElement {
  const el = document.querySelector<HTMLElement>("[data-testid=db-result-grid] .dvn-scroller");
  if (!el) {
    const g = document.querySelector("[data-testid=db-result-grid]");
    const classes = g
      ? [...g.querySelectorAll("[class]")].map((e) => e.getAttribute("class")).join("|")
      : "no grid";
    throw new Error(`no grid scroller (${phase}) ${classes.slice(0, 400)}`);
  }
  return el;
}

/** 셀 영역(머리 아래)에서 가장 흔한 화소 색 — 셀 배경이다(글자 · 경계선은 적다). */
function cellBackground(): string {
  const canvas = document.querySelector<HTMLCanvasElement>("[data-testid=db-result-grid] canvas")!;
  const ctx = canvas.getContext("2d")!;
  const ratio = canvas.width / canvas.clientWidth;
  const x = Math.round(100 * ratio);
  const y = Math.round((28 + ROW_HEIGHT) * ratio);
  const w = Math.round(800 * ratio);
  const h = Math.round(ROW_HEIGHT * 10 * ratio);
  const data = ctx.getImageData(x, y, w, h).data;
  const counts = new Map<string, number>();
  for (let i = 0; i < data.length; i += 4) {
    const hex = `#${[data[i]!, data[i + 1]!, data[i + 2]!].map((v) => v.toString(16).padStart(2, "0")).join("")}`;
    counts.set(hex, (counts.get(hex) ?? 0) + 1);
  }
  return [...counts.entries()].sort((a, b) => b[1] - a[1])[0]![0];
}

async function main() {
  const host = fakeHost();
  const full = events(ROWS, false);
  const hostile = '<img src="x" onerror="window.__pwned=1"><script>window.__pwned=2</script>';

  phase = "first";
  // 1) 실행 → 첫 행이 그려진 프레임, 다 받기.
  const t0 = performance.now();
  const first = execute(host, full);
  await until("the first row", () => drewFirst !== undefined);
  await nextFrame();
  const firstFrameMs = performance.now() - t0;
  await first.done;
  const buffer = first.q.results[0]!.buffer;
  await until("every row", () => buffer.state.kind === "done" && buffer.rowCount === ROWS);
  const allRowsMs = performance.now() - t0;
  await nextFrame();
  await nextFrame();

  phase = "fling";
  // 2) 끝까지 연속 스크롤.
  const s = scroller();
  const height = s.scrollHeight - s.clientHeight;
  const fling = await scrollFrames(
    s,
    FLING_FRAMES,
    (f) => Math.round((height * (f + 1)) / FLING_FRAMES),
    work,
  );
  s.scrollTop = 0;
  await nextFrame();

  phase = "streaming";
  // 3) 스트리밍 중 스크롤: 새 실행을 걸고 받는 동안 프레임마다 아래로.
  const second = execute(host, full);
  const streamBuffer = () => second.q.results[0]!.buffer;
  await until(
    "the first rows of the second run",
    () => second.q.results[0]?.buffer.firstRowAt !== undefined,
  );
  await nextFrame();
  const intervals: number[] = [];
  work.start();
  let last = await work.frame();
  let top = 0;
  let framesWhileStreaming = 0;
  while (streamBuffer().state.kind === "streaming") {
    const loaded = streamBuffer().rowCount;
    top = Math.min(
      top + STREAM_STEP_ROWS * ROW_HEIGHT,
      Math.max(0, loaded * ROW_HEIGHT - s.clientHeight),
    );
    const sc = scroller();
    sc.scrollTop = top;
    sc.dispatchEvent(new Event("scroll"));
    const now = await work.frame();
    intervals.push(now - last);
    last = now;
    framesWhileStreaming++;
  }
  const streamingWork = work.take();
  await second.done;
  const { frameStats } = await import("../../src/perf-frames");
  const streaming = {
    frames: framesWhileStreaming,
    rowsAtEnd: streamBuffer().rowCount,
    interval: frameStats(intervals),
    work: frameStats(streamingWork),
  };

  phase = "capped";
  // 4) 상한 + 1 행: Host 가 상한(10만)에서 커서를 버리고 truncated 로 끝낸다.
  const third = execute(host, events(ROWS, true));
  await third.done;
  await until(
    "the capped status",
    () => document.querySelector("[data-testid=db-grid-capped]") !== null,
  );
  const capped = document.querySelector("[data-testid=db-grid-capped]")!.textContent;

  phase = "theme";
  // 5) 라이트 → 다크 전환에 다시 칠한다.
  await nextFrame();
  await nextFrame();
  const lightPixel = cellBackground();
  store.getState().setPreference("dark");
  await nextFrame();
  await nextFrame();
  await nextFrame();
  const darkPixel = cellBackground();
  store.getState().setPreference("light");

  phase = "hostile";
  // 6) 신뢰할 수 없는 값: 글자로만 그린다(이미지 · 스크립트가 생기지 않고 오버레이 편집기 청크를 불러오지 않는다).
  const fourth = execute(
    host,
    events(3, false, (i) => [
      { int: i },
      { text: hostile },
      { json: `{"h":"${hostile.replaceAll('"', '\\"')}"}` },
    ]),
  );
  await fourth.done;
  await nextFrame();
  await nextFrame();
  const scripts = performance
    .getEntriesByType("resource")
    .map((e) => e.name.split("/").pop()!)
    .filter((n) => n.endsWith(".js"));

  const result = {
    userAgent: navigator.userAgent,
    rows: ROWS,
    columns: COLS,
    chunkRows: CHUNK,
    viewport: { width: s.clientWidth, height: s.clientHeight },
    firstFrameMs: Math.round(firstFrameMs * 10) / 10,
    allRowsMs: Math.round(allRowsMs * 10) / 10,
    fling,
    streaming,
    capped,
    theme: {
      light: lightPixel,
      dark: darkPixel,
      lightToken: theme.variants.light!["grid.background"],
      darkToken: theme.variants.dark!["grid.background"],
    },
    stubErrors,
    hostile: {
      imgOrScript: document.querySelectorAll("img, script:not([type=module])").length,
      pwned: (window as unknown as { __pwned?: number }).__pwned ?? null,
      overlayEditorOpen: document.querySelector(".gdg-overlay-editor, #portal .gdg-style") !== null,
      scripts,
    },
  };
  window.__resultGridBench = result;
  document.title = `done ${JSON.stringify(result)}`;
}

main().catch((e: unknown) => {
  document.title = `error [${phase}] ${e instanceof Error ? `${e.message} ${e.stack}` : String(e)}`;
});
