// 앱 안 DB 측정 시나리오(release 번들, `crates/pw-perf` 의 DB 측정 — CONTRIBUTING.md 「성능 기준선」 의 「DB 시나리오」). perf-scenario.ts
// 가 측정 모드 프로젝트 창에서 첫 프레임 뒤에 부른다. 14-m3-plan.md M3-21 이 release 번들에서 다시 재는 것(M3-16 · M3-13 · M3-18 의
// 완료 기준, 06 「성능 목표」 의 DB 행)을 잰다. 연결은 측정하는 쪽이 프로젝트의 `.pitwall/db/<id>.toml` 에 적고(id = 이름), 프로젝트는
// 신뢰로 등록돼 있다(미신뢰 프로젝트는 연결을 열지 못한다 — 결정 M3-D13). 키는 모두 웹뷰 안에서 만든 이벤트다(OS 키 입력이 아니다).
//
// `db` 시나리오 — 탐색기(⌘8)에 보이는 연결로 단계를 고른다(없는 연결의 단계는 건너뛴다):
//
// 1. `grid_open`(M3-16, 06 「쿼리 결과 첫 화면」): 연결 `perf-grid` 의 새 콘솔(⌘⇧L — 「마지막 연결」 을 이 연결로 적어 둔다)에서
//    [연결] 하고 `SELECT * FROM perf_rows`(10만 행 × 10열)를 넣은 뒤, [실행] 을 누른 때부터 그리드 canvas 에 첫 행의 둘째 칸
//    (`r0c1`)을 그린 다음 프레임까지. 실행 범위 RPC · Host 실행 · 첫 청크 · IPC · 그리기가 모두 든다.
// 2. `grid_rows`: 상태 줄이 끝(`done`)이고 그리드 행 수가 `GRID_ROWS` 인 때까지. `grid_scroll`: 처음부터 끝까지 `FLING_FRAMES`
//    프레임에(`fling`, 한 프레임에 약 50행 — 헤드리스 하네스 perf/result-grid 와 같다).
// 3. `grid_stream_scroll`(M3-16 「스트리밍 중에도」): `SELECT * FROM perf_stream`(같은 행을 천천히 보낸다)을 실행하고, 첫 행이 오면
//    받는 동안 프레임마다 `STREAM_STEP_ROWS` 행씩(받은 끝을 넘지 않게) 아래로 스크롤한다.
// 4. `explorer_tables`(M3-13, 06 「스키마 탐색기 첫 화면」): 연결 `perf-schema` 를 펼쳐(연결을 연다) 스키마 `public` 을 펼친 뒤,
//    「테이블」 묶음(1만 개)의 ▸ 를 누른 때부터 첫 테이블 행이 그려진 다음 프레임까지.
// 5. `redis_open`(M3-18, 06 「Redis 키 브라우저 첫 화면」): 연결 `perf-redis`(키 100만 개)를 탐색기에서 더블클릭해 연 탭에서 [연결] 을
//    누른 때부터 키 트리에 첫 줄이 그려진 다음 프레임까지(연결 + 패턴 없는 스캔의 첫 묶음). `redis_keys`: 스캔이 끝난 때까지.
//    `redis_scroll`: 그룹 `k:` 를 펼쳐 첫 키 줄을 그린 프레임까지(`expandMs`) 잰 뒤 처음부터 끝까지 `FLING_FRAMES` 프레임에.
//
// `db-idle` 시나리오 — 콜드 스타트 · 유휴 메모리를 「DB 연결 하나를 연 프로젝트」 로(M3-21): 연결 `perf-idle` 의 새 콘솔에서 [연결] 하고
// 세션이 열리면 `db_connected` 를 남기고 끝난다. 측정하는 쪽은 그때부터 유휴 시간을 기다려 메모리를 잰다.
//
// 끝나면 `scenario_done`, 단계가 실패하면 `scenario_error { step, message }` 하나를 남기고 멈춘다.

