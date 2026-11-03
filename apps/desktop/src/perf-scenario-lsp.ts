// 앱 안 LSP · 심볼 측정 시나리오(release 번들, `crates/pw-perf` 의 LSP 측정 — CONTRIBUTING.md 「성능 기준선」 의 「LSP 시나리오」).
// perf-scenario.ts 가 측정 모드 프로젝트 창에서 첫 프레임 뒤에 부른다. 15-m4-plan.md M4-19 가 release 번들에서 재는 것 — 06 「성능
// 목표」 의 LSP · 편집 행(결정 M4-D19)과 심볼로 이동 · 클래스로 이동(M4-16) — 을 잰다. 키 · 클릭은 모두 웹뷰 안에서 만든 이벤트다
// (OS 키 입력이 아니다). 글자는 에디터에 `view.dispatch`(`input.type`)로 넣는다 — 입력기 · 키 반복 · 키맵을 거치는 입력은 들지 않는다.
//
// 언어 서버는 측정하는 쪽이 전역 설정 `[lsp.rust-analyzer] path` 로 끼운 가짜(`pw-perf-lsp`, crates/pw-perf/src/lsp_fake.rs)다. 가짜는
// 곧바로 답하고 보낸 때의 벽시계를 표지 파일에 남긴다 — 「서버 발행 → 밑줄」 · 「응답 → 대상 위치」 는 측정하는 쪽이 그 벽시계와 여기서
// 남긴 「그 프레임의 벽시계」(`shownEpochMs`)로 낸다(git 시나리오의 체크아웃 → 상태바와 같은 방법).
//
// `lsp` 시나리오 — 픽스처(`crates/pw-perf/src/lsp.rs` 의 `write_fixture`)의 `MAIN_PATH` 를 프로젝트 트리에서 열고 서버가 준비되면:
//
// 1. `lsp_ready`: 더블클릭 → 상태바의 LSP 표지가 준비(`data-state=ready`)인 때까지(참고).
// 2. `completion`(06 「자동완성」): `COMPLETION_LINE` 끝에 `.`(서버의 트리거 글자)을 넣은 때부터 자동완성 팝업에 서버의 첫 항목이
//    그려진 다음 프레임까지, `SAMPLES` 번. 매번 팝업을 닫고 글자를 지운다.
// 3. `diagnostics`(06 「진단 표시」): `DIAG_LINE` 끝에 가짜가 진단을 다는 낱말(`DIAG_MARK`)을 넣고, 그 줄에 밑줄(`.pw-diagnostic`)이
//    그려진 다음 프레임의 벽시계를 적는다(`editEpochMs` 는 넣기 직전). 낱말을 지우고 밑줄이 사라질 때까지 기다린다. `SAMPLES` 번.
// 4. `definition`(06 「정의로 이동」): `DEF_LINE` 의 `target_fn` 에서 선언으로 이동(⌘B 의 함수 `gotoDeclaration` 을 직접 부른다 —
//    키맵 처리는 들지 않는다). 가짜는 요청마다 다른 파일(`src/t<k>.rs`)의 `DEF_TARGET_LINE` 을 답한다 — 대상 파일의 에디터가 보이고
//    커서가 그 줄에 있는 다음 프레임의 벽시계를 적는다. ⌘[(웹뷰 안 이벤트)로 돌아온다. `DEFINITION_SAMPLES` 번(파일마다 처음 연다).
// 5. `problems_loaded` → `problems_open` → `problems_scroll`(06 「문제 툴 윈도우」): `PROBLEMS_PATH` 를 열면 가짜가 `src/p/` 의 파일
//    2,000개에 진단 5개씩(1만 개)을 보낸다. 상태바의 개수가 다 찬 뒤(`problems_loaded`), ⌘6 keydown 부터 문제 툴 윈도우에 첫 줄이 그려진
//    다음 프레임까지(`problems_open` — 「프로젝트」 범위가 기본), 그 목록을 처음부터 끝까지 `PROBLEMS_FLING_FRAMES` 프레임에 스크롤한다.
//
// `lsp-typing` · `lsp-typing-off` 시나리오(06 「타이핑 지연」): `MAIN_PATH` 를 열고(켬이면 서버 준비까지) `TYPING_LINE` 끝에 `TYPING_TEXT`
// 를 되풀이해 `TYPING_KEYS` 자를 `KEY_MS` 마다 넣는다. 키마다 「넣기 직전 → 다음 rAF」(`frame`)와 「→ 그 프레임 뒤 첫 작업」(`painted` —
// 그 프레임의 그리기가 끝난 뒤)을 잰다. 끝나면 넣은 글자를 지운다. 켬 · 끔은 측정하는 쪽이 설정으로 가른다 — 끔은 서버를 띄우지 않는다.
//
// `symbols` · `classes` 시나리오(06 「심볼 인덱스」, M4-16): 합성 저장소(파일 2만 · 심볼 50만)를 연 창에서 `index.status`(이 창과 같은
// 프로젝트의 Host 연결)가 준비(`ready` · 다시 읽을 파일 0)가 될 때까지 기다려 `index_ready` 를 적는다. 그 뒤 기다림 없이 첫 팝업 —
// `symbols` 는 ⌘⌥O, `classes` 는 ⌘O — 을 열어 질의를 한 글자씩 친다: 키마다 「입력 이벤트 → 그 질의의 결과 줄이 DOM 에 든 다음
// 프레임」(`totalMs`)과 그 안의 `index.symbols.search` 왕복(`rpcMs` — Host + IPC, `RpcClient.call` 을 감싸 잰다). 첫 키가 「창을 연 뒤
// 첫 검색의 첫 키」 다. 팝업을 닫고 다른 쪽 팝업도 같은 질의로 잰다(`symbol_keys_second` — 첫 사용이 아니다).
//
// `lsp-idle` 시나리오(이 저장소 + rust-analyzer): `IDLE_PATH` 를 열고 서버가 준비 상태로 `IDLE_READY_HOLD_MS` 동안 머물면 `server_ready`
// 를 적고 끝난다(측정하는 쪽이 그때부터 유휴 시간을 기다려 메모리를 잰다).
//
// 끝나면 `scenario_done`, 단계가 실패하면 `scenario_error { step, message }` 하나를 남기고 멈춘다.

