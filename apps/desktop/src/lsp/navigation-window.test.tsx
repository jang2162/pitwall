// @vitest-environment happy-dom
// 내비게이션(15-m4-plan M4-08)을 프로젝트 창(App kind="project")과 가짜 Host 로 본다 — 선언으로 이동(하나 · 여럿 · 선언 위),
// 구현, 사용처 팝업과 「검색 결과」 고정, 루트 밖 문서(외부 소스 탭 — 읽기 전용, 그 안의 요청은 토큰으로), 서버의 showDocument,
// 호버 · 빠른 문서(서버 글은 HTML 로 해석하지 않는다), 맡을 서버가 없으면 조용함, 뒤로 가기(M4-09). 창 틀(가짜 Host · 마운트)은
// lsp-window.test.tsx(M4-07)와 같고, 기능 행의 Host RPC(`lsp.hover` 등)는 가짜 서버 관리자의 처리기(`handle`)로 보낸다.
//
// CodeMirror 는 불러올 때 macOS 인지 정한다(navigator.platform) — 그래서 불러오기 전에 Mac 으로 둔다.

import { EditorSelection } from "@codemirror/state";
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
import { editorContext } from "../editor/extensions/context";
import type { fakeSyntaxServer } from "../editor/syntax-testing";
import { fakeLspServer } from "./lsp-testing";
import { PREVIEW_READ_BYTES } from "./navigation";
import { currentPopup } from "./popup";

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
  /** 받은 RPC(메서드 · 매개변수). */
  calls: [] as { method: string; params: unknown }[],
}));

/** 이 창의 label 과 창을 앞으로 가져온 횟수(./window-focus — Tauri 밖이라 시험이 정한다). */
const focus = vi.hoisted(() => ({ label: null as string | null, forward: 0 }));

