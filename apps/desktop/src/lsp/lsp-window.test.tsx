// @vitest-environment happy-dom
// 웹뷰 LSP 다리(15-m4-plan M4-07)를 프로젝트 창(App kind="project")과 가짜 Host 로 본다 — 탭이 문서를 언어 서버에 여는지(파일 탭의
// 편집 등급 문서만), 한글 조합 중에는 바뀜을 보내지 않고 끝난 뒤 한 번 보내는지(CodeMirror 의 조합 이벤트 · 트랜잭션), 팝업 바탕의
// 액션 칸 · Markdown 링크 · 코드 강조 · 닫힘(편집 · 스크롤 · Esc — 멀티 커서를 접지 않는다), 상태 이벤트 뒤 능력 칸. 창 틀(가짜 Host ·
// 마운트)은 multi-cursor-window.test.tsx 와 같다.
//
// CodeMirror 는 불러올 때 macOS 인지 정한다(navigator.platform) — 그래서 불러오기 전에 Mac 으로 둔다.

import { StateEffect } from "@codemirror/state";
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
import { fakeSyntaxServer, miniHighlight } from "../editor/syntax-testing";
import { fakeLspServer } from "./lsp-testing";
import { currentPopup, openPopup, popupActions, popupField } from "./popup";

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
const ctrl = (code: string, more: KeyboardEventInit = {}) =>
  keydown({ code, key: code.replace(/^Key/, "").toLowerCase(), ctrlKey: true, ...more });

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

const lspContext = () => view().state.facet(editorContext)?.lsp;
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

it("opens a file tab's document on the language server once and closes it with the tab", async () => {
  await mount();
  await open(MAIN, 1, 1);
  expect(host.lsp!.called("open")).toEqual([
    {
      projectId: "p1",
      path: MAIN,
      language: "Rust",
      version: 0,
      text: "fn main() {}\n",
    },
  ]);
  expect(lspContext()?.path).toBe(MAIN);
  // 같은 파일을 다시 열면(같은 탭으로 간다) 다시 열지 않는다.
  await open(MAIN, 1, 3);
  expect(host.lsp!.called("open")).toHaveLength(1);
  await keydown({ code: "KeyW", key: "w", metaKey: true }, view().contentDOM);
  await settle();
  expect(host.lsp!.called("close")).toEqual([{ docId: "d1" }]);
});

it("does not open documents over the edit tier or without a language (④)", async () => {
  await mount();
  await open(BIG, 1, 1);
  await open(NOTES, 1, 1);
  expect(host.lsp!.called("open")).toEqual([]);
  expect(lspContext()).toBeUndefined();
  // 문서가 열리지 않은 탭에는 LSP 기능 칸이 비어 있다(팝업 바탕도 없다).
  expect(view().state.field(popupField, false)).toBeUndefined();
});

it("sends no change while Korean IME composition runs and one change after it ends (⑥)", async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  await mount();
  await open(MAIN, 1, 1);
  view().focus();
  await compose("compositionstart");
  await composeText(0, 0, "ㅎ");
  await composeText(0, 1, "하");
  await composeText(0, 1, "한");
  await settle();
  expect(host.lsp!.called("change")).toEqual([]);
  // 조합 중에는 요청도 묻지 않는다.
  const call = vi.fn(async () => ({ staleVersion: false }));
  expect(await lspContext()!.request("hover", call)).toBeUndefined();
  expect(call).not.toHaveBeenCalled();
  await compose("compositionend");
  await act(async () => vi.advanceTimersByTimeAsync(1));
  await settle();
  expect(host.lsp!.called("change")).toEqual([
    { docId: "d1", version: 1, changes: [{ from: 0, to: 0, insert: "한" }] },
  ]);
  expect(host.lsp!.docs.get("d1")!.text).toBe("한fn main() {}\n");
  // 조합이 아닌 입력은 곧바로 간다.
  await act(async () =>
    view().dispatch({ changes: { from: 0, insert: "x" }, userEvent: "input.type" }),
  );
  await settle();
  expect(host.lsp!.called("change")).toHaveLength(2);
  vi.useRealTimers();
});