import { EditorView } from "@codemirror/view";

import { LAST_CONNECTION_KEY } from "./db/console/console-window";
import { ROW_HEIGHT as GRID_ROW_HEIGHT } from "./db/grid/glide-grid";
import { frameStats, scrollFrames } from "./perf-frames";
import { type ScenarioEnv, until } from "./perf-scenario";

/** 측정하는 쪽(`crates/pw-perf` 의 `db.rs`)이 적는 연결 id(= 이름). */
export const GRID_CONNECTION = "perf-grid";
export const SCHEMA_CONNECTION = "perf-schema";
export const REDIS_CONNECTION = "perf-redis";
export const IDLE_CONNECTION = "perf-idle";

/** 결과 테이블(가짜 드라이버 `perf_fake_db.rs` 의 응답 · 실 PG 픽스처의 테이블 이름). */
export const ROWS_SQL = "SELECT * FROM perf_rows";
export const STREAM_SQL = "SELECT * FROM perf_stream";
/** 첫 행의 둘째 칸 — 가짜는 `r{행}c{열}`, 실 PG 픽스처도 같은 꼴로 심는다. */
export const FIRST_CELL = "r0c1";
export const GRID_ROWS = 100_000;
export const STREAM_STEP_ROWS = 20;
export const FLING_FRAMES = 2000;

/** 탐색기 스키마 · 「테이블」 묶음, Redis 그룹. */
export const SCHEMA = "public";
export const REDIS_GROUP = "k:";
export const REDIS_FIRST_KEY = "k:0";

const ms = (x: number) => Math.round(x * 10) / 10;

const key = (code: string, k: string, mods: { shift?: boolean } = {}) =>
  new KeyboardEvent("keydown", {
    code,
    key: k,
    metaKey: true,
    shiftKey: mods.shift ?? false,
    bubbles: true,
    cancelable: true,
  });

/** 이 창의 프로젝트 id(주소의 `project`). */
function projectOf(doc: Document): string {
  const id = new URLSearchParams(doc.defaultView?.location.search ?? "").get("project");
  if (!id) throw new Error("the window address has no project id");
  return id;
}

/**
 * canvas 에 `text` 를 처음 그린 시각을 적는다. `restore` 로 걷는다. 어느 canvas 인지는 보지 않는다 — Glide 는 문서 밖의 버퍼 canvas
 * 에 그린 뒤 화면 canvas 로 옮겨 붙여(이중 버퍼) 그리드 요소 아래에서 찾을 수 없다. `r0c1` 같은 글은 결과 그리드만 그린다.
 */
export function watchCellText(now: () => number, text: string) {
  const proto = CanvasRenderingContext2D.prototype;
  const fillText = proto.fillText;
  let at: number | undefined;
  /** 실패 진단: 그동안 그린 글의 앞 몇 개. */
  const seen: string[] = [];
  proto.fillText = function (this: CanvasRenderingContext2D, t, x, y, maxWidth) {
    if (at === undefined && t === text) at = now();
    if (at === undefined && seen.length < 12 && !seen.includes(t)) seen.push(t);
    return fillText.call(this, t, x, y, maxWidth);
  };
  return {
    at: () => at,
    reset: () => void (at = undefined),
    seen: () => seen.slice(),
    restore: () => void (proto.fillText = fillText),
  };
}

/** 탐색기의 연결 행. */
export function connectionRow(doc: Document, id: string): HTMLElement | null {
  return doc.querySelector<HTMLElement>(`[data-row="connection"][data-connection="${id}"]`);
}

/** 탐색기 행의 ▸ 를 누른다(행의 첫 칸이 펼치기 단추다 — src/db/explorer/db-explorer.tsx). */
function toggle(row: HTMLElement) {
  (row.firstElementChild as HTMLElement).click();
}

function dblclick(el: Element) {
  el.dispatchEvent(new MouseEvent("dblclick", { bubbles: true, cancelable: true }));
}