import { closeCompletion } from "@codemirror/autocomplete";
import { EditorView } from "@codemirror/view";
import { RpcClient } from "@pitwall/rpc-client";

import { gotoDeclaration } from "./lsp/navigation";
import { frameStats, scrollFrames } from "./perf-frames";
import { type ScenarioEnv, treeRow, until } from "./perf-scenario";
import { connectHost } from "./tauri-transport";

/** 픽스처의 파일(측정하는 쪽 `crates/pw-perf/src/lsp.rs` 와 같은 값 — 시험이 같은지 본다). */
export const MAIN_PATH = "src/main.rs";
export const MAIN_FIRST_LINE = "// pitwall perf: lsp target";
export const PROBLEMS_PATH = "src/problems.rs";
/** 1 부터 센 줄. */
export const COMPLETION_LINE = 20;
export const DIAG_LINE = 30;
export const DEF_LINE = 40;
export const TYPING_LINE = 50;
/** `DEF_LINE` 에서 이동할 낱말. */
export const DEF_WORD = "target_fn";
/** 가짜 서버가 진단을 다는 낱말(`lsp_fake.rs` 의 `DIAG_MARK`). */
export const DIAG_MARK = "perf_diag";
/** 가짜 서버의 완성 항목 이름 머리(`lsp_fake.rs` 의 `COMPLETION_LABEL`). */
export const COMPLETION_LABEL = "perfItem";
/** 정의 대상 파일 `src/t<k>.rs` 의 첫 줄 머리와 가짜가 답하는 줄(1 부터). */
export const TARGET_FIRST_LINE = "// pitwall perf: definition target";
export const DEF_TARGET_LINE = 401;
export const DEF_TARGETS = 10;
/** 문제 툴 윈도우: 가짜가 보내는 진단(파일 2,000 × 5 — 심각도를 돌려 가며: 오류 2 · 경고 1 · 정보 1 · 힌트 1). */
export const PROBLEM_FILES = 2_000;
export const PROBLEMS_PER_FILE = 5;
export const PROBLEM_ERRORS = PROBLEM_FILES * 2;
export const PROBLEM_WARNINGS = PROBLEM_FILES;
export const PROBLEMS_FLING_FRAMES = 1500;

