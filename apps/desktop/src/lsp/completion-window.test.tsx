// @vitest-environment happy-dom
// 자동완성 · 시그니처 도움말(15-m4-plan M4-11)을 프로젝트 창(App kind="project")과 가짜 Host 로 본다 — 키 입력에서 목록까지 기다림이
// 없는지(가짜 시계), 모자란 목록만 글자마다 다시 묻는지, 바꾸기 범위 · 자동 import 가 undo 한 번인지, 스니펫 탭 정지, 고른 항목만
// resolve 하고 지난 것을 취소하는지, 한글 조합 중 ↩ 가 고르지 않는지, 시그니처의 지금 인자가 `,` 마다 옮겨지는지, 편집 직후 요청이
// 버려지지 않는지(M4-07 H-1). 창 틀(가짜 Host · 마운트)은 lsp-window.test.tsx 와 같고, 기능 RPC(`lsp.completion` 등)는 가짜 Host 의
// 처리기(`fakeLspServer().handle`)로 간다 — `request.cancel` 은 그 처리기의 `AbortSignal` 이 된다.
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
  LspCompletionItem,
  LspCompletionList,
  LspSignatureHelpAnswer,
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
  /** 도는 기능 RPC 의 취소(`request.cancel`). */
  cancels: new Map<number, AbortController>(),
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
        if (m.method === "request.cancel") {
          host.cancels.get((m.params as { id: number }).id)?.abort();
          return;
        }
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
        // 기능 행의 Host RPC(`LspServices.call`) — 가짜 Host 의 처리기로. `request.cancel` 이 오면 그 처리기의 signal 이 abort 된다.
        if (/^lsp\.(completion|signatureHelp)/.test(m.method) && host.lsp && m.id !== undefined) {
          const abort = new AbortController();
          host.cancels.set(m.id, abort);
          return void host.lsp.services
            .call(m.method as never, m.params as never, { signal: abort.signal })
            .then(reply, (e: { code?: number; message: string }) =>
              send({ id: m.id, error: { code: e.code ?? -32603, message: e.message } }),
            )
            .finally(() => host.cancels.delete(m.id!));
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

const MAIN = "/work/demo/src/main.rs";

let root: Root | undefined;

function useUserAgent(ua: string) {
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(ua);
}

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  useUserAgent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)");
  vi.spyOn(console, "info").mockImplementation(() => {});
  window.history.replaceState(null, "", "/?window=project&project=p1");
  host.files = new Map([[MAIN, "\n"]]);
  host.settings = {};
  host.subscribers = [];
  host.syntax = undefined;
  host.lsp = fakeLspServer();
  host.sizeClass = new Map();
  host.cancels = new Map();
  host.opened = [];
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

const lspContext = () => view().state.facet(editorContext)?.lsp;

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

// ---- 가짜 서버의 기능 ------------------------------------------------------------------------------------------------------------

type CompletionAsk = {
  docId: string;
  version: number;
  offset: number;
  trigger: { kind: string; character?: string };
};

/** 완성 · 시그니처를 맡는 서버의 능력 칸. */
function features(options: { resolveProvider?: boolean } = {}) {
  return {
    completion: {
      server: "ra",
      options: { triggerCharacters: ["."], resolveProvider: options.resolveProvider ?? false },
      optionsTruncated: false,
    },
    signature_help: {
      server: "ra",
      options: { triggerCharacters: ["("], retriggerCharacters: [","] },
      optionsTruncated: false,
    },
  };
}

function item(label: string, extra: Partial<LspCompletionItem> = {}): LspCompletionItem {
  return {
    label,
    labelDetail: null,
    labelDescription: null,
    kind: 2,
    detail: null,
    documentation: null,
    filterText: null,
    sortText: null,
    preselect: false,
    deprecated: false,
    edit: null,
    insertText: null,
    snippet: false,
    additionalEdits: [],
    commitCharacters: null,
    command: null,
    raw: { label },
    ...extra,
  };
}

