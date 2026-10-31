// Redis 키 트리 측정 하네스(docs/design/14-m3-plan.md M3-18 의 완료 기준 — 키 100만 개, 패턴 없는 첫 화면 500 ms, 키 트리를 끝까지
// 스크롤할 때 06 「스크롤 60fps」 판정). 키 트리 화면(`KeyTreeView`)과 스캔(`KeyScan` → `KeyTree`)을 그대로 띄우고, Host 는 메모리 속
// 가짜다 — `db.kv.scan` 스트림을 M3-22 의 꼴(`accepted` → `keys` 1,000키씩 … → `done`)로 보내고, 메시지는 IPC 처럼 JSON 문자열로
// 오간다(memory transport). 키는 live 시험(`crates/pw-db/tests/kv_live.rs` 의 100만 키 시험)이 심는 서버와 같은 꼴이다: `k:0` ~
// `k:999999` 의 string, 크기는 `MEMORY USAGE` 의 값 언저리, SCAN 처럼 섞인 차례. 청크의 JSON 은 재기 전에 만들어 둔다 — Host 쪽(SCAN +
// 타입 · TTL · 크기 파이프라인 + 직렬화)은 Rust 몫이고 live 시험이 첫 묶음을 잰다(M3-22 ⑤). 첫 화면의 전체 값은 그 Host 시간 +
// 여기서 잰 창 시간이다(보고에 함께 적는다).
//
// 만들기 · 돌리기: perf/redis-keys/run.mjs 머리 주석. 끝나면 결과가 `window.__redisKeysBench` 와 페이지 제목(`done <JSON>`)에 남는다.
//
// 잰 것(헤드리스 — 실제 창은 M3-21 이 release 번들에서 다시 잰다):
// - `firstFrameMs`: 스캔 시작(스트림 요청)부터 트리에 첫 줄을 그린 뒤 다음 프레임까지(패턴 없음).
// - `allKeysMs`: 스캔 시작부터 100만 키를 다 받아 스캔이 끝난 때까지(청크 사이 `CHUNK_GAP_MS`).
// - `expandMs`: 다 받은 뒤 그룹 `k`(키 100만 개)를 펼쳐 첫 키 줄을 그린 프레임까지(이름 순 정렬 포함).
// - `fling`: 펼친 그룹을 처음부터 끝까지 `FLING_FRAMES` 프레임에(한 프레임에 약 500줄). `streaming`: 그룹을 펼친 채 새 스캔을 걸고
//   스캔이 끝날 때까지 프레임마다 아래로 `STREAM_STEP_ROWS` 줄씩. 값은 src/perf-frames.ts 의 `interval`(판정: `withinVsync` ≥ 0.95 ·
//   `vsyncMs` ≤ 17) · `work`.

import "../bench.css";

import { LocalizationProvider } from "@fluent/react";
import {
  createMemoryTransportPair,
  type DbKvScanEvent,
  type KvKeyInfo,
  RpcClient,
} from "@pitwall/rpc-client";
import { useCallback, useState, useSyncExternalStore } from "react";
import { createRoot } from "react-dom/client";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { KeyScan } from "../../src/db/redis/key-scan";
import { KeyTree } from "../../src/db/redis/key-tree";
import { KeyTreeView, ROW_HEIGHT } from "../../src/db/redis/key-tree-view";
import { bundledLocalization } from "../../src/i18n/testing";
import { frameStats, installFrameWork, scrollFrames } from "../../src/perf-frames";
import { createThemeStore } from "../../src/theme/theme-store";
import { ThemeStoreProvider } from "../../src/theme/theme-switcher";
import { applyTokens, parseTheme } from "../../src/theme/tokens";

const KEYS = 1_000_000;
const CHUNK = 1_000;
const CHUNK_GAP_MS = 1;
const FLING_FRAMES = 2000;
const STREAM_STEP_ROWS = 20;
/** SCAN 처럼 섞인 차례: `i` 번째로 오는 키의 번호(100만과 서로소인 곱 — 한 번씩만 온다). */
const order = (i: number) => (i * 7919 + 13) % KEYS;

declare global {
  interface Window {
    __redisKeysBench?: unknown;
  }
}

const work = installFrameWork();
const nextFrame = () => work.frame();

function until(
  what: string,
  test: () => boolean,
  timeoutMs = 300_000,
  where: () => string = () => "",
): Promise<number> {
  return new Promise((resolve, reject) => {
    const started = performance.now();
    const tick = () => {
      if (test()) return resolve(performance.now());
      if (performance.now() - started > timeoutMs)
        return reject(new Error(`timed out: ${what} ${where()}`));
      void work.frame().then(tick);
    };
    tick();
  });
}

/** 스캔 스트림 항목(JSON 문자열로 미리 만든다). */
function events(): string[] {
  const out: DbKvScanEvent[] = [{ kind: "accepted", executionId: "x1" }];
  for (let from = 0; from < KEYS; from += CHUNK) {
    const keys: KvKeyInfo[] = Array.from({ length: CHUNK }, (_, k) => {
      const n = order(from + k);
      return { key: `k:${n}`, kind: "string", ttlMs: null, size: 56 + (n % 16) };
    });
    out.push({ kind: "keys", keys });
  }
  out.push({ kind: "done", keys: KEYS, elapsedMs: 0 });
  return out.map((e) => JSON.stringify(e));
}

