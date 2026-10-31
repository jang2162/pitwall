// DB 탐색기 측정 하네스(docs/design/14-m3-plan.md M3-13 의 완료 기준 — 테이블 1만 개 스키마를 펼친 첫 화면이 500 ms 안, 결정
// M3-D17). 탐색기 화면(`DbExplorerView`)을 그대로 띄우고, Host 는 메모리 속 가짜다 — `db.schema.*` 를 M3-10 의 꼴(객체 목록은 서버
// 쪽에서 나눈 500개 쪽)로 답하고, 메시지는 IPC 처럼 JSON 문자열로 오간다(memory transport). Host 쪽의 일(캐시 · 드라이버 질의)은 Rust
// 몫이라 여기서 재지 않는다 — M3-10 의 시험이 첫 쪽에 드라이버를 한 번만 부르는 것을 본다.
//
// 만들기 · 돌리기: perf/db-explorer/run.mjs 머리 주석. 끝나면 결과가 `window.__dbExplorerBench` 와 페이지 제목(`done <JSON>`)에 남는다.
//
// 잰 것(헤드리스 — 실제 창은 M3-21 이 release 번들에서 다시 잰다):
// - `tablesMs`: 연결 → DB → 스키마를 펼쳐 둔 뒤 「테이블」 묶음(1만 개)의 ▸ 를 누른 때부터 첫 테이블 행이 그려진 다음 프레임까지.
// - `chainMs`: 연결 행의 ▸ 부터 DB · 스키마 · 「테이블」 을 차례로 펼쳐(앞 단계가 그려지면 곧바로 다음을 누른다) 첫 테이블 행이 그려진
//   다음 프레임까지 — 사람이 네 번 누르는 사이의 시간은 빠진다.
// - `domRows`: 그때 문서에 있는 행 수(가상화 — 1만 개를 다 그리지 않는다).

import "../bench.css";

import { LocalizationProvider } from "@fluent/react";
import { ActionRegistry } from "@pitwall/keymap";
import { createMemoryTransportPair, type ObjectInfo, RpcClient } from "@pitwall/rpc-client";
import { createRoot } from "react-dom/client";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { DbExplorerView, type ExplorerOps } from "../../src/db/explorer/db-explorer";
import { ExplorerModel } from "../../src/db/explorer/explorer-model";
import { bundledLocalization } from "../../src/i18n/testing";
import { createThemeStore } from "../../src/theme/theme-store";
import { ThemeStoreProvider } from "../../src/theme/theme-switcher";
import { applyTokens, parseTheme } from "../../src/theme/tokens";

const TABLES = 10_000;
const PROJECT = "bench";

declare global {
  interface Window {
    __dbExplorerBench?: unknown;
  }
}

const tables: ObjectInfo[] = Array.from({ length: TABLES }, (_, i) => ({
  name: `table_${String(i).padStart(5, "0")}`,
  kind: "table",
  comment: null,
}));

function answer(method: string, params: Record<string, unknown>): unknown {
  switch (method) {
    case "db.connection.list":
      return {
        connections: [
          {
            id: "pg",
            config: { name: "big-pg", driver: "postgres", host: "localhost", database: "app" },
            layers: [],
            readOnly: false,
            state: { kind: "ready" },
          },
        ],
        problem: null,
        stateProblem: null,
        trusted: true,
      };
    case "db.schema.databases":
      return { items: [{ name: "app" }], nextCursor: null };
    case "db.schema.schemas":
      return { items: [{ name: "public" }], nextCursor: null };
    case "db.schema.objects": {
      if (params.kind !== "table") return { items: [], nextCursor: null };
      const from = typeof params.cursor === "string" ? Number(params.cursor) : 0;
      const limit = Number(params.limit);
      const next = from + limit;
      return {
        items: tables.slice(from, next),
        nextCursor: next < tables.length ? String(next) : null,
      };
    }
    default:
      throw new Error(`unexpected ${method}`);
  }
}