export const SAMPLES = 20;
export const DEFINITION_SAMPLES = DEF_TARGETS;
/** 표본 사이 쉼 — 앞 표본의 뒤처리(진단 다시 보내기 · 팝업 닫기)가 끝나도록. */
export const SETTLE_MS = 150;

/** 타이핑: 초당 20자 15초. 글은 낱말 · `.`(완성 트리거) · 공백이 섞인 것을 되풀이한다. */
export const TYPING_KEYS = 300;
export const KEY_MS = 50;
export const TYPING_TEXT = "item.perfItem3 + value ";

/** 심볼 인덱스: 질의(합성 저장소 `crates/pw-index/tests/perf_repo.rs` 의 이름 — M4-15 · M4-16 의 질의와 같다). */
export const SYMBOL_QUERY = "handleItem1234x5";
export const CLASS_QUERY = "Widget1234";
export const INDEX_POLL_MS = 50;
export const INDEX_TIMEOUT_MS = 180_000;

/** 이 저장소를 연 실측(`lsp-idle`): 열 파일과 서버가 준비로 머물러야 하는 시간. */
export const IDLE_PATH = "crates/pw-core/src/lib.rs";
export const IDLE_READY_HOLD_MS = 15_000;
export const IDLE_TIMEOUT_MS = 20 * 60_000;

const ms = (x: number) => Math.round(x * 10) / 10;

const keydown = (code: string, key: string, mods: { alt?: boolean } = {}) =>
  new KeyboardEvent("keydown", {
    code,
    key,
    metaKey: true,
    altKey: mods.alt ?? false,
    bubbles: true,
    cancelable: true,
  });

function dblclick(el: Element) {
  el.dispatchEvent(new MouseEvent("dblclick", { bubbles: true, cancelable: true }));
}

/** 이 창의 프로젝트 id(주소의 `project`). */
function projectOf(doc: Document): string {
  const id = new URLSearchParams(doc.defaultView?.location.search ?? "").get("project");
  if (!id) throw new Error("the window address has no project id");
  return id;
}

const visible = (el: Element) => el.getClientRects().length > 0;

/** 첫 줄이 `first` 로 시작하는, 보이는 에디터. */
export function editorStartingWith(doc: Document, first: string): EditorView | undefined {
  for (const dom of doc.querySelectorAll<HTMLElement>(".cm-editor")) {
    if (!visible(dom)) continue;
    const view = EditorView.findFromDOM(dom);
    if (view && view.state.doc.line(1).text.startsWith(first)) return view;
  }
  return undefined;
}

/** 프로젝트 트리에서 `path`(`a/b/c.rs`)를 연다 — 닫힌 폴더는 차례로 더블클릭해 펼친다. */
async function openInTree(env: ScenarioEnv, path: string) {
  const parts = path.split("/");
  for (let i = 1; i < parts.length; i++) {
    const folder = parts.slice(0, i).join("/");
    const child = parts.slice(0, i + 1).join("/");
    if (treeRow(env.doc, child)) continue;
    dblclick(await until(env, `${folder} in the project tree`, () => treeRow(env.doc, folder)));
    await until(env, `${child} in the project tree`, () => treeRow(env.doc, child), 30_000);
  }
  dblclick(await until(env, `${path} in the project tree`, () => treeRow(env.doc, path)));
}