/** 탐색기가 보이는가 — 툴바가 있고 높이가 있다(접힌 툴 윈도우는 DOM 에 남고 높이가 0 이다, git 시나리오의 `hideGitLog` 와 같다). */
const explorerShown = (doc: Document) => {
  const bar = doc.querySelector<HTMLElement>("[data-testid=db-explorer-add]");
  return bar !== null && bar.isConnected && bar.getClientRects().length > 0 && bar.clientHeight > 0;
};

/**
 * 탐색기(⌘8)를 열고 연결 목록이 보일 때까지. 프로젝트 창이 그려진 뒤에도 키맵이 붙기 전에 넣은 키는 버려지므로, 탐색기가 뜨지 않았으면
 * `EXPLORER_RETRY_MS` 마다 다시 넣는다 — 떠 있을 때는 넣지 않는다(⌘8 은 열고 닫기를 오간다).
 */
async function openExplorer(env: ScenarioEnv) {
  const { doc } = env;
  let pressed = -Infinity;
  await until(env, "the DB explorer", () => {
    if (explorerShown(doc)) return true;
    if (env.now() - pressed >= EXPLORER_RETRY_MS) {
      doc.body.dispatchEvent(key("Digit8", "8"));
      pressed = env.now();
    }
    return false;
  });
  await until(env, "the DB explorer's connections", () =>
    doc.querySelector("[data-row=connection]"),
  );
}

export const EXPLORER_RETRY_MS = 2000;

/**
 * 탐색기를 닫는다 — 그리드 단계 전에. 탐색기 툴 윈도우가 에디터 영역을 줄이면 콘솔의 결과 칸(콘솔 탭의 아래 절반)이 몇 픽셀만 남아
 * 행을 하나도 그리지 않는다(2026-11-01 짧은 실기: 그리드 696×10). 처음 누름은 그 툴 윈도우를 활성으로만 만들 수 있어(⌘9 와 같다)
 * 닫힐 때까지 `EXPLORER_RETRY_MS / 4` 마다 넣는다.
 */
async function closeExplorer(env: ScenarioEnv) {
  const { doc } = env;
  let pressed = -Infinity;
  await until(env, "the DB explorer to close", () => {
    if (!explorerShown(doc)) return true;
    if (env.now() - pressed >= EXPLORER_RETRY_MS / 4) {
      doc.body.dispatchEvent(key("Digit8", "8"));
      pressed = env.now();
    }
    return false;
  });
}

/** 프로젝트 창이 다 그려지고 키맵이 붙을 때까지(git 시나리오와 같다). */
async function projectWindow(env: ScenarioEnv) {
  await until(env, "the project window", () => env.doc.querySelector("[data-tool-window=project]"));
  await env.sleep(300);
}

/** `connectionId` 의 새 콘솔(⌘⇧L)을 열고 [연결] 해 세션이 열린 콘솔. */
async function openConsole(env: ScenarioEnv, connectionId: string): Promise<HTMLElement> {
  const { doc } = env;
  const win = doc.defaultView!;
  win.localStorage.setItem(LAST_CONNECTION_KEY, JSON.stringify({ [projectOf(doc)]: connectionId }));
  const before = new Set(doc.querySelectorAll("[data-testid=db-console]"));
  doc.body.dispatchEvent(key("KeyL", "l", { shift: true }));
  const console_ = await until(env, "a new SQL console", () =>
    [...doc.querySelectorAll<HTMLElement>("[data-testid=db-console]")].find(
      (c) => !before.has(c) && c.querySelector("[data-testid=db-console-connect]"),
    ),
  );
  const connect = await until(env, "the console's connect button", () => {
    const b = console_.querySelector<HTMLButtonElement>("[data-testid=db-console-connect]");
    return b && !b.disabled ? b : undefined;
  });
  connect.click();
  await until(env, "the console's session", () => console_.dataset.session, 60_000);
  return console_;
}