/** JSON-RPC 를 메모리 transport 로 답하는 가짜 Host. */
function fakeHost(): RpcClient {
  const [clientSide, serverSide] = createMemoryTransportPair();
  serverSide.onMessage((text) => {
    const m = JSON.parse(text) as { id: number; method: string; params: Record<string, unknown> };
    let reply: object;
    try {
      reply = { jsonrpc: "2.0", id: m.id, result: answer(m.method, m.params) };
    } catch (e) {
      reply = { jsonrpc: "2.0", id: m.id, error: { code: -32601, message: String(e) } };
    }
    serverSide.send(JSON.stringify(reply));
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
const ops: ExplorerOps = {
  copy: async () => {},
  confirm: async () => false,
  notify: () => {},
  openDdl: () => {},
};

function mount(): ExplorerModel {
  const model = new ExplorerModel({ calls: fakeHost(), projectId: PROJECT });
  root.render(
    <ThemeStoreProvider store={store}>
      <LocalizationProvider l10n={l10n}>
        <DbExplorerView model={model} registry={new ActionRegistry()} ops={ops} />
      </LocalizationProvider>
    </ThemeStoreProvider>,
  );
  // 툴 윈도우(explorer-feature.tsx)가 처음 보일 때와 같이 목록을 읽는다.
  void model.reloadList();
  return model;
}

const frame = () => new Promise<number>((r) => requestAnimationFrame(() => r(performance.now())));

function waitFor(what: string, test: () => boolean, timeoutMs = 10_000): Promise<number> {
  return new Promise((resolve, reject) => {
    const started = performance.now();
    const tick = () => {
      if (test()) return resolve(performance.now());
      if (performance.now() - started > timeoutMs) return reject(new Error(`timed out: ${what}`));
      requestAnimationFrame(tick);
    };
    tick();
  });
}

const rowEl = (kind: string, name: string) =>
  document.querySelector<HTMLElement>(`[data-row="${kind}"][data-name="${name}"]`);

/** 그 행의 ▸ 를 누른다(사람의 클릭과 같은 이벤트). */
function toggle(kind: string, name: string) {
  const r = rowEl(kind, name);
  if (!r) throw new Error(`no ${kind} row ${name}`);
  (r.firstElementChild as HTMLElement).click();
}

async function expandTo(kind: string, name: string, next: { kind: string; name: string }) {
  toggle(kind, name);
  await waitFor(`${next.kind} ${next.name}`, () => rowEl(next.kind, next.name) !== null, 10_000);
}

let phase = "start";
try {
  const ua = navigator.userAgent;

  // 1) 스키마까지 펼쳐 둔 뒤 「테이블」 묶음.
  phase = "tables";
  let model = mount();
  await waitFor("connection row", () => rowEl("connection", "big-pg") !== null);
  await expandTo("connection", "big-pg", { kind: "database", name: "app" });
  await expandTo("database", "app", { kind: "schema", name: "public" });
  await expandTo("schema", "public", { kind: "group", name: "테이블" });
  await frame();
  await frame();
  const start = performance.now();
  toggle("group", "테이블");
  await waitFor("first table", () => rowEl("object", "table_00000") !== null);
  const tablesMs = (await frame()) - start;
  const domRows = document.querySelectorAll("[data-row]").length;
  const modelRows = model.rows().length;

  // 2) 연결부터 「테이블」 까지.
  phase = "chain";
  root.render(<></>);
  await frame();
  model = mount();
  await waitFor("connection row", () => rowEl("connection", "big-pg") !== null);
  await frame();
  const chainStart = performance.now();
  await expandTo("connection", "big-pg", { kind: "database", name: "app" });
  await expandTo("database", "app", { kind: "schema", name: "public" });
  await expandTo("schema", "public", { kind: "group", name: "테이블" });
  toggle("group", "테이블");
  await waitFor("first table", () => rowEl("object", "table_00000") !== null);
  const chainMs = (await frame()) - chainStart;

  const result = { userAgent: ua, tables: TABLES, tablesMs, chainMs, domRows, modelRows };
  window.__dbExplorerBench = result;
  document.title = `done ${JSON.stringify(result)}`;
} catch (e) {
  document.title = `error [${phase}] ${e instanceof Error ? `${e.message} ${e.stack}` : String(e)}`;
}
