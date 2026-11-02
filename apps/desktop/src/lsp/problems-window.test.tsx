// @vitest-environment happy-dom
// 진단의 창 쪽(15-m4-plan M4-06)을 프로젝트 창(App kind="project")과 가짜 Host 로 본다 — 문제 툴 윈도우가 진단 1만 개 · 파일
// 2천 개에서 보이는 줄만 그리고(④ — DOM 줄 수 상한), 상태바의 수가 `lsp.diagnostics.counts` 의 답과 같고(⑥) 이벤트를 따라가고,
// 줄을 열면 그 위치로 가고(위치로 가기), 에디터의 `F2` 가 키맵으로 다음 오류에 간다(⑤). 창 틀(가짜 Host · 마운트)은
// lsp-window.test.tsx 와 같고, 진단 RPC(`lsp.diagnostics.*`)만 `host.diagnostics` 로 더했다.
//
// CodeMirror 는 불러올 때 macOS 인지 정한다(navigator.platform) — 그래서 불러오기 전에 Mac 으로 둔다.

import { EditorView } from "@codemirror/view";
import type {
  ConfigView,
  JsonValue,
  FsRead,
  FsStat,
  HostInfo,
  KeymapFile,
  LangDetectResult,
  LangList,
  Project,
  ProjectTrust,
} from "@pitwall/rpc-client";
import type { LspDiagnostic, LspDiagnosticsCounts, LspDiagnosticsList } from "@pitwall/rpc-client";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, vi } from "vitest";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../App";
import type { KeymapNative } from "../keymap/native";
import { activeWorktreeStore } from "../open-with/active-worktree";
import { projectOpenInbox } from "../project-open";
import { createThemeStore } from "../theme/theme-store";
import { act, endTest, it } from "../test-support/ended-test";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";
import type { fakeSyntaxServer } from "../editor/syntax-testing";
import { fakeLspServer } from "./lsp-testing";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

vi.hoisted(() => {
  Object.defineProperty(navigator, "platform", { value: "MacIntel", configurable: true });
});

const host = vi.hoisted(() => ({
  files: new Map<string, string>(),
  /** 전역 settings.toml 의 값(`config.get`). */
  settings: {} as JsonValue,
  subscribers: [] as {
    send(m: object): void;
    id: number;
    topics: string[];
    projectId?: string;
  }[],
  /** 구문 분석 서비스(`syntax.*`). 없으면 그 메서드가 없는 Host 다(강조하지 않는다). */
  syntax: undefined as ReturnType<typeof fakeSyntaxServer> | undefined,
  /** 서버 관리자(`lsp.document.*`). */
  lsp: undefined as ReturnType<typeof fakeLspServer> | undefined,
  /** 크기 등급을 바꿀 파일(5 MiB 초과의 읽기 전용 등). */
  sizeClass: new Map<string, "editable" | "readOnly">(),
  /** 외부로 연 주소(`openUrl`). */
  opened: [] as string[],
  /** 진단 RPC(`lsp.diagnostics.list` · `counts`)의 처리기. */
  diagnostics: {} as Record<string, (params: never) => unknown>,
  /** 받은 진단 RPC(메서드, 매개변수). */
  diagnosticCalls: [] as [string, unknown][],
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: async (url: string) => void host.opened.push(url),
}));

const LANGUAGES: LangList = {
  languages: [
    { name: "Python", popular: true, lineComment: "#" },
    { name: "Rust", popular: true, lineComment: "//" },
    { name: "COBOL", popular: false, lineComment: null },
    { name: "Text", popular: false, lineComment: null },
  ],
};