vi.mock("./window-focus", () => ({
  windowLabel: () => focus.label,
  bringForward: async () => void focus.forward++,
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
        host.calls.push({ method: m.method, params: m.params });
        if (m.method.startsWith("lsp.") && host.lsp) {
          const { source, services } = host.lsp;
          const run =
            (
              {
                "lsp.document.open": source.open,
                "lsp.document.change": source.change,
                "lsp.document.save": source.save,
                "lsp.document.close": source.close,
                "lsp.document.capabilities": source.capabilities,
              } as Record<string, (p: unknown) => Promise<unknown>>
            )[m.method] ??
            ((p: unknown) =>
              (services.call as (k: string, p: unknown) => Promise<unknown>)(m.method, p));
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
          case "fs.read": {
            const whole = host.files.get(path)!;
            const { offset = 0, length } = m.params as { offset?: number; length?: number };
            const text = length === undefined ? whole : whole.slice(offset, offset + length);
            return reply({
              path,
              size: whole.length,
              mtimeMs: 1,
              offset,
              eof: offset + text.length >= whole.length,
              version: length === undefined ? "1" : null,
              content: { kind: "text", text },
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
          }
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
const LIB = "/work/demo/src/lib.rs";
/** 루트 밖(표준 라이브러리 소스) — 토큰으로만 읽힌다. */
const VEC = "/rust/library/alloc/src/vec/mod.rs";

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
    [MAIN, "fn main() {\n    helper();\n    other();\n}\n"],
    [LIB, "pub fn helper() {}\npub fn other() {}\npub fn third() {}\n"],
  ]);
  host.settings = {};
  host.subscribers = [];
  host.syntax = undefined;
  host.lsp = fakeLspServer();
  focus.label = null;
  focus.forward = 0;
  host.sizeClass = new Map();
  host.opened = [];
  host.calls = [];
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

/** 선택 범위들(anchor, head). */
const ranges = () => view().state.selection.ranges.map((r) => [r.anchor, r.head]);

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

const popupDom = () => view().dom.querySelector<HTMLElement>(".pw-lsp-popup");

async function compose(event: "compositionstart" | "compositionend") {
  await act(async () => {
    view().contentDOM.dispatchEvent(new CompositionEvent(event, { bubbles: true, data: "" }));
  });
}

/** 조합 트랜잭션 하나(입력기가 조합 중의 글자를 바꾼 것). */
async function composeText(from: number, to: number, insert: string) {
  await act(async () =>
    view().dispatch({
      changes: { from, to, insert },
      selection: { anchor: from + insert.length },
      userEvent: "input.type.compose",
    }),
  );
}

const SERVED = { server: "rust-analyzer", options: true, optionsTruncated: false };

/** 서버가 hover · 정의 · 구현 · 사용처를 맡는다(문서를 열기 전에 둔다 — 열면 능력 칸을 묻는다). */
function serve() {
  host.lsp!.setFeatures({
    hover: SERVED,
    definition: SERVED,
    implementation: SERVED,
    references: SERVED,
  });
}

function loc(
  path: string,
  line: number,
  column: number,
  endColumn: number,
  preview: string,
  token?: string,
) {
  return { path, token: token ?? null, line, column, endLine: line, endColumn, preview };
}

const answer = (...locations: ReturnType<typeof loc>[]) => ({
  staleVersion: false,
  server: "rust-analyzer",
  locations,
  truncated: false,
});

/** 지금 활성 탭의 경로(외부 소스 탭은 보일 경로)와 커서(줄, 열 — 1 부터). */
function here() {
  const v = view();
  const head = v.state.selection.main.head;
  const line = v.state.doc.lineAt(head);
  return {
    path: v.state.facet(editorContext)?.path,
    line: line.number,
    column: head - line.from + 1,
  };
}

/** 커서를 (줄, 열)에 두고 에디터에 포커스. */
async function caret(line: number, column: number) {
  await act(async () => {
    const v = view();
    v.focus();
    v.dispatch({ selection: { anchor: v.state.doc.line(line).from + column - 1 } });
  });
}

const cmd = (code: string, key: string, more: KeyboardEventInit = {}) =>
  keydown({ code, key, metaKey: true, ...more }, view().contentDOM);

const popupRows = () =>
  [...(popupDom()?.querySelectorAll<HTMLElement>("[data-location]") ?? [])].map(
    (r) => r.dataset.location,
  );

/** 마우스가 가리킬 문서 위치 — happy-dom 은 배치가 없어 `posAtCoords` 를 이것으로 돌린다. */
let pointer = 0;
function mouse(type: string, init: MouseEventInit = {}) {
  return act(async () => {
    view().contentDOM.dispatchEvent(
      new MouseEvent(type, { bubbles: true, cancelable: true, clientX: 1, clientY: 1, ...init }),
    );
  });
}

beforeEach(() => {
  pointer = 0;
  vi.spyOn(EditorView.prototype, "posAtCoords").mockImplementation(() => pointer);
});

it("goes to the one declaration, back with ⌘[, and lists several in a popup (①, ⑤)", async () => {
  serve();
  host.lsp!.handle("lsp.definition", async () => answer(loc(LIB, 1, 8, 14, "pub fn helper() {}")));
  await mount();
  await open(MAIN, 1, 1);
  await caret(2, 6);
  await cmd("KeyB", "b");
  await settle();
  expect(host.lsp!.called("call:lsp.definition")).toEqual([
    { docId: "d1", version: 0, offset: 17 },
  ]);
  expect(here()).toEqual({ path: LIB, line: 1, column: 8 });
  // ⑤ 정의로 간 뒤 ⌘[ 로 돌아온다(위치로 가기가 떠난 자리를 기록에 넣었다 — M4-09).
  await keydown({ code: "BracketLeft", key: "[", metaKey: true }, view().contentDOM);
  await settle();
  expect(here()).toEqual({ path: MAIN, line: 2, column: 6 });

  // 셋이면 위치 팝업 — 파일 · 줄 · 줄 글, ↓ ↩ 로 고른다.
  host.lsp!.handle("lsp.definition", async () =>
    answer(
      loc(LIB, 1, 8, 14, "pub fn helper() {}"),
      loc(LIB, 2, 8, 13, "pub fn other() {}"),
      loc(LIB, 3, 8, 13, "pub fn third() {}"),
    ),
  );
  await caret(2, 6);
  await cmd("KeyB", "b");
  await settle();
  expect(currentPopup(view())?.kind).toBe("locations");
  expect(popupRows()).toEqual([`${LIB}:1`, `${LIB}:2`, `${LIB}:3`]);
  expect(popupDom()!.textContent).toContain("pub fn other() {}");
  const list = popupDom()!.querySelector<HTMLElement>("[role=listbox]")!;
  expect(document.activeElement).toBe(list);
  // 목록이 받은 키는 창(키맵)까지 가지 않는다.
  const reached = vi.fn();
  window.addEventListener("keydown", reached);
  await keydown({ code: "ArrowDown", key: "ArrowDown" }, list);
  window.removeEventListener("keydown", reached);
  expect(reached).not.toHaveBeenCalled();
  await keydown({ code: "Enter", key: "Enter" }, list);
  await settle();
  expect(here()).toEqual({ path: LIB, line: 2, column: 8 });
});

it("shows the usages when the caret is on the declaration itself (①)", async () => {
  serve();
  // 커서가 `main` 선언 위 — 정의의 답이 그 자리를 품는다.
  host.lsp!.handle("lsp.definition", async () => answer(loc(MAIN, 1, 4, 8, "fn main() {")));
  host.lsp!.handle("lsp.references", async () =>
    answer(loc(LIB, 1, 1, 3, "pub fn helper() {}"), loc(MAIN, 3, 5, 10, "    other();")),
  );
  await mount();
  await open(MAIN, 1, 1);
  await caret(1, 5);
  await cmd("KeyB", "b");
  await settle();
  expect(host.lsp!.called("call:lsp.references")).toEqual([
    { docId: "d1", version: 0, offset: 4, includeDeclaration: false },
  ]);
  expect(currentPopup(view())?.kind).toBe("locations");
  expect(popupDom()!.textContent).toContain("「main」 의 사용처 · 2곳");
  expect(here().path).toBe(MAIN);
});

it("goes to the implementation with ⌘⌥B and with ⌘⌥+click, and the declaration with ⌘+click", async () => {
  serve();
  host.lsp!.handle("lsp.implementation", async () =>
    answer(loc(LIB, 3, 8, 13, "pub fn third() {}")),
  );
  host.lsp!.handle("lsp.definition", async () => answer(loc(LIB, 2, 8, 13, "pub fn other() {}")));
  await mount();
  await open(MAIN, 1, 1);
  await caret(2, 6);
  await cmd("KeyB", "b", { altKey: true });
  await settle();
  expect(here()).toEqual({ path: LIB, line: 3, column: 8 });

  await open(MAIN, 1, 1);
  pointer = 28; // 셋째 줄의 `other`
  await mouse("mousedown", { metaKey: true });
  await settle();
  expect(host.lsp!.called("call:lsp.definition").at(-1)).toMatchObject({ offset: 28 });
  expect(here()).toEqual({ path: LIB, line: 2, column: 8 });

  await open(MAIN, 1, 1);
  pointer = 17;
  await mouse("mousedown", { metaKey: true, altKey: true });
  await settle();
  expect(host.lsp!.called("call:lsp.implementation").at(-1)).toMatchObject({ offset: 17 });
  expect(here()).toEqual({ path: LIB, line: 3, column: 8 });
});

it("pins usages to the Search Results tool window as a file tree and opens a line with ↩ (④)", async () => {
  serve();
  host.lsp!.handle("lsp.references", async () =>
    answer(
      loc(LIB, 1, 8, 14, "pub fn helper() {}"),
      loc(MAIN, 2, 5, 11, "    helper();"),
      loc(LIB, 3, 1, 4, "pub fn third() {}"),
    ),
  );
  // 「검색 결과」 트리는 가상화한다 — happy-dom 에는 배치가 없어 트리에 보이는 높이를 준다(find-in-files.test.tsx 와 같다).
  for (const prop of ["offsetHeight", "clientHeight"] as const)
    vi.spyOn(HTMLElement.prototype, prop, "get").mockImplementation(function (this: HTMLElement) {
      return this.getAttribute("role") === "tree" ? 600 : 0;
    });
  await mount();
  await open(MAIN, 1, 1);
  await caret(2, 6);
  await keydown({ code: "F7", key: "F7", altKey: true }, view().contentDOM);
  await settle();
  expect(popupRows()).toEqual([`${LIB}:1`, `${MAIN}:2`, `${LIB}:3`]);
  expect(popupDom()!.textContent).toContain("⌘↩ 검색 결과에 고정");
  const list = popupDom()!.querySelector<HTMLElement>("[role=listbox]")!;
  await keydown({ code: "Enter", key: "Enter", metaKey: true }, list);
  await settle();
  // 활성 그룹이 툴 윈도우로 옮겨 갔다 — 팝업은 문서 전체에서 본다.
  expect(document.querySelector(".pw-lsp-popup")).toBeNull();
  const tool = document.querySelector<HTMLElement>('[data-tool-window="searchResults"]')!;
  expect(tool.textContent).toContain("「helper」 의 사용처 · 3곳");
  expect(
    [...tool.querySelectorAll<HTMLElement>("[data-search-result]")].map(
      (r) => r.dataset.searchResult,
    ),
  ).toEqual([LIB, `${LIB}:1`, `${LIB}:3`, MAIN, `${MAIN}:2`]);
  const tree = tool.querySelector<HTMLElement>("[role=tree]")!;
  await act(async () => tree.focus());
  for (let i = 0; i < 5; i++) await keydown({ code: "ArrowDown", key: "ArrowDown" }, tree);
  await keydown({ code: "Enter", key: "Enter" }, tree);
  await settle();
  expect(here()).toEqual({ path: MAIN, line: 2, column: 5 });
});

it("opens a location outside the root read-only through its token, asks the same server inside and comes back (②)", async () => {
  serve();
  host.lsp!.handle("lsp.definition", async (p) =>
    (p as { token?: string }).token
      ? answer(loc(VEC, 1, 12, 15, "pub struct Vec;", "tok-1"))
      : answer(loc(VEC, 2, 6, 9, "impl Vec {}", "tok-1")),
  );
  host.lsp!.handle("lsp.document.read", async (p) => {
    expect(p).toEqual({ token: "tok-1" });
    return {
      text: "pub struct Vec;\nimpl Vec {}\n",
      uri: `file://${VEC}`,
      path: VEC,
      server: "rust-analyzer",
    };
  });
  host.lsp!.handle("lsp.hover", async () => ({
    staleVersion: false,
    hover: { kind: "markdown", value: "struct **Vec**", from: null, to: null },
  }));
  await mount();
  await open(MAIN, 1, 1);
  await caret(2, 6);
  await cmd("KeyB", "b");
  await settle();
  const tab = document.querySelector<HTMLElement>('[data-outside-source="tok-1"]')!;
  expect(tab.querySelector("[data-outside-source-bar]")!.textContent).toBe(
    `외부 소스 · 읽기 전용 · rust-analyzer${VEC}`,
  );
  expect(here()).toEqual({ path: VEC, line: 2, column: 6 });
  // 읽기 전용: 편집 키는 아무것도 바꾸지 않고, ⌘S 는 쓰지 않는다(쓰기 RPC 가 없다).
  expect(view().state.readOnly).toBe(true);
  await cmd("Backspace", "Backspace");
  await cmd("KeyD", "d");
  await cmd("KeyS", "s");
  await settle();
  expect(view().state.doc.toString()).toBe("pub struct Vec;\nimpl Vec {}\n");
  expect(
    host.calls.filter((c) => c.method === "fs.write" || c.method.startsWith("lsp.document.change")),
  ).toEqual([]);
  expect(host.lsp!.called("open")).toHaveLength(1);
  // 그 문서 안의 hover · 정의는 같은 서버로(토큰).
  await keydown({ code: "F1", key: "F1" }, view().contentDOM);
  await settle();
  expect(host.lsp!.called("call:lsp.hover")).toEqual([{ token: "tok-1", offset: 21 }]);
  expect(popupDom()!.textContent).toContain("struct Vec");
  await cmd("KeyB", "b");
  await settle();
  expect(host.lsp!.called("call:lsp.definition").at(-1)).toEqual({ token: "tok-1", offset: 21 });
  expect(here()).toEqual({ path: VEC, line: 1, column: 12 });
  // 같은 토큰은 탭 하나.
  expect(document.querySelectorAll('[data-outside-source="tok-1"]')).toHaveLength(1);
  // 뒤로 가기가 외부 소스 탭 안에서도 오가고 원래 파일로 돌아온다.
  await keydown({ code: "BracketLeft", key: "[", metaKey: true }, view().contentDOM);
  await settle();
  expect(here()).toEqual({ path: VEC, line: 2, column: 6 });
  await keydown({ code: "BracketLeft", key: "[", metaKey: true }, view().contentDOM);
  await settle();
  expect(here()).toEqual({ path: MAIN, line: 2, column: 6 });
  // 파일 탭이 아니다 — 최근 파일(⌘E)에 들지 않는다(활성 탭이 아닐 때 본다 — 목록은 지금 탭을 빼고 보인다).
  await keydown({ code: "KeyE", key: "e", metaKey: true }, view().contentDOM);
  await settle();
  const recent = document.querySelector<HTMLElement>("[data-search-everywhere]")!;
  expect(recent.textContent).toContain("main.rs");
  expect(recent.textContent).not.toContain("mod.rs");
  expect(recent.textContent).not.toContain("tok-1");
  await keydown({ code: "Escape", key: "Escape" });
  await settle();
});

it("opens a location inside the root from an outside tab in the worktree its token came from (review L4)", async () => {
  const WT = "/work/demo-wt";
  const WMAIN = `${WT}/src/main.rs`;
  const WLIB = `${WT}/src/lib.rs`;
  host.files.set(WMAIN, host.files.get(MAIN)!);
  host.files.set(WLIB, host.files.get(LIB)!);
  serve();
  host.lsp!.handle("lsp.definition", async (p) =>
    (p as { token?: string }).token
      ? answer(loc(WLIB, 1, 8, 14, "pub fn helper() {}"))
      : answer(loc(VEC, 1, 12, 15, "pub struct Vec;", "tok-l4")),
  );
  host.lsp!.handle("lsp.document.read", async () => ({
    text: "pub struct Vec;\n",
    uri: `file://${VEC}`,
    path: VEC,
    server: "rust-analyzer",
  }));
  await mount();
  await act(async () => projectOpenInbox.push({ worktree: WT, file: WMAIN, line: 2, col: 6 }));
  await settle();
  await cmd("KeyB", "b");
  await settle();
  expect(here().path).toBe(VEC);
  // 외부 소스 탭 안에서 루트 안 위치로 — 그 토큰을 받은 문서의 워크트리로 연다.
  await cmd("KeyB", "b");
  await settle();
  expect(here().path).toBe(WLIB);
  const reads = host.calls.filter(
    (c) => c.method === "fs.read" && (c.params as { path: string }).path === WLIB,
  );
  expect(reads.length).toBeGreaterThan(0);
  expect(reads.every((c) => (c.params as { worktree: string | null }).worktree === WT)).toBe(true);
});

it("does not open an outside tab when the token cannot be read", async () => {
  serve();
  host.lsp!.handle("lsp.definition", async () => answer(loc(VEC, 1, 1, 4, "x", "tok-gone")));
  host.lsp!.handle("lsp.document.read", async () => {
    throw new Error("no such document token");
  });
  await mount();
  await open(MAIN, 1, 1);
  await caret(2, 6);
  await cmd("KeyB", "b");
  await settle();
  expect(document.querySelector("[data-outside-source]")).toBeNull();
  expect(here().path).toBe(MAIN);
});

it("opens a file the server shows with window/showDocument (③ — the Host lets only files inside the root through)", async () => {
  serve();
  await mount();
  await open(MAIN, 1, 1);
  await emit("lsp.showDocument", {
    window: null,
    path: LIB,
    worktree: null,
    line: 2,
    column: 5,
    takeFocus: true,
    server: "rust-analyzer",
  });
  expect(here()).toEqual({ path: LIB, line: 2, column: 5 });
});

it("opens a shown document only in the window the Host picked, and moves focus only when the server asks (review M2)", async () => {
  focus.label = "project-a";
  serve();
  await mount();
  await open(MAIN, 1, 1);
  const show = (window: string | null, takeFocus: boolean, line: number) =>
    emit("lsp.showDocument", {
      window,
      path: LIB,
      worktree: null,
      line,
      column: 1,
      takeFocus,
      server: "rust-analyzer",
    });
  // 다른 창을 가리키는 이벤트(그 서버를 더 최근에 쓴 창) · label 없는 이벤트는 이 창이 열지 않는다.
  await show("project-b", true, 2);
  await show(null, true, 2);
  expect(here().path).toBe(MAIN);
  expect(focus.forward).toBe(0);
  // takeFocus 가 아니면 (새 탭으로) 열되 키보드 포커스는 그 전 자리 그대로, 창도 건드리지 않는다.
  const field = document.createElement("input");
  document.body.append(field);
  field.focus();
  await show("project-a", false, 3);
  expect(here()).toEqual({ path: LIB, line: 3, column: 1 });
  expect(document.activeElement).toBe(field);
  expect(focus.forward).toBe(0);
  // takeFocus 면 열고 창을 앞으로 가져오기를 부른다(앱이 포커스를 가졌을 때만 가져오는 것은 window-focus.test.ts).
  await show("project-a", true, 2);
  expect(here()).toEqual({ path: LIB, line: 2, column: 1 });
  expect(focus.forward).toBe(1);
});

it("shows quick documentation as a pinned popup and never lets the server's Markdown run as HTML", async () => {
  serve();
  host.lsp!.handle("lsp.hover", async () => ({
    staleVersion: false,
    hover: {
      kind: "markdown",
      value:
        "<script>window.__pwned = 1</script> **helper**\n\n![pixel](https://tracker.example/p.png)\n\n[run](javascript:alert(1)) [docs](https://docs.example/helper)",
      from: 16,
      to: 22,
    },
  }));
  await mount();
  await open(MAIN, 1, 1);
  await caret(2, 6);
  await keydown({ code: "F1", key: "F1" }, view().contentDOM);
  await settle();
  const dom = popupDom()!;
  expect(currentPopup(view())).toMatchObject({ kind: "hover", data: { pinned: true } });
  expect(dom.querySelector("script")).toBeNull();
  expect(dom.textContent).toContain("<script>window.__pwned = 1</script>");
  expect((window as { __pwned?: number }).__pwned).toBeUndefined();
  // 이미지는 주소를 읽지 않는다(`src` 없이 원래 주소만 담는다).
  const img = dom.querySelector("img")!;
  expect(img.getAttribute("src")).toBeNull();
  // 링크는 http(s) 만 밖으로.
  const links = [...dom.querySelectorAll("a")];
  await act(async () => links.find((a) => a.textContent === "run")?.click());
  expect(host.opened).toEqual([]);
  await act(async () => links.find((a) => a.textContent === "docs")!.click());
  expect(host.opened).toEqual(["https://docs.example/helper"]);
  // 고정 팝업은 마우스가 다른 낱말로 가도 남는다.
  pointer = 40;
  await mouse("mousemove");
  expect(popupDom()).not.toBeNull();
});

it("hovers after 0.5 s, underlines with ⌘ and previews the declaration, and closes when the mouse leaves the word", async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  serve();
  host.lsp!.handle("lsp.hover", async () => ({
    staleVersion: false,
    hover: { kind: "plaintext", value: "fn helper()", from: null, to: null },
  }));
  host.lsp!.handle("lsp.definition", async () => answer(loc(LIB, 1, 8, 14, "pub fn helper() {}")));
  await mount();
  await open(MAIN, 1, 1);
  pointer = 17;
  await mouse("mousemove");
  await act(async () => vi.advanceTimersByTimeAsync(400));
  expect(host.lsp!.called("call:lsp.hover")).toEqual([]);
  await act(async () => vi.advanceTimersByTimeAsync(150));
  await settle();
  expect(host.lsp!.called("call:lsp.hover")).toEqual([{ docId: "d1", version: 0, offset: 17 }]);
  expect(popupDom()!.textContent).toBe("fn helper()");
  pointer = 40;
  await mouse("mousemove");
  expect(popupDom()).toBeNull();

  // 답을 기다리는 사이 마우스가 낱말을 떠나면 그 답은 버린다.
  let answerLate: (v: unknown) => void = () => {};
  host.lsp!.handle("lsp.hover", () => new Promise((r) => (answerLate = r)));
  pointer = 17;
  await mouse("mousemove");
  await act(async () => vi.advanceTimersByTimeAsync(550));
  await settle();
  pointer = 40;
  await mouse("mousemove");
  await act(async () =>
    answerLate({
      staleVersion: false,
      hover: { kind: "plaintext", value: "late", from: null, to: null },
    }),
  );
  await settle();
  expect(popupDom()).toBeNull();

  // ⌘ 를 누른 채: 밑줄과 선언 미리보기(정의의 앞 줄들을 코드 블록으로).
  pointer = 17;
  await mouse("mousemove", { metaKey: true });
  expect(view().dom.querySelector(".pw-lsp-link")?.textContent).toBe("helper");
  await act(async () => vi.advanceTimersByTimeAsync(550));
  await settle();
  const code = popupDom()!.querySelector("pre code")!;
  expect(code.textContent).toBe("pub fn helper() {}\npub fn other() {}\npub fn third() {}\n");
  await act(async () => window.dispatchEvent(new KeyboardEvent("keyup", { key: "Meta" })));
  expect(view().dom.querySelector(".pw-lsp-link")).toBeNull();
});

/** `⌘` 를 누른 채 「helper」 위에 머물러 선언 미리보기의 글을 받고, 마우스를 낱말 밖으로 뺀다. */
async function cmdPreview() {
  pointer = 17;
  await mouse("mousemove", { metaKey: true });
  await act(async () => vi.advanceTimersByTimeAsync(550));
  await settle();
  const text = popupDom()?.querySelector("pre code")?.textContent;
  pointer = 40;
  await mouse("mousemove", { metaKey: true });
  return text;
}

it("reads only the head of a file inside the root for the ⌘ preview (review L3)", async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  serve();
  // 정의가 앞부분 상한 뒤에 있는 큰 파일 — 앞부분만 읽고 위치의 한 줄로 물러난다.
  host.files.set(LIB, `${"x".repeat(PREVIEW_READ_BYTES)}\npub fn helper() {}\npub fn other() {}\n`);
  host.lsp!.handle("lsp.definition", async () => answer(loc(LIB, 2, 8, 14, "pub fn helper() {}")));
  await mount();
  await open(MAIN, 1, 1);
  expect(await cmdPreview()).toBe("pub fn helper() {}\n");
  expect(
    host.calls.filter((c) => c.method === "fs.read" && (c.params as { path: string }).path === LIB),
  ).toEqual([
    {
      method: "fs.read",
      params: { projectId: "p1", worktree: null, path: LIB, offset: 0, length: PREVIEW_READ_BYTES },
    },
  ]);
});

it("reads a token document once for the ⌘ preview of a location outside the root (review L3)", async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  serve();
  host.lsp!.handle("lsp.definition", async () =>
    answer(loc(VEC, 1, 12, 15, "pub struct Vec;", "tok-l3")),
  );
  let reads = 0;
  host.lsp!.handle("lsp.document.read", async () => {
    reads++;
    return {
      text: "pub struct Vec;\nimpl Vec {}\n",
      uri: `file://${VEC}`,
      path: VEC,
      server: "rust-analyzer",
    };
  });
  await mount();
  await open(MAIN, 1, 1);
  // 한 번 읽은 미리보기를 기억한다 — 같은 정의 위에서 다시 ⌘ 를 눌러도 다시 읽지 않는다.
  expect(await cmdPreview()).toBe("pub struct Vec;\nimpl Vec {}\n");
  expect(await cmdPreview()).toBe("pub struct Vec;\nimpl Vec {}\n");
  expect(reads).toBe(1);
});

it("does not hover past the end of a line that ends with a word (review L5)", async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  serve();
  host.files.set(MAIN, "fn main() {\n    let x = helper\n}\n");
  host.lsp!.handle("lsp.hover", async () => ({
    staleVersion: false,
    hover: { kind: "plaintext", value: "fn helper()", from: null, to: null },
  }));
  // 글자 하나가 10px 인 배치(happy-dom 은 배치가 없다). 「helper」 는 24..30 — 화면에서 240..300.
  vi.spyOn(EditorView.prototype, "coordsAtPos").mockImplementation((pos: number) => ({
    left: pos * 10,
    right: pos * 10,
    top: 0,
    bottom: 10,
  }));
  await mount();
  await open(MAIN, 1, 1);
  // 줄 끝 오른쪽 빈 곳 — 가장 가까운 위치는 줄 끝(30)이지만 낱말 밖이다.
  pointer = 30;
  await mouse("mousemove", { clientX: 500 });
  await act(async () => vi.advanceTimersByTimeAsync(600));
  await settle();
  expect(host.lsp!.called("call:lsp.hover")).toEqual([]);
  // 낱말 위면 묻고, 거기서 줄 끝 너머로 나가면 팝업이 닫힌다.
  pointer = 27;
  await mouse("mousemove", { clientX: 270 });
  await act(async () => vi.advanceTimersByTimeAsync(600));
  await settle();
  expect(host.lsp!.called("call:lsp.hover")).toHaveLength(1);
  expect(popupDom()!.textContent).toBe("fn helper()");
  pointer = 30;
  await mouse("mousemove", { clientX: 500 });
  expect(popupDom()).toBeNull();
});