it("shows actions plugged into the popup's actions slot on a diagnostic popup only (⑤)", async () => {
  await mount();
  await open(MAIN, 1, 1);
  const ran: string[] = [];
  await act(async () =>
    view().dispatch({
      effects: StateEffect.appendConfig.of(
        popupActions.of({
          kinds: ["diagnostic"],
          actions: (popup) => [
            { id: "quickFix", label: "빠른 수정", run: () => ran.push(String(popup.data)) },
          ],
        }),
      ),
    }),
  );
  await act(async () =>
    openPopup(view(), { kind: "hover", pos: 3, body: { kind: "text", text: "fn main()" } }),
  );
  await settle();
  expect(popupDom()?.textContent).toBe("fn main()");
  expect(popupDom()?.querySelector("[data-popup-action]")).toBeNull();
  await act(async () =>
    openPopup(view(), {
      kind: "diagnostic",
      pos: 3,
      body: { kind: "text", text: "unused variable" },
      data: "E0001",
    }),
  );
  await settle();
  const action = popupDom()!.querySelector<HTMLButtonElement>("[data-popup-action=quickFix]");
  expect(action?.textContent).toBe("빠른 수정");
  await act(async () => action!.click());
  await settle();
  expect(ran).toEqual(["E0001"]);
  expect(popupDom()).toBeNull();
});

it("closes the popup with Esc without folding the cursors, then Esc folds them (M4-14)", async () => {
  await mount();
  await open(WORDS, 1, 1);
  await ctrl("KeyG", { metaKey: true });
  expect(ranges()).toHaveLength(3);
  await act(async () =>
    openPopup(view(), { kind: "hover", pos: 0, body: { kind: "text", text: "foo: int" } }),
  );
  await settle();
  expect(popupDom()).not.toBeNull();
  await keydown({ code: "Escape", key: "Escape" }, view().contentDOM);
  expect(popupDom()).toBeNull();
  expect(ranges()).toHaveLength(3);
  await keydown({ code: "Escape", key: "Escape" }, view().contentDOM);
  expect(ranges()).toHaveLength(1);
});

it("closes the popup on an edit and on a scroll", async () => {
  await mount();
  await open(MAIN, 1, 1);
  const show = () =>
    act(async () =>
      openPopup(view(), { kind: "hover", pos: 3, body: { kind: "text", text: "docs" } }),
    );
  await show();
  await settle();
  expect(currentPopup(view())).not.toBeNull();
  await act(async () => view().dispatch({ changes: { from: 0, insert: " " } }));
  expect(currentPopup(view())).toBeNull();
  await show();
  await act(async () => {
    view().scrollDOM.dispatchEvent(new Event("scroll"));
  });
  expect(currentPopup(view())).toBeNull();
});

it("renders Markdown safely, opens only web links outside and colours code blocks with the syntax service", async () => {
  host.syntax = fakeSyntaxServer();
  await mount();
  await open(MAIN, 1, 1);
  await act(async () =>
    openPopup(view(), {
      kind: "hover",
      pos: 3,
      body: {
        kind: "markdown",
        markdown: [
          "<img src=x onerror=alert(1)> **main**",
          "",
          "```",
          'fn main() { let s = "x"; }',
          "```",
          "",
          "[docs](https://doc.rust-lang.org/std/) [bad](javascript:alert(1)) [file](../etc/passwd)",
        ].join("\n"),
        language: "Mini",
      },
    }),
  );
  await settle();
  const dom = popupDom()!;
  expect(dom.querySelector("img")).toBeNull();
  expect(dom.textContent).toContain("<img src=x onerror=alert(1)>");
  expect(dom.querySelector("strong")?.textContent).toBe("main");
  const tokens = [...dom.querySelectorAll("pre code span")].map((s) => [
    s.className,
    s.textContent,
  ]);
  expect(tokens).toEqual(
    miniHighlight('fn main() { let s = "x"; }').map((s) => [
      `pw-syntax-${s.token}`,
      'fn main() { let s = "x"; }'.slice(s.from, s.to),
    ]),
  );
  for (const link of dom.querySelectorAll("a"))
    await act(async () => link.dispatchEvent(new MouseEvent("click", { bubbles: true })));
  expect(host.opened).toEqual(["https://doc.rust-lang.org/std/"]);
});

it("reads the server capabilities into the tab's context and asks again after an lsp.status event", async () => {
  await mount();
  await open(MAIN, 1, 1);
  expect(lspContext()?.capabilities).toEqual({});
  host.lsp!.setFeatures({
    hover: { server: "rust-analyzer", options: true, optionsTruncated: false },
  });
  await emit("lsp.status", { projectId: "p1", instances: [] });
  expect(lspContext()?.capabilities).toEqual({
    hover: { server: "rust-analyzer", options: true, optionsTruncated: false },
  });
});