vi.mock("../tauri-transport", async () => {
  const { createMemoryTransportPair: pair } = await import("@pitwall/rpc-client");
  const { DEFAULT_KEYMAP_TOML } = await import("../keymap/default-keymap");
  const { i18nState } = await import("../i18n/testing");
  return {
    connectHost: async () => {
      const [client, server] = pair();
      let next = 1;
      const send = (m: object) => server.send(JSON.stringify({ jsonrpc: "2.0", ...m }));
      server.onMessage((text) => {
        const m = JSON.parse(text) as { id?: number; method: string; params: never };
        const reply = (result: unknown) => send({ id: m.id, result });
        const path = (m.params as { path?: string } | undefined)?.path ?? "";
        if (m.method.startsWith("syntax.") && host.syntax) {
          const { source } = host.syntax;
          const run = {
            "syntax.open": source.open,
            "syntax.edit": source.edit,
            "syntax.highlights": source.highlights,
            "syntax.close": source.close,
          }[m.method] as (p: unknown) => Promise<unknown>;
          return void run(m.params).then(reply, (e: { code?: number; message: string }) =>
            send({ id: m.id, error: { code: e.code ?? -32603, message: e.message } }),
          );
        }
        if (m.method.startsWith("lsp.diagnostics.")) {
          host.diagnosticCalls.push([m.method, m.params]);
          const handler = host.diagnostics[m.method];
          if (!handler)
            return send({ id: m.id, error: { code: -32601, message: `no ${m.method}` } });
          return void Promise.resolve(handler(m.params)).then(reply);
        }
        if (m.method.startsWith("lsp.document.") && host.lsp) {
          const { source } = host.lsp;
          const run = {
            "lsp.document.open": source.open,
            "lsp.document.change": source.change,
            "lsp.document.save": source.save,
            "lsp.document.close": source.close,
            "lsp.document.capabilities": source.capabilities,
          }[m.method] as (p: unknown) => Promise<unknown>;
          return void run(m.params).then(reply, (e: { code?: number; message: string }) =>
            send({ id: m.id, error: { code: e.code ?? -32603, message: e.message } }),
          );
        }
        switch (m.method) {
          case "i18n.get":
            return reply(i18nState("ko"));
          case "config.get":
            return reply({
              snapshot: {
                files: [{ id: "settings", value: host.settings, layers: [] }],
                notices: [],
              },
              error: null,
            } satisfies ConfigView);
          case "keymap.get":
            return reply({
              id: "pitwall-default",
              extension: "dev.pitwall.keymap-default",
              kind: "base",
              content: DEFAULT_KEYMAP_TOML,
            } satisfies KeymapFile);
          // 상태바의 브랜치 항목(src/git/branch-state.ts)이 묻는다 — 이 시험은 저장소 없음으로 답한다.
          case "git.refs":
            return reply({
              workTree: null,
              head: null,
              branches: [],
              remoteBranches: [],
              tags: [],
              unsupported: null,
              broken: [],
            });
          case "git.operation":
            return reply({ operation: null });
          case "host.info":
            return reply({
              hostId: "local",
              kind: "local",
              version: "0",
              os: "t",
              arch: "t",
            } satisfies HostInfo);
          case "task.list":
            return reply([]);
          case "project.get":
            return reply({
              id: "p1",
              name: "demo",
              host: "local",
              roots: ["/work/demo"],
              favorite: false,
              tags: [],
              configMode: "repository",
              slug: "demo",
              createdAt: 1,
              openedAt: null,
            } satisfies Project);
          case "project.trust":
            return reply({
              projectId: "p1",
              ownerId: "p1",
              state: "trusted",
            } satisfies ProjectTrust);
          case "project.storage":
            return reply({ kind: "ok", path: "/data/state.db" });
          case "project.claimTrustPrompt":
            return reply(false);
          case "fs.stat":
            return reply({
              path,
              kind: "file",
              target: null,
              size: host.files.get(path)!.length,
              mtimeMs: 1,
              readonly: false,
              sizeClass: host.sizeClass.get(path) ?? "editable",
            } satisfies FsStat);
          case "fs.read":
            return reply({
              path,
              size: host.files.get(path)!.length,
              mtimeMs: 1,
              offset: 0,
              eof: true,
              version: "1",
              content: { kind: "text", text: host.files.get(path)! },
              judgment: {
                binary: false,
                encoding: "UTF-8",
                lossy: false,
                bom: null,
                lineEnding: "lf",
                longLines: false,
                sizeClass: host.sizeClass.get(path) ?? "editable",
                editable: (host.sizeClass.get(path) ?? "editable") === "editable",
                highlight: true,
              },
            } satisfies FsRead);
          case "lang.detect": {
            const { files } = m.params as { files: { path: string }[] };
            return reply({
              files: files.map((f) => ({
                language: f.path.endsWith(".rs")
                  ? "Rust"
                  : f.path.endsWith(".py")
                    ? "Python"
                    : null,
                source: "extension",
                vendored: false,
                generated: false,
                documentation: false,
              })),
            } satisfies LangDetectResult);
          }
          case "lang.list":
            return reply(LANGUAGES);
          case "events.subscribe": {
            const id = next++;
            const p = m.params as { topics: string[]; projectId?: string };
            host.subscribers.push({ send, id, topics: p.topics, projectId: p.projectId });
            return reply({ subscriptionId: id });
          }
          case "events.unsubscribe":
            return reply(null);
          default:
            return send({ id: m.id, error: { code: -32601, message: `no ${m.method}` } });
        }
      });
      return { transport: client, close: async () => {} };
    },
  };
});