/** `lsp.completion` 처리기: 물은 것을 모으고 `answer` 로 답한다(Host 의 `staleVersion` 규칙 그대로). */
function serveCompletion(answer: (ask: CompletionAsk) => Partial<LspCompletionList>) {
  const asked: CompletionAsk[] = [];
  host.lsp!.handle("lsp.completion", async (params) => {
    const ask = params as CompletionAsk;
    asked.push(ask);
    return {
      server: "ra",
      staleVersion: host.lsp!.stale(ask),
      isIncomplete: false,
      truncated: false,
      items: [],
      ...answer(ask),
    } satisfies LspCompletionList;
  });
  return asked;
}

async function openMain(text: string, cursor: number, resolveProvider = false) {
  host.files.set(MAIN, text);
  host.lsp!.setFeatures(features({ resolveProvider }));
  await mount();
  await open(MAIN, 1, 1);
  await act(async () => {
    view().focus();
    view().dispatch({ selection: { anchor: cursor } });
  });
  await settle();
}

/** 사람이 친 글자(`input.type`) — 한 글자씩. */
async function type(chars: string) {
  for (const ch of chars) {
    await act(async () => {
      const v = view();
      v.dispatch({
        ...v.state.replaceSelection(ch),
        userEvent: "input.type",
        scrollIntoView: true,
      });
    });
    await settle();
  }
}

const text = () => view().state.doc.toString();
const listOpen = () => view().dom.querySelector(".cm-tooltip-autocomplete") !== null;
const listLabels = () =>
  [...view().dom.querySelectorAll(".cm-tooltip-autocomplete li .cm-completionLabel")].map(
    (e) => e.textContent,
  );
const selection = () => {
  const r = view().state.selection.main;
  return view().state.sliceDoc(r.from, r.to);
};
/** CodeMirror 는 목록이 열린 뒤 75 ms 동안 고르는 키를 받지 않는다(`interactionDelay`). */
const afterInteractionDelay = () => act(async () => new Promise((r) => setTimeout(r, 90)));
const press = (key: string, more: KeyboardEventInit = {}) =>
  keydown({ key, code: key, ...more }, view().contentDOM);
/** macOS 의 ⌥Space(키 값은 줄 바꿈 없는 공백이다 — 키맵은 `code` 로 본다). */
const optionSpace = () => keydown({ key: " ", code: "Space", altKey: true }, view().contentDOM);

// ---- 시험 ----------------------------------------------------------------------------------------------------------------------

it("asks and opens the list on a typed trigger and on ⌥Space with no timer in between (①)", async () => {
  const asked = serveCompletion(() => ({ items: [item("area"), item("width")] }));
  await openMain("r\n", 1);
  vi.useFakeTimers();
  // 0 ms 타이머와 마이크로태스크만 돌린다 — 대기열 · 디바운스(지연 있는 타이머)가 있으면 묻지도 열지도 않는다.
  const zero = () =>
    act(async () => {
      for (let i = 0; i < 30; i++) await vi.advanceTimersByTimeAsync(0);
    });
  try {
    await act(async () => {
      const v = view();
      v.dispatch({ ...v.state.replaceSelection("."), userEvent: "input.type" });
    });
    await zero();
    expect(asked).toEqual([
      { docId: "d1", version: 1, offset: 2, trigger: { kind: "character", character: "." } },
    ]);
    expect(listLabels()).toEqual(["area", "width"]);
    await act(async () => {
      view().contentDOM.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "Escape",
          code: "Escape",
          bubbles: true,
          cancelable: true,
        }),
      );
    });
    await zero();
    expect(listOpen()).toBe(false);
    await act(async () => {
      view().contentDOM.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: " ",
          code: "Space",
          altKey: true,
          bubbles: true,
          cancelable: true,
        }),
      );
    });
    await zero();
    expect(asked[1]).toEqual({ docId: "d1", version: 1, offset: 2, trigger: { kind: "invoked" } });
    expect(listLabels()).toEqual(["area", "width"]);
    // ⌥Space 는 글자를 넣지 않는다(키맵이 받았다).
    expect(text()).toBe("r.\n");
  } finally {
    vi.useRealTimers();
  }
});