const lspState = (doc: Document) =>
  doc.querySelector<HTMLElement>("[data-testid=status-lsp]")?.getAttribute("data-state") ??
  undefined;

async function openMain(env: ScenarioEnv, lsp: boolean): Promise<EditorView> {
  const started = env.now();
  await openInTree(env, MAIN_PATH);
  const view = await until(env, `the editor of ${MAIN_PATH}`, () =>
    editorStartingWith(env.doc, MAIN_FIRST_LINE),
  );
  if (lsp) {
    await until(
      env,
      "the language server to be ready",
      () => lspState(env.doc) === "ready",
      60_000,
    );
    await env.report("lsp_ready", { ms: ms(env.now() - started) });
  }
  return view;
}

/** 줄 `line`(1 부터) 끝에 `text` 를 넣는다(저장 안 함). 되돌리는 함수를 돌려준다. */
function appendToLine(view: EditorView, line: number, text: string): () => void {
  const at = view.state.doc.line(line).to;
  view.dispatch({
    changes: { from: at, insert: text },
    selection: { anchor: at + text.length },
    userEvent: "input.type",
  });
  return () => view.dispatch({ changes: { from: at, to: at + text.length }, userEvent: "delete" });
}

/** 자동완성 팝업에 서버의 첫 항목이 있는가. */
const completionShown = (view: EditorView) => {
  const popup = view.dom.querySelector(".cm-tooltip-autocomplete");
  return popup !== null && (popup.textContent ?? "").includes(`${COMPLETION_LABEL}0`);
};

async function completion(env: ScenarioEnv, view: EditorView) {
  const samples: number[] = [];
  for (let i = 0; i < SAMPLES; i++) {
    const started = env.now();
    const revert = appendToLine(view, COMPLETION_LINE, ".");
    await until(env, "the completion popup", () => completionShown(view), 10_000);
    await env.work.frame();
    samples.push(ms(env.now() - started));
    closeCompletion(view);
    revert();
    await until(env, "the completion popup to close", () => !completionShown(view), 10_000);
    await env.sleep(SETTLE_MS);
  }
  await env.report("completion", { samples });
}

/** 줄 `line`(1 부터)에 진단 밑줄이 그려졌는가. */
const underlined = (view: EditorView, line: number) => {
  for (const el of view.dom.querySelectorAll(".pw-diagnostic")) {
    const at = view.posAtDOM(el);
    if (view.state.doc.lineAt(at).number === line) return true;
  }
  return false;
};

async function diagnostics(env: ScenarioEnv, view: EditorView) {
  const samples: { editEpochMs: number; shownEpochMs: number }[] = [];
  for (let i = 0; i < SAMPLES; i++) {
    const editEpochMs = env.epochNow();
    const revert = appendToLine(view, DIAG_LINE, ` ${DIAG_MARK}`);
    await until(env, "the diagnostic underline", () => underlined(view, DIAG_LINE), 10_000);
    await env.work.frame();
    samples.push({ editEpochMs, shownEpochMs: env.epochNow() });
    revert();
    await until(env, "the underline to go", () => !underlined(view, DIAG_LINE), 10_000);
    await env.sleep(SETTLE_MS);
  }
  await env.report("diagnostics", { samples });
}

const targetPath = (k: number) => `src/t${k}.rs`;