/** 네이티브 메뉴(src-tauri/src/menu.rs): 마지막으로 받은 accelerator 와 메뉴에서 고른 액션을 보내는 자리. */
const menu = {
  accelerators: {} as Record<string, string | null>,
  choose: undefined as ((action: string) => void) | undefined,
};

const native = {
  syncMenu: async (accelerators) => {
    menu.accelerators = accelerators;
  },
  onMenuAction: (listener) => {
    menu.choose = listener;
    return () => {
      if (menu.choose === listener) menu.choose = undefined;
    };
  },
  openSettings: async () => {},
  closeWindow: async () => {},
  guardClose: () => () => {},
} satisfies KeymapNative;

/** 낱말 `foo` 셋. */
const WORDS = "/work/demo/words.py";
const MAIN = "/work/demo/src/main.rs";
/** 크기 등급이 읽기 전용(5 MiB 초과로 판정). */
const BIG = "/work/demo/big.py";
/** 언어가 없는 파일(플레인 텍스트). */
const NOTES = "/work/demo/notes.txt";

let root: Root | undefined;

function useUserAgent(ua: string) {
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(ua);
}

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  useUserAgent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)");
  vi.spyOn(console, "info").mockImplementation(() => {});
  window.history.replaceState(null, "", "/?window=project&project=p1");
  host.files = new Map([
    [WORDS, "foo = 1\nbar = foo\nfoobar = foo\n"],
    [MAIN, "fn main() {}\n"],
    [BIG, "x = 1\n"],
    [NOTES, "hello\n"],
  ]);
  host.settings = {};
  host.subscribers = [];
  host.syntax = undefined;
  host.lsp = fakeLspServer();
  host.sizeClass = new Map([[BIG, "readOnly"]]);
  host.opened = [];
  host.diagnostics = {};
  host.diagnosticCalls = [];
  activeWorktreeStore.setState({ top: undefined });
  localStorage.clear();
  // happy-dom 은 DOM 선택을 바꾸는 그 자리에서 `selectionchange` 를 동기로 보낸다(브라우저는 나중에 보낸다). 줄 수가 바뀌는 편집이면
  // CodeMirror 가 자기 업데이트 도중에 자기가 둔 선택의 이벤트를 받아 「update in progress」 로 멈춘다. 브라우저에서는 그 이벤트가
  // 업데이트 뒤에 와 아무것도 바꾸지 않으므로, 업데이트 중에 온 것은 버린다(CodeMirror 의 `updateState`: 0 = 쉼).
  const dispatch = document.dispatchEvent.bind(document);
  vi.spyOn(document, "dispatchEvent").mockImplementation((e) => {
    const updating = [...document.querySelectorAll<HTMLElement>(".cm-editor")].some(
      (dom) =>
        (EditorView.findFromDOM(dom) as unknown as { updateState: number }).updateState !== 0,
    );
    return e.type === "selectionchange" && updating ? true : dispatch(e);
  });
});

