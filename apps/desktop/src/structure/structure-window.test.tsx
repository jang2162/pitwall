// @vitest-environment happy-dom
// 구조 툴 윈도우 · 파일 구조 팝업(15-m4-plan M4-10)을 프로젝트 창(App kind="project")과 가짜 Host 로 본다 — 가짜 서버의 계층 심볼이
// 트리로 오고(출처 「LSP」), 서버가 멈추면(능력 칸에서 `document_symbol` 이 빠짐) 심볼 인덱스로 바뀌고 출처 표지가 바뀐다, 미신뢰
// 프로젝트는 조용히 인덱스, 커서 따라가기 · 정렬 · 고르면 위치로 가기(기록에 넣는다), ⌘F12 거르기 · 이동 · Esc, 편집 뒤 300 ms,
// 1만 심볼의 첫 화면(가상화). 창 틀(가짜 Host · 마운트)은 src/lsp/lsp-window.test.tsx 와 같다.
//
// CodeMirror 는 불러올 때 macOS 인지 정한다(navigator.platform) — 그래서 불러오기 전에 Mac 으로 둔다.

import { EditorView } from "@codemirror/view";
import type {
  IndexFileSymbols,
  LspDocumentSymbols,
  LspOutlineSymbol,
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
import { fakeTreeLayout } from "../file-tree/testing";
import { fakeSyntaxServer } from "../editor/syntax-testing";
import { fakeLspServer } from "../lsp/lsp-testing";
import { createDocumentStore } from "../editor/documents";
import { createEditorWorkspace } from "../editor/editor-workspace";
import { bundledTranslator } from "../i18n/testing";
import { fileStructureActions } from "./file-structure-popup";
import { structureStrings } from "./strings";

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
  /** 기능 행의 Host RPC(`lsp.symbols.document` · `index.file.symbols`) 처리기. */
  handlers: new Map<string, (params: never) => Promise<unknown>>(),
  /** 기능 행의 Host RPC 호출 기록(메서드, 매개변수). */
  calls: [] as { method: string; params: unknown }[],
  trusted: true,
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
        const handler = host.handlers.get(m.method);
        if (handler) {
          host.calls.push({ method: m.method, params: m.params });
          return void handler(m.params).then(reply, (e: { code?: number; message: string }) =>
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
              state: host.trusted ? "trusted" : "untrusted",
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

const MAIN = "/work/demo/src/main.rs";
/** 줄 0 `struct Point {`, 줄 1 `    x: u8,`, 줄 2 `}`, 줄 3 빈 줄, 줄 4 `fn main() {}`(오프셋 25 ~ 37) … 줄 25 까지 빈 줄, 줄 26 `fn tail() {}`. */
const MAIN_TEXT = `struct Point {\n    x: u8,\n}\n\nfn main() {}\n${"\n".repeat(21)}fn tail() {}\n`;

let root: Root | undefined;

function useUserAgent(ua: string) {
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(ua);
}

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  useUserAgent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)");
  vi.spyOn(console, "info").mockImplementation(() => {});
  window.history.replaceState(null, "", "/?window=project&project=p1");
  host.files = new Map([[MAIN, MAIN_TEXT]]);
  host.settings = {};
  host.subscribers = [];
  host.syntax = undefined;
  host.lsp = fakeLspServer();
  host.sizeClass = new Map();
  host.opened = [];
  host.handlers = new Map();
  host.calls = [];
  host.trusted = true;
  fakeTreeLayout(600);
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

/** 에디터 탭은 하나다 — 툴 윈도우에 포커스가 가도 찾게 활성 그룹으로 가리지 않는다. */
const view = () => EditorView.findFromDOM(document.querySelector<HTMLElement>(".cm-editor")!)!;

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
// ---- 구조 툴 윈도우 · 파일 구조 팝업 ----------------------------------------------------------------------------------------

/** MAIN_TEXT 의 (줄, 글자) → UTF-16 오프셋. */
function off(line: number, col: number): number {
  const lines = MAIN_TEXT.split("\n");
  return lines.slice(0, line).reduce((n, l) => n + l.length + 1, 0) + col;
}

const DOCUMENT_SYMBOL = {
  document_symbol: { server: "rust-analyzer", options: true, optionsTruncated: false },
};

/** 서버의 답(Host 가 바꾼 꼴 — crates/pw-lsp/src/symbols.rs). */
function lspSymbols(extra: Partial<LspOutlineSymbol>[] = []): LspDocumentSymbols {
  const s = (
    name: string,
    kind: number,
    parent: number | null,
    [sl, sc, el, ec]: number[],
    [nl, nc, nlen]: number[],
    detail: string | null = null,
  ): LspOutlineSymbol => ({
    name,
    detail,
    kind,
    deprecated: false,
    parent,
    from: off(sl!, sc!),
    to: off(el!, ec!),
    selectionFrom: off(nl!, nc!),
    selectionTo: off(nl!, nc!) + nlen!,
  });
  return {
    server: "rust-analyzer",
    staleVersion: false,
    truncated: false,
    symbols: [
      s("Point", 23, null, [0, 0, 2, 1], [0, 7, 5]),
      s("x", 8, 0, [1, 4, 1, 9], [1, 4, 1], "u8"),
      s("main", 12, null, [4, 0, 4, 12], [4, 3, 4], "fn()"),
      s("tail", 12, null, [26, 0, 26, 12], [26, 3, 4], "fn()"),
      ...extra.map((e) => ({ ...s("extra", 12, null, [4, 0, 4, 1], [4, 0, 1]), ...e })),
    ],
  };
}

/** 심볼 인덱스의 답(줄 · 글자). 세부가 없다 — 서버 답과 가른다. */
function indexSymbols(): IndexFileSymbols {
  const r = (sl: number, sc: number, el: number, ec: number) => ({
    start: { line: sl, character: sc },
    end: { line: el, character: ec },
  });
  return {
    language: "Rust",
    symbols: [
      {
        name: "Point",
        kind: "struct",
        parent: null,
        range: r(0, 0, 2, 1),
        selectionRange: r(0, 7, 0, 12),
      },
      { name: "x", kind: "field", parent: 0, range: r(1, 4, 1, 9), selectionRange: r(1, 4, 1, 5) },
      {
        name: "main",
        kind: "function",
        parent: null,
        range: r(4, 0, 4, 12),
        selectionRange: r(4, 3, 4, 7),
      },
      {
        name: "tail",
        kind: "function",
        parent: null,
        range: r(26, 0, 26, 12),
        selectionRange: r(26, 3, 26, 7),
      },
    ],
  };
}

function serve(lsp: () => LspDocumentSymbols = () => lspSymbols()) {
  host.handlers.set("lsp.symbols.document", async () => lsp());
  host.handlers.set("index.file.symbols", async () => indexSymbols());
}

const called = (method: string) =>
  host.calls.filter((c) => c.method === method).map((c) => c.params);

const structure = () => document.querySelector<HTMLElement>('[data-tool-window="structure"]')!;
/** 보이는 줄: [이름, 깊이(aria-level), 세부]. */
const rows = () =>
  [...structure().querySelectorAll<HTMLElement>("[data-structure-row]")].map((r) => [
    r.querySelector("[data-structure-name]")!.textContent,
    Number(r.getAttribute("aria-level")),
  ]);
const selectedRow = () =>
  structure().querySelector('[aria-selected="true"] [data-structure-name]')?.textContent;
const source = () => {
  const label = structure().querySelector<HTMLElement>("[data-structure-source]");
  return label && [label.dataset.structureSource, label.textContent];
};

async function openStructure() {
  await act(async () => menu.choose!("toolWindow.structure"));
  await settle();
}

/** 서버가 뜸 · 멈춤: 능력 칸을 바꾸고 `lsp.status` 로 알린다(웹뷰가 칸을 다시 묻는다). */
async function serverReady(ready: boolean) {
  host.lsp!.setFeatures(ready ? DOCUMENT_SYMBOL : {});
  await emit("lsp.status", { projectId: "p1", instances: [] });
  await settle();
}

const cursor = () => view().state.selection.main.head;

it("shows the server's symbols as a tree, then the index when the server stops, and the server again when it is back", async () => {
  serve();
  await mount();
  await open(MAIN, 1, 1);
  await serverReady(true);
  await openStructure();
  expect(rows()).toEqual([
    ["Point", 1],
    ["x", 2],
    ["main", 1],
    ["tail", 1],
  ]);
  expect(source()).toEqual(["lsp", "LSP · rust-analyzer"]);
  expect(called("lsp.symbols.document")).toEqual([{ docId: "d1", version: 0 }]);
  expect(structure().textContent).toContain("u8");
  expect(called("index.file.symbols")).toEqual([]);

  // 서버가 멈추면 능력 칸에서 빠진다 — 인덱스(저장하지 않은 내용을 싣는다)로 바뀌고 출처 표지가 바뀐다.
  await serverReady(false);
  expect(source()).toEqual(["index", "인덱스"]);
  expect(rows()).toEqual([
    ["Point", 1],
    ["x", 2],
    ["main", 1],
    ["tail", 1],
  ]);
  expect(structure().textContent).not.toContain("u8");
  expect(called("index.file.symbols")).toEqual([{ projectId: "p1", path: MAIN, text: MAIN_TEXT }]);

  // 다시 뜨면 서버로.
  await serverReady(true);
  expect(source()).toEqual(["lsp", "LSP · rust-analyzer"]);
  expect(called("lsp.symbols.document")).toHaveLength(2);
});

it("falls back to the index quietly in an untrusted project, without asking the server", async () => {
  serve();
  host.trusted = false;
  await mount();
  await open(MAIN, 1, 1);
  await openStructure();
  expect(source()).toEqual(["index", "인덱스"]);
  expect(rows().map((r) => r[0])).toEqual(["Point", "x", "main", "tail"]);
  expect(called("lsp.symbols.document")).toEqual([]);
  // 조용히: 실패 문구 · 알림이 없다.
  expect(document.body.textContent).not.toContain("심볼을 읽지 못했다");
});

it.each([
  // 서버를 내려놓는 중에 칸이 아직 남아 있으면 Host 가 「맡을 서버 없음」 으로 답한다 — 요청 도우미가 삼키는 코드라 따로 받는다.
  { code: 5001, message: "no running language server offers document_symbol" },
  { code: 5003, message: "the server is gone" },
])("uses the index when the server request fails with $code", async ({ code, message }) => {
  serve();
  host.handlers.set("lsp.symbols.document", () => Promise.reject({ code, message }));
  await mount();
  await open(MAIN, 1, 1);
  await serverReady(true);
  await openStructure();
  expect(called("lsp.symbols.document")).toHaveLength(1);
  expect(source()).toEqual(["index", "인덱스"]);
});

it("drops a stale answer and keeps the tree it has", async () => {
  let stale = false;
  serve(() => ({ ...lspSymbols(stale ? [{ name: "late" }] : []), staleVersion: stale }));
  await mount();
  await open(MAIN, 1, 1);
  await serverReady(true);
  await openStructure();
  expect(rows()).toHaveLength(4);
  stale = true;
  // 상태 이벤트로 다시 읽는다 — 낡았다고 표시된 답은 그리지 않고, 인덱스로 가지도 않는다.
  await serverReady(true);
  expect(called("lsp.symbols.document")).toHaveLength(2);
  expect(rows().map((r) => r[0])).not.toContain("late");
  expect(source()).toEqual(["lsp", "LSP · rust-analyzer"]);
  expect(called("index.file.symbols")).toEqual([]);
});

it("reads again 300 ms after the edits stop", async () => {
  serve();
  await mount();
  await open(MAIN, 1, 1);
  await serverReady(true);
  await openStructure();
  expect(called("lsp.symbols.document")).toHaveLength(1);
  await act(async () => view().dispatch({ changes: { from: 0, insert: "// a\n" } }));
  await act(async () => new Promise((r) => setTimeout(r, 150)));
  await act(async () => view().dispatch({ changes: { from: 0, insert: "// b\n" } }));
  await act(async () => new Promise((r) => setTimeout(r, 150)));
  // 마지막 편집 뒤 300 ms 가 아직 안 됐다.
  expect(called("lsp.symbols.document")).toHaveLength(1);
  await act(async () => new Promise((r) => setTimeout(r, 300)));
  await settle();
  expect(called("lsp.symbols.document")).toEqual([
    { docId: "d1", version: 0 },
    { docId: "d1", version: 2 },
  ]);
});

it("follows the cursor to the innermost symbol, and stops when the toggle is off", async () => {
  serve();
  await mount();
  await open(MAIN, 1, 1);
  await serverReady(true);
  await openStructure();
  await act(async () => view().dispatch({ selection: { anchor: off(1, 6) } }));
  await settle();
  expect(selectedRow()).toBe("x");
  await act(async () => view().dispatch({ selection: { anchor: off(0, 2) } }));
  await settle();
  expect(selectedRow()).toBe("Point");
  const toggle = structure().querySelector<HTMLButtonElement>(
    'button[aria-label="커서 따라가기"]',
  )!;
  expect(toggle.getAttribute("aria-pressed")).toBe("true");
  await act(async () => toggle.click());
  expect(toggle.getAttribute("aria-pressed")).toBe("false");
  await act(async () => view().dispatch({ selection: { anchor: off(4, 5) } }));
  await settle();
  expect(selectedRow()).toBe("Point");
  await act(async () => toggle.click());
  await settle();
  expect(selectedRow()).toBe("main");
});

it("sorts by name with the toggle", async () => {
  serve();
  await mount();
  await open(MAIN, 1, 1);
  await serverReady(true);
  await openStructure();
  const sort = structure().querySelector<HTMLButtonElement>(
    'button[aria-label="이름 순으로 정렬"]',
  )!;
  await act(async () => sort.click());
  expect(rows().map((r) => r[0])).toEqual(["main", "Point", "x", "tail"]);
  await act(async () => sort.click());
  expect(rows().map((r) => r[0])).toEqual(["Point", "x", "main", "tail"]);
});

it("goes to a picked symbol on click and Enter, and records where it left for ⌘[", async () => {
  serve();
  await mount();
  await open(MAIN, 1, 1);
  await serverReady(true);
  await openStructure();
  const row = (name: string) =>
    [...structure().querySelectorAll<HTMLElement>("[data-structure-row]")].find(
      (r) => r.querySelector("[data-structure-name]")!.textContent === name,
    )!;
  await act(async () => row("tail").click());
  await settle();
  expect(cursor()).toBe(off(26, 3));
  // 고르기는 위치로 가기다 — 떠난 자리(1:1)가 뒤로 가기 기록에 든다.
  await keydown({ code: "BracketLeft", key: "[", metaKey: true }, view().contentDOM);
  expect(cursor()).toBe(0);
  // 키보드: 트리에서 ↓ 로 고르고 ↩.
  const tree = structure().querySelector<HTMLElement>('[role="tree"]')!;
  await act(async () => tree.focus());
  await keydown({ key: "Home" }, tree);
  await keydown({ key: "ArrowDown" }, tree);
  await keydown({ key: "ArrowDown" }, tree);
  expect(selectedRow()).toBe("main");
  await keydown({ key: "Enter" }, tree);
  expect(cursor()).toBe(off(4, 3));
});

it("draws server names and details as text, not HTML", async () => {
  serve(() => lspSymbols([{ name: '<img src=x onerror="alert(1)">', detail: "<b>bold</b>" }]));
  await mount();
  await open(MAIN, 1, 1);
  await serverReady(true);
  await openStructure();
  expect(structure().textContent).toContain('<img src=x onerror="alert(1)">');
  expect(structure().textContent).toContain("<b>bold</b>");
  expect(structure().querySelector("img, b")).toBeNull();
});

it("filters the file structure popup fuzzily on ⌘F12, goes on Enter and closes on Esc without reaching the window", async () => {
  serve();
  await mount();
  await open(MAIN, 1, 1);
  await serverReady(true);
  const popup = () => document.querySelector<HTMLElement>("[data-file-structure]");
  const items = () =>
    [...popup()!.querySelectorAll<HTMLElement>("[data-structure-item]")].map((i) => [
      i.querySelector("[data-structure-name]")!.textContent,
      i.getAttribute("data-disabled") === "true" ? "dim" : "match",
    ]);
  const selectedItem = () =>
    popup()!.querySelector('[data-selected="true"] [data-structure-name]')?.textContent;

  await keydown({ code: "F12", key: "F12", metaKey: true }, view().contentDOM);
  expect(popup()).not.toBeNull();
  expect(items()).toEqual([
    ["Point", "match"],
    ["x", "match"],
    ["main", "match"],
    ["tail", "match"],
  ]);
  // 커서(1:1)를 담은 심볼이 골라져 있고, 출처를 적는다.
  expect(selectedItem()).toBe("Point");
  expect(popup()!.querySelector("[data-structure-source]")!.textContent).toBe(
    "LSP · rust-analyzer",
  );

  // 퍼지: 「tl」 은 tail 에 맞는다.
  const input = popup()!.querySelector<HTMLInputElement>("input")!;
  await act(async () => {
    const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
    set.call(input, "tl");
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await settle();
  expect(items()).toEqual([["tail", "match"]]);
  expect(selectedItem()).toBe("tail");
  await keydown({ key: "Enter" }, input);
  expect(popup()).toBeNull();
  expect(cursor()).toBe(off(26, 3));

  // 조상은 흐리게 남는다.
  await keydown({ code: "F12", key: "F12", metaKey: true }, view().contentDOM);
  const input2 = popup()!.querySelector<HTMLInputElement>("input")!;
  await act(async () => {
    const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
    set.call(input2, "x");
    input2.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await settle();
  expect(items()).toEqual([
    ["Point", "dim"],
    ["x", "match"],
  ]);
  // Esc 는 팝업이 처리했다고 막는다 — 창의 Esc(멀티 커서 접기)가 그 눌림에서 돌지 않는다.
  const esc = new KeyboardEvent("keydown", {
    key: "Escape",
    code: "Escape",
    bubbles: true,
    cancelable: true,
  });
  await act(async () => void input2.dispatchEvent(esc));
  await settle();
  expect(esc.defaultPrevented).toBe(true);
  expect(popup()).toBeNull();
});

it("shows the first screen of a 10,000-symbol file drawing only the visible rows (time to the first row is logged)", async () => {
  const many: LspDocumentSymbols = {
    server: "rust-analyzer",
    staleVersion: false,
    truncated: false,
    symbols: Array.from({ length: 10_000 }, (_, i) => ({
      name: `symbol_${i}`,
      detail: "fn()",
      kind: i % 100 === 0 ? 2 : 12,
      deprecated: false,
      // 100 개마다 모듈 하나, 그 아래 함수 99개.
      parent: i % 100 === 0 ? null : i - (i % 100),
      from: 0,
      to: 1,
      selectionFrom: 0,
      selectionTo: 0,
    })),
  };
  let answeredAt = 0;
  host.handlers.set("lsp.symbols.document", async () => {
    answeredAt = performance.now();
    return many;
  });
  await mount();
  await open(MAIN, 1, 1);
  await serverReady(true);
  let drawnAt = 0;
  const watch = new MutationObserver(() => {
    if (!drawnAt && document.querySelector("[data-structure-row]")) drawnAt = performance.now();
  });
  watch.observe(document.body, { childList: true, subtree: true });
  await openStructure();
  watch.disconnect();
  expect(answeredAt).toBeGreaterThan(0);
  expect(drawnAt).toBeGreaterThan(0);
  const ms = drawnAt - answeredAt;
  // 벽시계(답이 웹뷰에 닿음 → 첫 줄이 DOM 에)는 콘솔에만 남긴다 — 기기 부하에 흔들리는 최댓값은 단언하지 않는다(거터 성능 시험과
  // 같은 규칙, M2-17 리뷰 F2). 200 ms 판정은 부하가 낮을 때 잰 값을 보고에 적고, 프레임까지는 M4-19 가 release 번들에서 잰다(결정
  // M4-D19). 여기서는 그 시간을 심볼 수와 무관하게 하는 것 — 가상화로 보이는 줄만 그린다는 것 — 을 단언한다.
  console.log(`[perf] structure first screen of 10,000 symbols: ${ms.toFixed(1)} ms`);
  // 가상화: 600 px 높이에 줄 22 px — 보이는 줄과 위아래 여유만 그린다.
  const drawn = structure().querySelectorAll("[data-structure-row]").length;
  expect(drawn).toBeGreaterThan(10);
  expect(drawn).toBeLessThan(80);
  expect(rows()[0]).toEqual(["symbol_0", 1]);
});

it("keeps the file structure action off in a window without a project", async () => {
  const workspace = createEditorWorkspace({
    documents: createDocumentStore({
      stat: () => Promise.reject(new Error("none")),
      read: () => Promise.reject(new Error("none")),
    }),
    stat: () => Promise.reject(new Error("none")),
    onOpenFailed() {},
    onSaveFailed() {},
    onSaveBlocked() {},
    confirmUnsaved: async () => "cancel",
  });
  const [action] = fileStructureActions({
    workspace,
    strings: structureStrings(bundledTranslator("ko")),
    open() {},
  });
  expect(action!.id).toBe("navigate.fileStructure");
  expect(action!.enabled!()).toBe(false);
});

it("reads nothing while the structure tool window is hidden", async () => {
  serve();
  await mount();
  await open(MAIN, 1, 1);
  await serverReady(true);
  // 레이아웃 복원으로 숨은 툴 윈도우도 마운트돼 있다 — 보이기 전에는 묻지 않는다.
  expect(called("lsp.symbols.document")).toEqual([]);
  expect(called("index.file.symbols")).toEqual([]);
  await openStructure();
  expect(called("lsp.symbols.document")).toHaveLength(1);
  // 숨기면(모든 툴 윈도우 숨기기) 편집 뒤에도 묻지 않는다.
  await act(async () => menu.choose!("toolWindow.hideAll"));
  await settle();
  await act(async () => view().dispatch({ changes: { from: 0, insert: "// a\n" } }));
  await act(async () => new Promise((r) => setTimeout(r, 400)));
  expect(called("lsp.symbols.document")).toHaveLength(1);
});

it("draws only the first rows of a large file in the popup and says how many more there are", async () => {
  host.handlers.set("lsp.symbols.document", async () => ({
    server: "rust-analyzer",
    staleVersion: false,
    truncated: false,
    symbols: Array.from({ length: 1000 }, (_, i) => ({
      name: `f${i}`,
      detail: null,
      kind: 12,
      deprecated: false,
      parent: null,
      from: 0,
      to: 1,
      selectionFrom: 0,
      selectionTo: 0,
    })),
  }));
  await mount();
  await open(MAIN, 1, 1);
  await serverReady(true);
  await keydown({ code: "F12", key: "F12", metaKey: true }, view().contentDOM);
  const popup = document.querySelector<HTMLElement>("[data-file-structure]")!;
  expect(popup.querySelectorAll("[data-structure-item]")).toHaveLength(300);
  expect(popup.querySelector("[data-structure-more]")!.textContent).toBe(
    "700줄 더 — 더 입력해 좁힌다",
  );
});