it("asks again on every letter only while the list is incomplete (②)", async () => {
  let incomplete = true;
  const asked = serveCompletion(() => ({
    isIncomplete: incomplete,
    items: [item("area"), item("arc"), item("apple")],
  }));
  await openMain("r\n", 1);
  await type(".ar");
  // 「더 있음」 표지 — 모자란 목록이다.
  expect(view().dom.querySelector(".cm-tooltip-autocomplete")!.classList).toContain(
    "pw-lsp-completion-more",
  );
  expect(asked.map((a) => a.trigger)).toEqual([
    { kind: "character", character: "." },
    { kind: "incomplete" },
    { kind: "incomplete" },
  ]);
  await press("Escape");
  incomplete = false;
  await act(async () => view().dispatch({ selection: { anchor: view().state.doc.length } }));
  await type("q.");
  const before = asked.length;
  expect(asked.at(-1)!.trigger).toEqual({ kind: "character", character: "." });
  // 완전한 목록은 창에서 거른다 — 더 치어도 묻지 않는다.
  await type("ar");
  expect(asked).toHaveLength(before);
  expect(listLabels()).toEqual(["arc", "area"]);
  expect(view().dom.querySelector(".cm-tooltip-autocomplete")!.classList).not.toContain(
    "pw-lsp-completion-more",
  );
});

it("filters by filterText, orders equal matches by sortText and shows the kind icon", async () => {
  serveCompletion(() => ({
    items: [
      item("zeta", { sortText: "1", kind: 6 }),
      item("alpha", { sortText: "2", kind: 3 }),
      item("•describe()", { filterText: "describe", sortText: "0", kind: 15 }),
    ],
  }));
  await openMain("r\n", 1);
  await type(".");
  // 빈 접두어 — 점수가 같아 `sortText` 순이다(이름 순이 아니다).
  expect(listLabels()).toEqual(["•describe()", "zeta", "alpha"]);
  expect(view().dom.querySelector(".cm-completionIcon-variable")).not.toBeNull();
  expect(view().dom.querySelector(".cm-completionIcon-snippet")).not.toBeNull();
  await type("des");
  expect(listLabels()).toEqual(["•describe()"]);
});

it("follows the server's trigger characters as its registrations change", async () => {
  const asked = serveCompletion(() => ({ items: [item("area")] }));
  await openMain("r\n", 1);
  await type(".");
  expect(asked).toHaveLength(1);
  await press("Escape");
  // 서버가 동적 등록으로 트리거를 `:` 로 바꿨다 — Host 가 `lsp.status` 를 내고 창이 능력 칸을 다시 묻는다.
  host.lsp!.setFeatures({
    completion: { server: "ra", options: { triggerCharacters: [":"] }, optionsTruncated: false },
  });
  await emit("lsp.status", { projectId: "p1", instances: [] });
  await type(" .");
  expect(asked).toHaveLength(1);
  await type(":");
  expect(asked.at(-1)!.trigger).toEqual({ kind: "character", character: ":" });
  expect(asked).toHaveLength(2);
});

it("applies the replace range with Tab, the insert range with Enter, and the import with the same undo (③)", async () => {
  // `use b;\n` 은 일곱 글자 — `r.` 뒤(9)에서 묻고 커서 뒤 `ar` 는 바꾸기 범위다.
  const edit = {
    newText: "area",
    insert: { from: 9, to: 9 },
    replace: { from: 9, to: 11 },
  };
  serveCompletion(() => ({
    items: [
      item("area", { edit, additionalEdits: [{ from: 0, to: 0, newText: "use a::Shape;\n" }] }),
    ],
  }));
  await openMain("use b;\nr.ar;\n", 9);
  await optionSpace();
  await afterInteractionDelay();
  await press("Tab");
  expect(text()).toBe("use a::Shape;\nuse b;\nr.area;\n");
  // undo 한 번에 본 편집과 자동 import 가 함께 돌아간다.
  await keydown({ key: "z", code: "KeyZ", metaKey: true }, view().contentDOM);
  expect(text()).toBe("use b;\nr.ar;\n");
  await act(async () => view().dispatch({ selection: { anchor: 9 } }));
  await optionSpace();
  await afterInteractionDelay();
  await press("Enter");
  expect(text()).toBe("use a::Shape;\nuse b;\nr.areaar;\n");
});