/** 콘솔 편집기의 글을 `sql` 로 바꾼다(저장은 콘솔이 알아서 한다 — 측정과 상관없다). */
function setSql(console_: HTMLElement, sql: string) {
  const el = console_.querySelector<HTMLElement>(".cm-editor");
  const view = el ? EditorView.findFromDOM(el) : null;
  if (!view) throw new Error("the console has no editor");
  view.dispatch({
    changes: { from: 0, to: view.state.doc.length, insert: sql },
    selection: { anchor: 0 },
  });
}

/** 실패 진단: 그리드에서 콘솔까지 조상 요소의 높이(어디서 줄어드는지). */
function chain(console_: HTMLElement): string {
  const out: string[] = [];
  let el: HTMLElement | null = console_.querySelector<HTMLElement>("[data-testid=db-result-grid]");
  while (el && out.length < 16) {
    out.push(`${el.dataset.testid ?? el.tagName.toLowerCase()}:${el.clientHeight}`);
    if (el === console_) break;
    el = el.parentElement;
  }
  return out.join(">");
}

const status = (c: HTMLElement) =>
  c.querySelector<HTMLElement>("[data-testid=db-grid-status]")?.dataset.state;
const gridRows = (c: HTMLElement) =>
  Number(c.querySelector<HTMLElement>("[data-testid=db-result-grid]")?.dataset.rows ?? 0);
const gridScroller = (c: HTMLElement) =>
  c.querySelector<HTMLElement>("[data-testid=db-result-grid] .dvn-scroller");

function runButton(console_: HTMLElement): HTMLButtonElement {
  const b = console_.querySelector<HTMLButtonElement>("[data-testid=db-console-run]");
  if (!b) throw new Error("the console has no run button");
  return b;
}

async function gridSteps(env: ScenarioEnv, step: (s: string) => void) {
  step("grid console");
  const console_ = await openConsole(env, GRID_CONNECTION);
  setSql(console_, ROWS_SQL);
  await env.work.frame();
  await until(env, "the run button", () => !runButton(console_).disabled);

  step("grid_open");
  const cell = watchCellText(env.now, FIRST_CELL);
  try {
    const started = env.now();
    runButton(console_).click();
    const drawn = await until(env, "the first result row", cell.at, 60_000).catch((e: unknown) => {
      throw new Error(
        `${e instanceof Error ? e.message : String(e)} (status ${status(console_) ?? "none"}, rows ${gridRows(console_)}, error ${console_.querySelector("[data-testid=db-grid-error]")?.textContent ?? "none"}, grid ${gridScroller(console_)?.clientWidth ?? "-"}×${gridScroller(console_)?.clientHeight ?? "-"}, canvases ${console_.querySelectorAll("[data-testid=db-result-grid] canvas").length}, drawn ${JSON.stringify(cell.seen())}, window ${env.doc.defaultView?.innerWidth}×${env.doc.defaultView?.innerHeight}, chain ${chain(console_)})`,
      );
    });
    await env.work.frame();
    const firstMs = env.now() - started;
    const scroller = gridScroller(console_);
    await env.report("grid_open", {
      ms: ms(firstMs),
      firstDrawMs: ms(drawn - started),
      rowsAtFirst: gridRows(console_),
      viewport: { width: scroller?.clientWidth ?? 0, height: scroller?.clientHeight ?? 0 },
      window: {
        width: env.doc.defaultView?.innerWidth ?? 0,
        height: env.doc.defaultView?.innerHeight ?? 0,
      },
    });

    step("grid_rows");
    await until(
      env,
      "every result row",
      () => status(console_) === "done" && gridRows(console_) > 0,
      300_000,
    );
    await env.report("grid_rows", {
      rows: gridRows(console_),
      allRowsMs: ms(env.now() - started),
      state: status(console_),
    });
    await env.work.frame();
    await env.work.frame();

    step("grid_scroll");
    const s = gridScroller(console_);
    if (!s) throw new Error("the result grid has no scroller");
    const height = s.scrollHeight - s.clientHeight;
    const fling = await scrollFrames(
      s,
      FLING_FRAMES,
      (f) => Math.round((height * (f + 1)) / FLING_FRAMES),
      env.work,
    );
    await env.report("grid_scroll", {
      viewport: { width: s.clientWidth, height: s.clientHeight },
      scrollHeight: s.scrollHeight,
      fling,
    });
    s.scrollTop = 0;
    await env.work.frame();

    step("grid_stream_scroll");
    setSql(console_, STREAM_SQL);
    await until(env, "the run button after the first run", () => !runButton(console_).disabled);
    cell.reset();
    const streamStarted = env.now();
    runButton(console_).click();
    await until(env, "the first streamed row", cell.at, 60_000);
    const firstStreamMs = env.now() - streamStarted;
    const intervals: number[] = [];
    env.work.start();
    let last = await env.work.frame();
    let top = 0;
    let frames = 0;
    while (status(console_) === "streaming") {
      const sc = gridScroller(console_);
      if (sc) {
        const loaded = gridRows(console_);
        top = Math.min(
          top + STREAM_STEP_ROWS * GRID_ROW_HEIGHT,
          Math.max(0, loaded * GRID_ROW_HEIGHT - sc.clientHeight),
        );
        sc.scrollTop = top;
        sc.dispatchEvent(new Event("scroll"));
      }
      const now = await env.work.frame();
      intervals.push(now - last);
      last = now;
      frames++;
      if (env.now() - streamStarted > 300_000) throw new Error("the stream did not end in 300 s");
    }
    const worked = env.work.take();
    await until(env, "the end of the streamed result", () => status(console_) === "done", 60_000);
    await env.report("grid_stream_scroll", {
      firstMs: ms(firstStreamMs),
      frames,
      rowsAtEnd: gridRows(console_),
      allRowsMs: ms(env.now() - streamStarted),
      interval: frameStats(intervals),
      work: frameStats(worked),
    });
  } finally {
    cell.restore();
  }
}