function fakeHost(script: string[]): RpcClient {
  const [clientSide, server] = createMemoryTransportPair();
  let next = 1;
  const send = (m: object) => server.send(JSON.stringify({ jsonrpc: "2.0", ...m }));
  server.onMessage((text) => {
    const m = JSON.parse(text) as { id?: number; method: string };
    if (m.method !== "db.kv.scan") {
      if (m.id !== undefined)
        send({ id: m.id, error: { code: -32601, message: `no ${m.method}` } });
      return;
    }
    const streamId = next++;
    send({ id: m.id, result: { streamId } });
    void (async () => {
      for (const data of script) {
        server.send(
          `{"jsonrpc":"2.0","method":"stream.chunk","params":{"streamId":${streamId},"data":${data}}}`,
        );
        await new Promise((r) => setTimeout(r, CHUNK_GAP_MS));
      }
      send({ method: "stream.end", params: { streamId, status: "completed" } });
    })();
  });
  return new RpcClient(clientSide);
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
const noop = () => {};
const ACTIONS = { open: noop, editTtl: noop, remove: noop, rename: noop, copyName: noop };

let swap: ((b: { tree: KeyTree; scan: KeyScan }) => void) | undefined;

function Bench({ initial }: { initial: { tree: KeyTree; scan: KeyScan } }) {
  const [b, setB] = useState(initial);
  swap = setB;
  const state = useSyncExternalStore(
    useCallback((f) => b.scan.subscribe(f), [b.scan]),
    () => b.scan.state,
  );
  return (
    <KeyTreeView
      tree={b.tree}
      loading={state.kind === "scanning"}
      selected={undefined}
      onSelect={noop}
      actions={ACTIONS}
      readOnly={false}
    />
  );
}

function scroller(): HTMLElement {
  const el = document.querySelector<HTMLElement>("[data-testid=redis-key-tree]");
  if (!el) throw new Error("no key tree");
  return el;
}

const rowsShown = () => document.querySelectorAll("[data-testid=redis-key-tree] [role=treeitem]");

async function main() {
  const script = events();
  const client = fakeHost(script);

  // 1) 스캔 시작 → 첫 줄을 그린 프레임, 다 받기.
  const first = { tree: new KeyTree(":"), scan: undefined as unknown as KeyScan };
  first.scan = new KeyScan(first.tree);
  root.render(
    <ThemeStoreProvider store={store}>
      <LocalizationProvider l10n={l10n}>
        <Bench initial={first} />
      </LocalizationProvider>
    </ThemeStoreProvider>,
  );
  await nextFrame();
  await nextFrame();
  const t0 = performance.now();
  const done = first.scan.run(client, { sessionId: "s1", pattern: null });
  await until("the first row", () => rowsShown().length > 0);
  await nextFrame();
  const firstFrameMs = performance.now() - t0;
  const firstRows = rowsShown().length;
  await done;
  await until(
    "every key",
    () => first.scan.state.kind === "done" && first.tree.size === KEYS,
    300_000,
    () => `${first.scan.state.kind} ${first.tree.size}`,
  );
  const allKeysMs = performance.now() - t0;
  await nextFrame();

  // 2) 그룹 k(100만 키)를 펼친다.
  const group = document.querySelector<HTMLElement>("[data-group='k:']")!;
  const e0 = performance.now();
  group.click();
  await until("the first key row", () => document.querySelector("[data-key='k:0']") !== null);
  await nextFrame();
  const expandMs = performance.now() - e0;

  // 3) 끝까지 연속 스크롤.
  const s = scroller();
  const height = s.scrollHeight - s.clientHeight;
  const fling = await scrollFrames(
    s,
    FLING_FRAMES,
    (f) => Math.round((height * (f + 1)) / FLING_FRAMES),
    work,
  );
  await nextFrame();
  const lastKey = [...rowsShown()].at(-1)?.getAttribute("data-key");
  s.scrollTop = 0;
  await nextFrame();

  // 4) 펼친 채로 새 스캔(다시 찾기) — 받는 동안 프레임마다 아래로.
  const second = { tree: new KeyTree(":"), scan: undefined as unknown as KeyScan };
  second.scan = new KeyScan(second.tree);
  swap!(second);
  await nextFrame();
  const running = second.scan.run(fakeHost(script), { sessionId: "s1", pattern: null });
  await until("the first keys of the second scan", () => second.tree.size > 0);
  const intervals: number[] = [];
  work.start();
  let last = await work.frame();
  let top = 0;
  let framesWhileStreaming = 0;
  while (second.scan.state.kind === "scanning") {
    top = Math.min(
      top + STREAM_STEP_ROWS * ROW_HEIGHT,
      Math.max(0, s.scrollHeight - s.clientHeight),
    );
    s.scrollTop = top;
    s.dispatchEvent(new Event("scroll"));
    const now = await work.frame();
    intervals.push(now - last);
    last = now;
    framesWhileStreaming++;
  }
  const streamingWork = work.take();
  await running;
  const streaming = {
    frames: framesWhileStreaming,
    interval: frameStats(intervals),
    work: frameStats(streamingWork),
  };

  return {
    userAgent: navigator.userAgent,
    keys: KEYS,
    firstFrameMs,
    firstRows,
    allKeysMs,
    expandMs,
    fling,
    lastKey,
    streaming,
  };
}

main().then(
  (r) => {
    window.__redisKeysBench = r;
    document.title = `done ${JSON.stringify(r)}`;
  },
  (e: unknown) => {
    document.title = `error ${e instanceof Error ? e.message : String(e)}`;
  },
);