it("stays silent when no server takes the feature (an untrusted project starts none)", async () => {
  await mount();
  await open(MAIN, 1, 1);
  await caret(2, 6);
  await cmd("KeyB", "b");
  await keydown({ code: "F1", key: "F1" }, view().contentDOM);
  await keydown({ code: "F7", key: "F7", altKey: true }, view().contentDOM);
  await settle();
  expect(host.calls.filter((c) => /^lsp\.(hover|definition|references)$/.test(c.method))).toEqual(
    [],
  );
  expect(popupDom()).toBeNull();
  expect(document.querySelector("[data-notification-level=error]")).toBeNull();
});

it("asks nothing while Korean IME composition is going on", async () => {
  serve();
  host.lsp!.handle("lsp.hover", async () => ({ staleVersion: false, hover: null }));
  await mount();
  await open(MAIN, 1, 1);
  await caret(2, 6);
  await compose("compositionstart");
  await composeText(16, 16, "ㅎ");
  await keydown({ code: "F1", key: "F1" }, view().contentDOM);
  await settle();
  expect(host.lsp!.called("call:lsp.hover")).toEqual([]);
});

it("closes the location popup with Esc without folding the cursors (M4-14)", async () => {
  serve();
  host.lsp!.handle("lsp.definition", async () =>
    answer(loc(LIB, 1, 8, 14, "a"), loc(LIB, 2, 8, 13, "b")),
  );
  await mount();
  await open(MAIN, 1, 1);
  await caret(2, 6);
  await act(async () =>
    view().dispatch({
      selection: EditorSelection.create([EditorSelection.cursor(1), EditorSelection.cursor(17)], 1),
    }),
  );
  await cmd("KeyB", "b");
  await settle();
  const list = popupDom()!.querySelector<HTMLElement>("[role=listbox]")!;
  await keydown({ code: "Escape", key: "Escape" }, list);
  expect(popupDom()).toBeNull();
  expect(ranges()).toHaveLength(2);
});