afterEach(async () => {
  await endTest();
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

const settle = () => act(async () => new Promise((r) => setTimeout(r, 20)));

async function mount() {
  const element = document.createElement("div");
  element.style.width = "1280px";
  element.style.height = "800px";
  document.body.append(element);
  const store = createThemeStore({
    system: { current: () => "light", subscribe: () => () => {} },
    storage: { read: () => undefined, write: () => {} },
    theme: parseTheme(defaultTheme),
    apply: () => {},
  });
  root = createRoot(element);
  await act(async () =>
    root!.render(
      <ThemeStoreProvider store={store}>
        <App kind="project" native={native} />
      </ThemeStoreProvider>,
    ),
  );
  await settle();
}

async function open(file: string, line?: number, col?: number) {
  await act(async () => projectOpenInbox.push({ file, line, col }));
  await settle();
}

async function keydown(init: KeyboardEventInit, target?: Element | null) {
  await act(async () => {
    (target ?? document.activeElement ?? document.body).dispatchEvent(
      new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }),
    );
  });
  await settle();
}

const view = () =>
  EditorView.findFromDOM(document.querySelector<HTMLElement>(".dv-active-group .cm-editor")!)!;

/** 프로젝트 범위 이벤트를 구독한 연결에 보낸다. */
async function emit(topic: string, payload: unknown) {
  await act(async () => {
    for (const sub of host.subscribers)
      if (sub.topics.includes(topic))
        sub.send({
          method: "events.event",
          params: { subscriptionId: sub.id, projectId: "p1", topic, payload },
        });
  });
  await settle();
}

type Diagnostic = LspDiagnostic;

function diag(
  line: number,
  character: number,
  severity: Diagnostic["severity"] = "error",
): Diagnostic {
  return {
    server: "rust-analyzer",
    origin: "push",
    version: null,
    range: { start: { line, character }, end: { line, character: character + 1 } },
    severity,
    message: `${severity} ${line}:${character}`,
    code: null,
    source: null,
    codeHref: null,
    tags: [],
    related: [],
    data: null,
  };
}

/** Host 처럼 경로 차례 · 쪽 단위(`limit` 개)로 답한다. 커서는 앞 쪽까지 준 진단 수. */
function serveList(files: Map<string, Diagnostic[]>) {
  host.diagnostics["lsp.diagnostics.list"] = ((p: {
    path?: string | null;
    cursor?: string | null;
    limit: number;
  }) => {
    const paths = [...files.keys()].filter((f) => !p.path || f === p.path).sort();
    const flat = paths.flatMap((path) => files.get(path)!.map((d) => [path, d] as const));
    const from = p.cursor ? Number(p.cursor) : 0;
    const page = flat.slice(from, from + p.limit);
    const grouped = new Map<string, Diagnostic[]>();
    for (const [path, d] of page) grouped.set(path, [...(grouped.get(path) ?? []), d]);
    return {
      files: [...grouped].map(([path, diagnostics]) => ({ path, diagnostics, truncated: false })),
      nextCursor: from + p.limit < flat.length ? String(from + p.limit) : null,
      truncated: false,
    } satisfies LspDiagnosticsList;
  }) as never;
}

function serveCounts(counts: () => LspDiagnosticsCounts) {
  host.diagnostics["lsp.diagnostics.counts"] = (() => counts()) as never;
}

const statusItem = () => document.querySelector<HTMLElement>('[data-testid="status-problems"]');
const problemRows = () => [...document.querySelectorAll<HTMLElement>("[data-problem-row]")];

async function openProblems() {
  await vi.waitFor(() => expect(statusItem()).not.toBeNull());
  await act(async () => statusItem()!.click());
  await settle();
  await vi.waitFor(() =>
    expect(document.querySelector('[data-tool-window="problems"]')).not.toBeNull(),
  );
}

function treeHeight(height: number) {
  for (const prop of ["offsetHeight", "clientHeight"] as const)
    vi.spyOn(HTMLElement.prototype, prop, "get").mockImplementation(function (this: HTMLElement) {
      return this.getAttribute("role") === "tree" ? height : 0;
    });
}