it("waits for resolve to bring the auto-import of the accepted item, still in one undo (③)", async () => {
  serveCompletion(() => ({ items: [item("Shape", { raw: { label: "Shape", data: 1 } })] }));
  host.lsp!.handle("lsp.completion.resolve", async (params) => ({
    server: "ra",
    staleVersion: false,
    item: item("Shape", {
      additionalEdits: [{ from: 0, to: 0, newText: "import { Shape } from './shapes';\n" }],
      raw: (params as { item: JsonValue }).item,
    }),
  }));
  await openMain("let s: Sha\n", 10, true);
  await optionSpace();
  await afterInteractionDelay();
  await press("Enter");
  expect(text()).toBe("import { Shape } from './shapes';\nlet s: Shape\n");
  await keydown({ key: "z", code: "KeyZ", metaKey: true }, view().contentDOM);
  expect(text()).toBe("let s: Sha\n");
});

it("moves through snippet tab stops (④)", async () => {
  serveCompletion(() => ({
    items: [item("scale", { snippet: true, insertText: "scale(${1:width}, ${2:height})$0" })],
  }));
  await openMain("sc\n", 2);
  await optionSpace();
  await afterInteractionDelay();
  await press("Enter");
  expect(text()).toBe("scale(width, height)\n");
  expect(selection()).toBe("width");
  await press("Tab");
  expect(selection()).toBe("height");
  await press("Tab");
  expect(view().state.selection.main.head).toBe("scale(width, height)".length);
  expect(text()).toBe("scale(width, height)\n");
});

it("resolves only the selected item, cancels the one left behind, and draws its docs safely (⑤)", async () => {
  serveCompletion(() => ({ items: ["a1", "b2", "c3", "d4"].map((l) => item(l)) }));
  const resolves: { label: string; signal?: AbortSignal }[] = [];
  host.lsp!.handle("lsp.completion.resolve", (params, signal) => {
    const label = ((params as { item: { label: string } }).item as { label: string }).label;
    resolves.push({ label, signal });
    return new Promise((resolve, reject) => {
      signal?.addEventListener("abort", () => reject(new Error("aborted")));
      if (label === "c3")
        resolve({
          server: "ra",
          staleVersion: false,
          item: item("c3", {
            documentation: {
              markdown: true,
              value:
                '**c** docs <script>window.pwed = 1</script> <img src=x onerror="window.pwed = 1"> [link](javascript:alert(1))',
            },
          }),
        });
    });
  });
  await openMain("\n", 0, true);
  await optionSpace();
  await afterInteractionDelay();
  await press("ArrowDown");
  await press("ArrowDown");
  await settle();
  expect(resolves.map((r) => r.label)).toEqual(["a1", "b2", "c3"]);
  expect(resolves.map((r) => r.signal?.aborted)).toEqual([true, true, false]);
  const info = view().dom.querySelector<HTMLElement>(".cm-completionInfo")!;
  expect(info.querySelector("strong")?.textContent).toBe("c");
  expect(info.querySelector("script, img")).toBeNull();
  expect((window as { pwed?: number }).pwed).toBeUndefined();
  await act(async () => info.querySelector<HTMLElement>("a")?.click());
  expect(host.opened).toEqual([]);
});