async function definition(env: ScenarioEnv, view: EditorView) {
  const samples: { askEpochMs: number; shownEpochMs: number; path: string }[] = [];
  const line = view.state.doc.line(DEF_LINE);
  const offset = line.from + line.text.indexOf(DEF_WORD) + 1;
  if (offset <= line.from) throw new Error(`line ${DEF_LINE} has no ${DEF_WORD}`);
  for (let k = 0; k < DEFINITION_SAMPLES; k++) {
    const head = `${TARGET_FIRST_LINE} ${k}`;
    const askEpochMs = env.epochNow();
    void gotoDeclaration(view, offset);
    await until(
      env,
      `${targetPath(k)} at line ${DEF_TARGET_LINE}`,
      () => {
        const target = editorStartingWith(env.doc, head);
        if (!target) return false;
        return target.state.doc.lineAt(target.state.selection.main.head).number === DEF_TARGET_LINE;
      },
      10_000,
    );
    await env.work.frame();
    samples.push({ askEpochMs, shownEpochMs: env.epochNow(), path: targetPath(k) });
    // 돌아온다(⌘[ — 웹뷰 안 이벤트). 처음 누름이 키맵이 붙기 전이면 다시 넣는다.
    let pressed = -Infinity;
    await until(env, `back to ${MAIN_PATH}`, () => {
      if (editorStartingWith(env.doc, MAIN_FIRST_LINE)) return true;
      if (env.now() - pressed > 1000) {
        env.doc.body.dispatchEvent(keydown("BracketLeft", "["));
        pressed = env.now();
      }
      return false;
    });
    await env.sleep(SETTLE_MS);
  }
  await env.report("definition", { samples });
}

/** 상태바의 문제 개수(오류 · 경고). */
function problemCounts(doc: Document) {
  const read = (severity: string) =>
    Number(
      (
        doc.querySelector(`[data-testid=status-problems] [data-count=${severity}]`)?.textContent ??
        ""
      ).replace(/\D/g, "") || "0",
    );
  return { errors: read("error"), warnings: read("warning") };
}

const problemsList = (doc: Document) =>
  doc.querySelector<HTMLElement>("[data-tool-window=problems] [data-keymap-context=problems]");

async function problems(env: ScenarioEnv) {
  const { doc } = env;
  const started = env.now();
  await openInTree(env, PROBLEMS_PATH);
  await until(
    env,
    `${PROBLEM_ERRORS} errors and ${PROBLEM_WARNINGS} warnings in the status bar`,
    () => {
      const c = problemCounts(doc);
      return c.errors === PROBLEM_ERRORS && c.warnings === PROBLEM_WARNINGS;
    },
    60_000,
  );
  await env.report("problems_loaded", { ms: ms(env.now() - started) });
  await env.sleep(500);
  const opened = env.now();
  doc.body.dispatchEvent(keydown("Digit6", "6"));
  const list = await until(
    env,
    "the first problem row",
    () => {
      const l = problemsList(doc);
      return l?.querySelector("[data-problem-row]") ? l : undefined;
    },
    30_000,
  );
  await env.work.frame();
  await env.report("problems_open", {
    ms: ms(env.now() - opened),
    rows: list.querySelectorAll("[data-problem-row]").length,
    viewport: { width: list.clientWidth, height: list.clientHeight },
  });
  // 모든 줄(파일 2,000 + 진단 1만)이 목록 높이에 들어올 때까지.
  await until(
    env,
    "the full problems list",
    () => list.scrollHeight > list.clientHeight * 10,
    30_000,
  );
  await env.sleep(300);
  const end = list.scrollHeight - list.clientHeight;
  const scroll = await scrollFrames(
    list,
    PROBLEMS_FLING_FRAMES,
    (f) => Math.round(((f + 1) / PROBLEMS_FLING_FRAMES) * end),
    env.work,
  );
  await env.report("problems_scroll", {
    ...scroll,
    scrollHeight: list.scrollHeight,
    viewport: { width: list.clientWidth, height: list.clientHeight },
  });
}

/** 그 프레임의 그리기 뒤 첫 작업까지 — rAF 콜백에서 건 메시지는 그 프레임의 렌더 갱신 뒤에 돈다. */
function afterPaint(env: ScenarioEnv): Promise<{ frame: number; painted: number }> {
  return new Promise((resolve) => {
    void env.work.frame().then(() => {
      const frame = env.now();
      const channel = new MessageChannel();
      channel.port1.onmessage = () => resolve({ frame, painted: env.now() });
      channel.port2.postMessage(undefined);
    });
  });
}