const ZERO: LspDiagnosticsCounts = {
  error: 0,
  warning: 0,
  information: 0,
  hint: 0,
  truncated: false,
};

it("draws only the visible rows of 10,000 diagnostics in 2,000 files, reading them page by page (④)", async () => {
  treeHeight(400);
  const files = new Map<string, Diagnostic[]>();
  for (let i = 0; i < 2_000; i++)
    files.set(
      `/work/demo/src/f${String(i).padStart(4, "0")}.rs`,
      [0, 1, 2, 3, 4].map((line) => diag(line, 0, line % 2 ? "warning" : "error")),
    );
  serveList(files);
  serveCounts(() => ({ ...ZERO, error: 6_000, warning: 4_000 }));
  await mount();
  await openProblems();
  await vi.waitFor(() => expect(problemRows().length).toBeGreaterThan(0));
  // 파일 줄 2천 + 진단 줄 1만 = 1만 2천 줄 중 보이는 만큼(400 px / 22 px + 앞뒤 여분)만 그린다.
  expect(problemRows().length).toBeLessThanOrEqual(50);
  const tree = document.querySelector<HTMLElement>('[data-tool-window="problems"] [role="tree"]')!;
  expect((tree.firstElementChild as HTMLElement).style.height).toBe(`${12_000 * 22}px`);
  const pages = host.diagnosticCalls.filter(
    ([m, p]) => m === "lsp.diagnostics.list" && !(p as { path?: string }).path,
  );
  expect(pages.map(([, p]) => (p as { cursor: string | null }).cursor)).toEqual([null, "5000"]);
  // 심각도 거르기의 수가 읽은 것 전부다.
  expect(document.querySelector('[data-problems-filter="error"]')?.getAttribute("aria-label")).toBe(
    "오류 6000",
  );
});

it("shows the counts lsp.diagnostics.counts answers in the status bar and asks again after events, once per burst (⑥)", async () => {
  let counts: LspDiagnosticsCounts = { ...ZERO, error: 3, warning: 7 };
  serveCounts(() => counts);
  serveList(new Map());
  await mount();
  await vi.waitFor(() => expect(statusItem()).not.toBeNull());
  const shown = () => [
    statusItem()!.querySelector('[data-count="warning"]')!.textContent,
    statusItem()!.querySelector('[data-count="error"]')!.textContent,
  ];
  expect(shown()).toEqual(["7", "3"]);
  expect(statusItem()!.getAttribute("aria-label")).toBe(
    "오류 3개 · 경고 7개 — 문제 툴 윈도우 열기",
  );
  counts = { ...ZERO, error: 4, warning: 1, truncated: true };
  const before = host.diagnosticCalls.filter(([m]) => m === "lsp.diagnostics.counts").length;
  await act(async () => {
    for (let i = 0; i < 10; i++)
      for (const sub of host.subscribers)
        if (sub.topics.includes("lsp.diagnostics"))
          sub.send({
            method: "events.event",
            params: {
              subscriptionId: sub.id,
              projectId: "p1",
              topic: "lsp.diagnostics",
              payload: {
                projectId: "p1",
                worktree: null,
                path: MAIN,
                diagnostics: [],
                truncated: false,
              },
            },
          });
  });
  await settle();
  await vi.waitFor(() => expect(shown()).toEqual(["1+", "4+"]));
  const after = host.diagnosticCalls.filter(([m]) => m === "lsp.diagnostics.counts").length;
  // 열 개가 몰려도 묻는 중이면 끝난 뒤 한 번만 더 — 열 번 묻지 않는다.
  expect(after - before).toBeGreaterThanOrEqual(1);
  expect(after - before).toBeLessThanOrEqual(2);
});