it("keeps Enter for the Korean IME while composing and opens no list for composed text (⑥)", async () => {
  const asked = serveCompletion(() => ({ items: [item("area"), item("width")] }));
  await openMain("r.\n", 2);
  await compose("compositionstart");
  await composeText(2, 2, "ㅎ");
  await composeText(2, 3, "하");
  await settle();
  expect(asked).toEqual([]);
  expect(listOpen()).toBe(false);
  await compose("compositionend");
  await settle();
  await act(async () => view().dispatch({ changes: { from: 2, to: 3 }, selection: { anchor: 2 } }));
  await optionSpace();
  await afterInteractionDelay();
  expect(listOpen()).toBe(true);
  await compose("compositionstart");
  await composeText(2, 2, "한");
  await press("Enter", { isComposing: true, keyCode: 229 });
  expect(text()).not.toContain("area");
  await compose("compositionend");
});

it("moves the active parameter at every comma and closes outside the call (⑦)", async () => {
  serveCompletion(() => ({ items: [] }));
  const label = "scale(width: u32, height: u32)";
  host.lsp!.handle("lsp.signatureHelp", async (params) => {
    const p = params as { docId: string; offset: number };
    const doc = host.lsp!.docs.get(p.docId)!;
    const call = doc.text.slice(doc.text.lastIndexOf("(", p.offset - 1) + 1, p.offset);
    const inside = doc.text.lastIndexOf("(", p.offset - 1) >= 0 && !call.includes(")");
    return {
      server: "ra",
      staleVersion: host.lsp!.stale({
        docId: p.docId,
        version: (params as { version: number }).version,
      }),
      help: inside
        ? {
            signatures: [
              {
                label,
                documentation: null,
                parameters: [
                  { from: 6, to: 16, documentation: null },
                  { from: 18, to: 29, documentation: { markdown: false, value: "높이" } },
                ],
                activeParameter: call.split(",").length - 1,
              },
            ],
            activeSignature: 0,
            raw: {},
          }
        : null,
    } satisfies LspSignatureHelpAnswer;
  });
  await openMain("scale\n", 5);
  const active = () =>
    view().dom.querySelector(".pw-lsp-popup[data-popup-kind=signature] .pw-lsp-signature-active")
      ?.textContent;
  await type("(");
  expect(active()).toBe("width: u32");
  await type("1");
  expect(active()).toBe("width: u32");
  await type(",");
  expect(active()).toBe("height: u32");
  expect(view().dom.querySelector(".pw-lsp-signature-doc")?.textContent).toBe("높이");
  await type(" 2)");
  expect(currentPopup(view(), "signature")).toBeNull();
  // ⌘P 로 다시 부른다.
  await act(async () => view().dispatch({ selection: { anchor: "scale(1, ".length } }));
  await keydown({ key: "p", code: "KeyP", metaKey: true }, view().contentDOM);
  expect(active()).toBe("height: u32");
});

it("keeps the answer to a completion asked right after an edit that reaches the Host late (M4-07 H-1)", async () => {
  host.lsp = fakeLspServer({ changeDelay: 5 });
  const asked = serveCompletion(() => ({ items: [item("area")] }));
  await openMain("r\n", 1);
  await type(".");
  await settle();
  expect(asked).toHaveLength(1);
  expect(listLabels()).toEqual(["area"]);
});

it("applies a chosen item at every caret with the same word range (multi-cursor)", async () => {
  serveCompletion(() => ({ items: [item("area")] }));
  await openMain("r.\nr.\n", 2);
  await act(async () =>
    view().dispatch({
      selection: EditorSelection.create([EditorSelection.cursor(2), EditorSelection.cursor(5)]),
    }),
  );
  await optionSpace();
  await afterInteractionDelay();
  await press("Enter");
  expect(text()).toBe("r.area\nr.area\n");
});