async function typing(env: ScenarioEnv, view: EditorView, lsp: boolean) {
  const start = view.state.doc.line(TYPING_LINE).to;
  view.dispatch({ selection: { anchor: start } });
  await env.sleep(500);
  const frame: number[] = [];
  const painted: number[] = [];
  for (let key = 0; key < TYPING_KEYS; key++) {
    const at = start + key;
    const ch = TYPING_TEXT[key % TYPING_TEXT.length]!;
    const t = env.now();
    view.dispatch({
      changes: { from: at, insert: ch },
      selection: { anchor: at + 1 },
      userEvent: "input.type",
    });
    const seen = await afterPaint(env);
    frame.push(seen.frame - t);
    painted.push(seen.painted - t);
    await env.sleep(KEY_MS);
  }
  closeCompletion(view);
  view.dispatch({ changes: { from: start, to: start + TYPING_KEYS }, userEvent: "delete" });
  await env.report("typing", {
    lsp,
    keys: TYPING_KEYS,
    frame: frameStats(frame),
    painted: frameStats(painted),
  });
}

/** `index.symbols.search` 의 왕복(Host + IPC)을 질의별로 적는다 — `RpcClient.call` 을 감싼다. `restore` 로 걷는다. */
export function watchSymbolCalls(now: () => number) {
  const proto = RpcClient.prototype as unknown as {
    call: (method: string, ...rest: unknown[]) => Promise<unknown>;
  };
  const call = proto.call;
  const times = new Map<string, number>();
  proto.call = function (this: unknown, method: string, ...rest: unknown[]) {
    const result = call.call(this, method, ...rest);
    if (method !== "index.symbols.search") return result;
    const query = (rest[0] as { query?: string } | undefined)?.query ?? "";
    const started = now();
    void result.then(
      () => times.set(query, now() - started),
      () => undefined,
    );
    return result;
  };
  return {
    rpc: (query: string) => times.get(query),
    restore: () => void (proto.call = call),
  };
}

async function indexReady(env: ScenarioEnv, frameEpochMs: number) {
  const projectId = projectOf(env.doc);
  const conn = await connectHost({ projectId });
  const client = new RpcClient(conn.transport);
  try {
    let last: { state: string; files: number; symbols: number; pending: number } | undefined;
    const started = env.now();
    for (;;) {
      const status = await client.call("index.status", { projectId });
      const root = status.roots[0];
      if (root) last = root;
      if (root && root.state === "ready" && root.pending === 0) break;
      if (env.now() - started > INDEX_TIMEOUT_MS)
        throw new Error(`the symbol index is not ready: ${JSON.stringify(last)}`);
      await env.sleep(INDEX_POLL_MS);
    }
    const epochMs = env.epochNow();
    await env.report("index_ready", {
      epochMs,
      fromFrameMs: ms(epochMs - frameEpochMs),
      files: last!.files,
      symbols: last!.symbols,
    });
  } finally {
    client.close();
    void conn.close();
  }
}

const seInput = (doc: Document) =>
  doc.querySelector<HTMLInputElement>("[data-search-everywhere] [cmdk-input]");