async function explorerSteps(env: ScenarioEnv, step: (s: string) => void) {
  const { doc } = env;
  step("explorer connect");
  const conn = connectionRow(doc, SCHEMA_CONNECTION)!;
  const c0 = env.now();
  toggle(conn);
  const schema = await until(
    env,
    `the schema row ${SCHEMA}`,
    () => doc.querySelector<HTMLElement>(`[data-row="schema"][data-name="${SCHEMA}"]`),
    60_000,
  );
  const connectMs = env.now() - c0;
  toggle(schema);
  const group = await until(env, "the tables group", () =>
    doc.querySelector<HTMLElement>(`[data-row="group"][data-object-kind="table"]`),
  );
  await env.work.frame();
  await env.work.frame();

  step("explorer_tables");
  const started = env.now();
  toggle(group);
  await until(env, "the first table row", () => doc.querySelector(`[data-row="object"]`), 60_000);
  await env.work.frame();
  const tablesMs = env.now() - started;
  await env.report("explorer_tables", {
    ms: ms(tablesMs),
    connectMs: ms(connectMs),
    domRows: doc.querySelectorAll("[data-row]").length,
    firstTable: doc.querySelector<HTMLElement>(`[data-row="object"]`)?.dataset.name ?? null,
  });
}

async function redisSteps(env: ScenarioEnv, step: (s: string) => void) {
  const { doc } = env;
  step("redis tab");
  dblclick(connectionRow(doc, REDIS_CONNECTION)!);
  const tab = await until(env, "the Redis tab", () =>
    doc.querySelector<HTMLElement>("[data-redis-document]"),
  );
  const connect = await until(env, "the Redis connect button", () => {
    const b = tab.querySelector<HTMLButtonElement>("[data-action=connect]");
    return b && !b.disabled ? b : undefined;
  });
  await env.work.frame();

  step("redis_open");
  const rows = () => tab.querySelectorAll("[data-testid=redis-key-tree] [role=treeitem]");
  const started = env.now();
  connect.click();
  await until(env, "the first key tree row", () => rows().length > 0, 60_000);
  await env.work.frame();
  await env.report("redis_open", { ms: ms(env.now() - started), firstRows: rows().length });

  step("redis_keys");
  const scan = () => tab.querySelector<HTMLElement>("[data-testid=redis-scan-status]");
  await until(
    env,
    "the end of the key scan",
    () => {
      const s = scan()?.dataset.scan;
      return s !== undefined && s !== "scanning" && s !== "idle" ? s : undefined;
    },
    600_000,
  );
  await env.report("redis_keys", {
    allKeysMs: ms(env.now() - started),
    state: scan()?.dataset.scan ?? null,
    text: scan()?.textContent ?? null,
  });
  await env.work.frame();

  step("redis_scroll");
  const group = await until(env, `the key group ${REDIS_GROUP}`, () =>
    tab.querySelector<HTMLElement>(`[data-group="${REDIS_GROUP}"]`),
  );
  const e0 = env.now();
  group.click();
  await until(
    env,
    "the first key row",
    () => tab.querySelector(`[data-key="${REDIS_FIRST_KEY}"]`),
    120_000,
  );
  await env.work.frame();
  const expandMs = env.now() - e0;
  const s = tab.querySelector<HTMLElement>("[data-testid=redis-key-tree]");
  if (!s) throw new Error("no key tree");
  const height = s.scrollHeight - s.clientHeight;
  const fling = await scrollFrames(
    s,
    FLING_FRAMES,
    (f) => Math.round((height * (f + 1)) / FLING_FRAMES),
    env.work,
  );
  await env.work.frame();
  const shown = [...rows()];
  await env.report("redis_scroll", {
    expandMs: ms(expandMs),
    viewport: { width: s.clientWidth, height: s.clientHeight },
    scrollHeight: s.scrollHeight,
    lastKey: shown.at(-1)?.getAttribute("data-key") ?? null,
    fling,
  });
}