it("runs the accepted item's command on the Host, and a parameter-hint command opens the signature", async () => {
  const executed: unknown[] = [];
  host.lsp!.handle("lsp.completion.execute", async (params) => {
    executed.push(params);
    return { server: "ra", result: null };
  });
  const hints: unknown[] = [];
  host.lsp!.handle("lsp.signatureHelp", async (params) => {
    hints.push(params);
    return { server: "ra", staleVersion: false, help: null };
  });
  const command = { title: "import", command: "fake.applyImport", arguments: [1] };
  serveCompletion(() => ({
    items: [
      item("area", { command }),
      item("scale", { command: { title: "", command: "editor.action.triggerParameterHints" } }),
    ],
  }));
  await openMain("r.\nq.\n", 2);
  await optionSpace();
  await afterInteractionDelay();
  // 고르기만 해서는 부르지 않는다.
  await press("ArrowDown");
  await press("ArrowUp");
  expect(executed).toEqual([]);
  await press("Enter");
  expect(executed).toEqual([{ docId: "d1", version: 0, command }]);
  // 둘째 줄의 `q.` 뒤(`r.area\n` 다음)에서 다시 연다.
  await act(async () => view().dispatch({ selection: { anchor: "r.area\nq.".length } }));
  await optionSpace();
  await afterInteractionDelay();
  await press("ArrowDown");
  await press("Enter");
  expect(hints).toHaveLength(1);
  expect(executed).toHaveLength(1);
});

it("does nothing and reports nothing when no server completes the document (untrusted · no server)", async () => {
  // 창 틀의 다른 Host 메서드가 없다는 로그는 이 시험과 무관하다 — LSP 의 것(`[lsp]`)만 본다.
  const errors = vi.spyOn(console, "error");
  const warnings = vi.spyOn(console, "warn");
  const lspLogs = () =>
    [...errors.mock.calls, ...warnings.mock.calls].filter((c) => String(c[0]).includes("[lsp]"));
  const asked = serveCompletion(() => ({ items: [item("area")] }));
  host.files.set(MAIN, "r\n");
  host.lsp!.setFeatures({});
  await mount();
  await open(MAIN, 1, 1);
  await act(async () => {
    view().focus();
    view().dispatch({ selection: { anchor: 1 } });
  });
  expect(lspContext()?.capabilities).toEqual({});
  await type(".a(");
  await optionSpace();
  await keydown({ key: "p", code: "KeyP", metaKey: true }, view().contentDOM);
  expect(asked).toEqual([]);
  expect(host.lsp!.called("call:lsp.signatureHelp")).toEqual([]);
  expect(listOpen()).toBe(false);
  expect(text()).toBe("r.a(\n");
  expect(lspLogs()).toEqual([]);
});

it("closes the list with Esc before the signature popup", async () => {
  serveCompletion(() => ({ items: [item("width"), item("height")] }));
  host.lsp!.handle("lsp.signatureHelp", async () => ({
    server: "ra",
    staleVersion: false,
    help: {
      signatures: [
        {
          label: "f(a)",
          documentation: null,
          parameters: [{ from: 2, to: 3, documentation: null }],
          activeParameter: 0,
        },
      ],
      activeSignature: 0,
      raw: {},
    },
  }));
  await openMain("f\n", 1);
  await type("(");
  expect(currentPopup(view(), "signature")).not.toBeNull();
  await optionSpace();
  expect(listOpen()).toBe(true);
  await press("Escape");
  expect(listOpen()).toBe(false);
  expect(currentPopup(view(), "signature")).not.toBeNull();
  await press("Escape");
  expect(currentPopup(view(), "signature")).toBeNull();
});

it("switches between overloads with ↑ ↓ while the caret stays", async () => {
  serveCompletion(() => ({ items: [] }));
  const signature = (label: string) => ({
    label,
    documentation: null,
    parameters: [{ from: 2, to: 3, documentation: null }],
    activeParameter: 0,
  });
  host.lsp!.handle("lsp.signatureHelp", async () => ({
    server: "ra",
    staleVersion: false,
    help: { signatures: [signature("f(a)"), signature("f(b, c)")], activeSignature: 0, raw: {} },
  }));
  await openMain("f\n", 1);
  await type("(");
  const shown = () => view().dom.querySelector(".pw-lsp-signature")?.textContent;
  expect(shown()).toBe("1/2 f(a)");
  await press("ArrowDown");
  expect(shown()).toBe("2/2 f(b, c)");
  await press("ArrowUp");
  expect(shown()).toBe("1/2 f(a)");
  expect(view().state.selection.main.head).toBe(2);
});