function typeInto(input: HTMLInputElement, value: string) {
  const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
  set.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

/** 팝업 하나를 열고(기다림 없이) 질의를 한 글자씩 친다. */
async function symbolKeys(
  env: ScenarioEnv,
  popup: "symbols" | "classes",
  calls: ReturnType<typeof watchSymbolCalls>,
) {
  const { doc } = env;
  const query = popup === "symbols" ? SYMBOL_QUERY : CLASS_QUERY;
  const opened = env.now();
  doc.body.dispatchEvent(keydown("KeyO", "o", { alt: popup === "symbols" }));
  const input = await until(env, `the ${popup} popup`, () => seInput(doc), 10_000);
  const openMs = env.now() - opened;
  const keys: { query: string; totalMs: number; rpcMs: number | null }[] = [];
  for (let n = 1; n <= query.length; n++) {
    const q = query.slice(0, n);
    const started = env.now();
    typeInto(input, q);
    await until(
      env,
      `results for ${JSON.stringify(q)}`,
      () => doc.querySelector(`[data-search-everywhere] [data-symbol-query="${CSS.escape(q)}"]`),
      30_000,
    );
    await env.work.frame();
    const rpc = calls.rpc(q);
    keys.push({
      query: q,
      totalMs: ms(env.now() - started),
      rpcMs: rpc === undefined ? null : ms(rpc),
    });
  }
  // 닫는다(Esc — 팝업이 받는다).
  input.dispatchEvent(
    new KeyboardEvent("keydown", { key: "Escape", code: "Escape", bubbles: true }),
  );
  await until(env, `the ${popup} popup to close`, () => !seInput(doc), 10_000);
  return { popup, openMs: ms(openMs), first: keys[0], keys };
}

async function symbolScenario(
  env: ScenarioEnv,
  first: "symbols" | "classes",
  frameEpochMs: number,
) {
  await indexReady(env, frameEpochMs);
  const calls = watchSymbolCalls(env.now);
  try {
    await env.report("symbol_keys", await symbolKeys(env, first, calls));
    await env.sleep(500);
    const second = first === "symbols" ? "classes" : "symbols";
    await env.report("symbol_keys_second", await symbolKeys(env, second, calls));
  } finally {
    calls.restore();
  }
}

async function serverIdle(env: ScenarioEnv, frameEpochMs: number) {
  await openInTree(env, IDLE_PATH);
  let since: number | undefined;
  await until(
    env,
    `the language server to stay ready for ${IDLE_READY_HOLD_MS} ms`,
    () => {
      if (lspState(env.doc) !== "ready") {
        since = undefined;
        return false;
      }
      since ??= env.now();
      return env.now() - since >= IDLE_READY_HOLD_MS;
    },
    IDLE_TIMEOUT_MS,
  );
  const epochMs = env.epochNow();
  await env.report("server_ready", { epochMs, fromFrameMs: ms(epochMs - frameEpochMs) });
}

export type LspScenario =
  | "lsp"
  | "lsp-typing"
  | "lsp-typing-off"
  | "lsp-idle"
  | "symbols"
  | "classes";

export async function runLspScenario(env: ScenarioEnv, scenario: LspScenario): Promise<void> {
  const frameEpochMs = env.epochNow();
  let step = "start";
  try {
    step = "project window";
    await until(env, "the project window", () =>
      env.doc.querySelector("[data-tool-window=project]"),
    );
    // 키맵이 붙고 첫 그림의 뒤처리가 끝나도록(git 시나리오와 같다).
    await env.sleep(300);
    if (scenario === "lsp") {
      step = "open";
      const view = await openMain(env, true);
      step = "completion";
      await completion(env, view);
      step = "diagnostics";
      await diagnostics(env, view);
      step = "definition";
      await definition(env, view);
      step = "problems";
      await problems(env);
    } else if (scenario === "lsp-typing" || scenario === "lsp-typing-off") {
      const lsp = scenario === "lsp-typing";
      step = "open";
      const view = await openMain(env, lsp);
      if (!lsp) {
        await env.sleep(1000);
        if (lspState(env.doc) === "ready")
          throw new Error("a language server runs in the LSP-off run");
      }
      step = "typing";
      await typing(env, view, lsp);
    } else if (scenario === "lsp-idle") {
      step = "server";
      await serverIdle(env, frameEpochMs);
    } else {
      step = "symbols";
      await symbolScenario(env, scenario, frameEpochMs);
    }
    await env.report("scenario_done", {});
  } catch (e) {
    await env.report("scenario_error", {
      step,
      message: e instanceof Error ? e.message : String(e),
    });
  }
}