/** `db` 시나리오. */
export async function runDbScenario(env: ScenarioEnv): Promise<void> {
  let current = "start";
  const step = (s: string) => void (current = s);
  try {
    step("project window");
    await projectWindow(env);
    step("explorer");
    await openExplorer(env);
    const has = (id: string) => connectionRow(env.doc, id) !== null;
    const grid = has(GRID_CONNECTION);
    const schema = has(SCHEMA_CONNECTION);
    const redis = has(REDIS_CONNECTION);
    if (!grid && !schema && !redis)
      throw new Error(
        `none of ${GRID_CONNECTION}, ${SCHEMA_CONNECTION}, ${REDIS_CONNECTION} is in the explorer`,
      );
    if (grid) {
      step("explorer close");
      await closeExplorer(env);
      await gridSteps(env, step);
      if (schema || redis) {
        step("explorer");
        await openExplorer(env);
      }
    }
    if (schema) await explorerSteps(env, step);
    if (redis) await redisSteps(env, step);
    await env.report("scenario_done", { grid, schema, redis });
  } catch (e) {
    await env.report("scenario_error", {
      step: current,
      message: e instanceof Error ? e.message : String(e),
    });
  }
}

/** `db-idle` 시나리오. */
export async function runDbIdleScenario(env: ScenarioEnv): Promise<void> {
  try {
    await projectWindow(env);
    const started = env.now();
    const console_ = await openConsole(env, IDLE_CONNECTION);
    await env.report("db_connected", {
      ms: ms(env.now() - started),
      session: console_.dataset.session ?? null,
      epochMs: env.epochNow(),
    });
    await env.report("scenario_done", {});
  } catch (e) {
    await env.report("scenario_error", {
      step: "db_connected",
      message: e instanceof Error ? e.message : String(e),
    });
  }
}