it("shows the partial-view notice only while some file is still cut by a limit", async () => {
  treeHeight(400);
  serveList(new Map());
  serveCounts(() => ZERO);
  await mount();
  await openProblems();
  const notice = () => document.querySelector('[data-testid="problems-truncated"]');
  const send = (truncated: boolean) =>
    emit("lsp.diagnostics", {
      projectId: "p1",
      worktree: null,
      path: MAIN,
      diagnostics: [diag(0, 3, "error")],
      truncated,
    });
  await send(true);
  await vi.waitFor(() => expect(notice()).not.toBeNull());
  // 범람이 지나가 그 파일이 더는 잘리지 않으면 안내도 꺼진다(M4-06 리뷰 L4 — 한 번 켜진 채 남지 않는다).
  await send(false);
  await vi.waitFor(() => expect(notice()).toBeNull());
  expect(problemRows().map((r) => r.dataset.problemRow)).toEqual([MAIN, `${MAIN}\n0`]);
});

it("opens a diagnostic's place with a double click or Enter, shows the current file only on its tab and filters severities", async () => {
  treeHeight(400);
  const other = "/work/demo/src/other.rs";
  host.files.set(other, "a\nb\n");
  serveList(
    new Map([
      [MAIN, [diag(0, 3, "error")]],
      [other, [diag(1, 0, "warning"), diag(0, 0, "error")]],
    ]),
  );
  serveCounts(() => ({ ...ZERO, error: 2, warning: 1 }));
  await mount();
  await openProblems();
  await vi.waitFor(() => expect(problemRows().length).toBe(5));
  const row = problemRows().find((r) => r.dataset.problemRow === `${MAIN}\n0`)!;
  await act(async () => row.dispatchEvent(new MouseEvent("dblclick", { bubbles: true })));
  await settle();
  await vi.waitFor(() => expect(view().state.doc.toString()).toBe("fn main() {}\n"));
  expect(view().state.selection.main.head).toBe(3);
  // 「현재 파일」 탭은 활성 에디터 탭의 파일만.
  await act(async () =>
    document.querySelector<HTMLElement>('[data-problems-scope="file"]')!.click(),
  );
  await vi.waitFor(() =>
    expect(problemRows().map((r) => r.dataset.problemRow)).toEqual([MAIN, `${MAIN}\n0`]),
  );
  await act(async () =>
    document.querySelector<HTMLElement>('[data-problems-scope="project"]')!.click(),
  );
  // 오류를 거르면 경고만 남는다.
  await act(async () =>
    document.querySelector<HTMLElement>('[data-problems-filter="error"]')!.click(),
  );
  await vi.waitFor(() =>
    expect(problemRows().map((r) => r.dataset.severity ?? "file")).toEqual(["file", "warning"]),
  );
  // ↩ 로도 연다(키보드).
  const tree = document.querySelector<HTMLElement>('[data-tool-window="problems"] [role="tree"]')!;
  await act(async () => problemRows()[1]!.click());
  await keydown({ key: "Enter", code: "Enter" }, tree);
  await vi.waitFor(() => expect(view().state.doc.toString()).toBe("a\nb\n"));
  expect(view().state.selection.main.head).toBe(2);
});

it("moves to the next error with F2 in the editor and wraps around (⑤)", async () => {
  serveList(new Map());
  serveCounts(() => ZERO);
  await mount();
  await open(MAIN);
  await vi.waitFor(() => expect(host.lsp!.called("open")).toHaveLength(1));
  await emit("lsp.diagnostics", {
    projectId: "p1",
    worktree: null,
    path: MAIN,
    diagnostics: [diag(0, 3, "error"), diag(0, 8, "error")],
    truncated: false,
  });
  await vi.waitFor(() =>
    expect(view().contentDOM.querySelectorAll(".pw-diagnostic-error")).toHaveLength(2),
  );
  await act(async () => view().focus());
  const at = () => view().state.selection.main.head;
  await keydown({ key: "F2", code: "F2" }, view().contentDOM);
  expect(at()).toBe(3);
  await keydown({ key: "F2", code: "F2" }, view().contentDOM);
  expect(at()).toBe(8);
  await keydown({ key: "F2", code: "F2" }, view().contentDOM);
  expect(at()).toBe(3);
  await keydown({ key: "F2", code: "F2", shiftKey: true }, view().contentDOM);
  expect(at()).toBe(8);
  expect(view().dom.querySelector(".pw-lsp-popup .pw-diagnostic-message")?.textContent).toBe(
    "error 0:8",
  );
});
