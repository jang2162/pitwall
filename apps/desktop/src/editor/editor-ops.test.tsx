// @vitest-environment happy-dom
// 에디터 조작(12-m1-plan M1-18 완료 기준)을 프로젝트 창(App kind="project")과 가짜 Host 로 본다 — 키는 창의 키맵 엔진(기본 키맵)을
// 지나고, 에디터는 실제 CodeMirror 다: ⌘L `120:5` → 커서, ⌘F 정규식의 일치 수, ⌘/ 의 언어별 줄 주석, 상태바 언어 바꾸기와 탭을
// 닫았다 열면 판별값(구문 강조가 꺼졌다 돌아옴), `font_size` 를 바꾸면 같은 뷰가 바뀜, 한글 조합 중에는 단축키가 돌지 않음, macOS ⌘⌫ · Win Ctrl+Y 줄 삭제,
// undo · redo 가 키와 Win/Linux Edit 메뉴(키맵 액션 항목)에서 한 번씩 돌고 찾기 칸의 키는 칸에 둠.
//
// CodeMirror 는 불러올 때 macOS 인지 정한다(navigator.platform) — 그래서 불러오기 전에 Mac 으로 둔다. ⌘⌫ 가 CodeMirror 의 「줄 앞까지
// 지우기」 와 창의 줄 삭제로 두 번 돌지 않는지가 그때만 보인다(extensions/navigation.ts).

import { SearchQuery } from "@codemirror/search";
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
import type { DockviewApi } from "dockview-react";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../App";
import type { KeymapNative } from "../keymap/native";
import { activeWorktreeStore } from "../open-with/active-worktree";
import { projectOpenInbox } from "../project-open";
import { createThemeStore } from "../theme/theme-store";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";
import { cursorOf } from "./editor-workspace";
import { editorContext } from "./extensions";
import { getDocLanguage, PLAIN_TEXT } from "./language";
import { fakeSyntaxServer, miniHighlight } from "./syntax-testing";

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
              sizeClass: "editable",
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
                sizeClass: "editable",
                editable: true,
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

async function chooseMenu(action: string) {
  await act(async () => menu.choose!(action));
  await settle();
}

const RS = "/work/demo/src/lib.rs";
const PY = "/work/demo/tool.py";

function lines(n: number): string {
  return Array.from({ length: n }, (_, i) => `line ${i + 1} — some text`).join("\n");
}

let root: Root | undefined;
let dock: DockviewApi | undefined;

function useUserAgent(ua: string) {
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(ua);
}

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  useUserAgent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)");
  vi.spyOn(console, "info").mockImplementation(() => {});
  window.history.replaceState(null, "", "/?window=project&project=p1");
  host.files = new Map([
    [RS, "fn main() {}\nfn  helper(x: u32) {}\nlet fnord = 1;\n" + lines(200)],
    [PY, "x = 1\ny = 2\n"],
  ]);
  host.settings = {};
  host.subscribers = [];
  host.syntax = undefined;
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

afterEach(() => {
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
        <App kind="project" native={native} onDockApi={(api) => (dock = api)} />
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
const statusText = (id: string) =>
  document.querySelector(`[data-status-item=${id}]`)?.textContent ?? "";

/** React 가 듣는 입력 칸에 글자를 넣는다. */
async function typeInto(input: HTMLInputElement, text: string) {
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, text);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await settle();
}

async function click(el: Element) {
  await act(async () => (el as HTMLElement).click());
  await settle();
}

async function emitHost(topic: string) {
  await act(async () => {
    for (const s of host.subscribers)
      if (s.projectId === undefined && s.topics.includes(topic))
        s.send({
          method: "events.event",
          params: { subscriptionId: s.id, projectId: null, topic, payload: {} },
        });
  });
  await settle();
}

it("moves the cursor to line:column with ⌘L, and the status bar shows it", async () => {
  await mount();
  await open(RS, 3, 2);
  expect(statusText("cursor")).toBe("3:2");
  await keydown({ code: "KeyL", key: "l", metaKey: true });
  const input = document.querySelector<HTMLInputElement>("[data-testid=goto-line-dialog] input")!;
  expect(input.value).toBe("3:2");
  await typeInto(input, "120:5");
  await keydown({ code: "Enter", key: "Enter" }, input);
  expect(document.querySelector("[data-testid=goto-line-dialog]")).toBeNull();
  expect(cursorOf(view())).toEqual({ line: 120, column: 5 });
  expect(view().hasFocus).toBe(true);
  expect(statusText("cursor")).toBe("120:5");

  // 틀린 입력은 닫지 않고 알린다.
  await keydown({ code: "KeyL", key: "l", metaKey: true });
  const again = document.querySelector<HTMLInputElement>("[data-testid=goto-line-dialog] input")!;
  await typeInto(again, "abc");
  await keydown({ code: "Enter", key: "Enter" }, again);
  expect(document.querySelector("[data-testid=goto-line-dialog] [role=alert]")).not.toBeNull();
  expect(cursorOf(view())).toEqual({ line: 120, column: 5 });
});

it("shows the number of regex matches for ⌘F, and ⌘⌫ in the find field edits the field, not the file", async () => {
  await mount();
  await open(RS, 1, 1);
  const before = view().state.doc.toString();
  await keydown({ code: "KeyF", key: "f", metaKey: true });
  const field = document.querySelector<HTMLInputElement>("[main-field]")!;
  expect(document.activeElement).toBe(field);
  await typeInto(field, String.raw`fn\s+\w+`);
  const count = () => document.querySelector("[data-testid=find-count]")!.textContent;
  expect(count()).toBe("결과 없음");
  await click(document.querySelector("button[aria-label=정규식]")!);
  expect(count()).toBe("2개");
  await keydown({ code: "Enter", key: "Enter" }, field);
  expect(count()).toBe("1/2");
  await keydown({ code: "Enter", key: "Enter" }, field);
  expect(count()).toBe("2/2");
  await keydown({ code: "Enter", key: "Enter", shiftKey: true }, field);
  expect(count()).toBe("1/2");

  // 찾기 칸의 ⌘⌫ 는 편집 키(editorFocus)가 아니다 — 파일의 줄이 남는다.
  await keydown({ code: "Backspace", key: "Backspace", metaKey: true }, field);
  expect(view().state.doc.toString()).toBe(before);

  // ⌘R 는 같은 막대에 바꾸기 줄을 연다. 모두 바꾸기.
  await keydown({ code: "KeyR", key: "r", metaKey: true }, field);
  const replace = document.querySelector<HTMLInputElement>("input[aria-label='바꿀 내용']")!;
  await typeInto(replace, "fn X");
  await click(
    [...document.querySelectorAll("button")].find((b) => b.textContent === "모두 바꾸기")!,
  );
  expect(view().state.doc.toString().split("\n").slice(0, 3)).toEqual([
    "fn X() {}",
    "fn X(x: u32) {}",
    "let fnord = 1;",
  ]);
});

// 막대는 트랜잭션마다 다시 그려진다. 커서만 움직인 것으로 문서 전체를 다시 세면 5 MiB 문서에서 커서 한 칸에 수백 ms 다 — 센 목록에서
// 현재 일치를 찾아야 한다. 문서가 바뀌면 다시 센다.
it("does not count the document again when only the selection moves", async () => {
  await mount();
  await open(RS, 1, 1);
  await keydown({ code: "KeyF", key: "f", metaKey: true });
  const field = document.querySelector<HTMLInputElement>("[main-field]")!;
  await typeInto(field, "fn");
  const count = () => document.querySelector("[data-testid=find-count]")!.textContent;
  expect(count()).toBe("3개");
  const getCursor = vi.spyOn(SearchQuery.prototype, "getCursor");
  const second = view().state.doc.line(2).from;
  await act(async () => view().dispatch({ selection: { anchor: second, head: second + 2 } }));
  await settle();
  expect(count()).toBe("2/3");
  await act(async () => view().dispatch({ selection: { anchor: 0 } }));
  await settle();
  expect(count()).toBe("3개");
  expect(getCursor).not.toHaveBeenCalled();
  await act(async () => view().dispatch({ changes: { from: 0, insert: "fn " } }));
  await settle();
  expect(getCursor).toHaveBeenCalled();
  expect(count()).toBe("4개");
});

it("toggles a line comment with the language's token: // for Rust, # for Python", async () => {
  await mount();
  await open(RS, 1, 1);
  await keydown({ code: "Slash", key: "/", metaKey: true });
  expect(view().state.doc.line(1).text).toBe("// fn main() {}");
  expect(cursorOf(view()).line).toBe(2);

  await open(PY, 1, 1);
  await keydown({ code: "Slash", key: "/", metaKey: true });
  expect(view().state.doc.line(1).text).toBe("# x = 1");
});

it("changes the document's language from the status bar, and forgets it when the tab closes", async () => {
  await mount();
  await open(PY, 1, 1);
  expect(statusText("language")).toBe("Python");
  await click(document.querySelector("[data-testid=status-language]")!);
  const plain = [...document.querySelectorAll("[cmdk-item]")].find(
    (i) => i.textContent === "일반 텍스트",
  )!;
  await act(async () => plain.dispatchEvent(new MouseEvent("click", { bubbles: true })));
  await settle();
  expect(getDocLanguage(PY)).toBe(PLAIN_TEXT);
  expect(statusText("language")).toBe("일반 텍스트");
  // 플레인 텍스트에는 줄 주석이 없다.
  await keydown({ code: "Slash", key: "/", metaKey: true });
  expect(view().state.doc.line(1).text).toBe("x = 1");

  await act(async () => dock!.activePanel!.api.close());
  await settle();
  await open(PY, 1, 1);
  expect(getDocLanguage(PY)).toBe("Python");
  expect(statusText("language")).toBe("Python");
});

it("turns highlighting off when the status bar picks Plain Text, and back on with the detected language after the tab reopens", async () => {
  host.syntax = fakeSyntaxServer({ grammars: { Rust: miniHighlight } });
  const syntax = host.syntax;
  const highlighted = () => document.querySelector(".dv-active-group [class*=pw-syntax-]");
  await mount();
  await open(RS, 1, 1);
  await vi.waitFor(() => expect(highlighted()).not.toBeNull());
  expect(syntax.docs.size).toBe(1);

  await click(document.querySelector("[data-testid=status-language]")!);
  const plain = [...document.querySelectorAll("[cmdk-item]")].find(
    (i) => i.textContent === "일반 텍스트",
  )!;
  await act(async () => plain.dispatchEvent(new MouseEvent("click", { bubbles: true })));
  await settle();
  expect(statusText("language")).toBe("일반 텍스트");
  expect(view().state.facet(editorContext)?.language).toBe(PLAIN_TEXT);
  await vi.waitFor(() => expect(highlighted()).toBeNull());
  await vi.waitFor(() => expect(syntax.docs.size).toBe(0));

  await act(async () => dock!.activePanel!.api.close());
  await settle();
  await open(RS, 1, 1);
  expect(statusText("language")).toBe("Rust");
  expect(view().state.facet(editorContext)?.language).toBe("Rust");
  await vi.waitFor(() => expect(highlighted()).not.toBeNull());
  expect(syntax.docs.size).toBe(1);
});

it("shows line ending and encoding of the active tab", async () => {
  await mount();
  // 줄 · 열 없이 연 탭도 처음부터 커서를 보인다(뷰가 생길 때의 알림, extensions/operations.ts).
  await open(RS);
  expect(statusText("cursor")).toBe("1:1");
  expect(statusText("lineEnding")).toBe("LF");
  expect(statusText("encoding")).toBe("UTF-8");
  view().dispatch({ selection: { anchor: 0, head: 15 } });
  await settle();
  expect(statusText("cursor")).toBe("2:3 (15자, 줄바꿈 1개)");
});

it("applies [editor] font_size and font_family from settings.toml to the open editor without a restart", async () => {
  await mount();
  await open(RS, 1, 1);
  const before = view();
  expect(before.state.facet(editorContext)?.font).toEqual({});

  host.settings = { editor: { font_size: 20, font_family: "Iosevka" } };
  await emitHost("config.changed");
  expect(view()).toBe(before);
  expect(before.state.facet(editorContext)?.font).toEqual({ size: 20, family: "Iosevka" });
  const css = [...document.querySelectorAll("style")].map((s) => s.textContent).join("\n");
  expect(css).toContain("font-size: 20px");
  expect(css).toContain("font-family: Iosevka");

  // 범위 밖 크기 · 빈 글꼴은 기본값.
  host.settings = { editor: { font_size: 500, font_family: " " } };
  await emitHost("config.changed");
  expect(before.state.facet(editorContext)?.font).toEqual({});
});

it("does not run shortcuts while an IME is composing", async () => {
  await mount();
  await open(PY, 1, 1);
  await keydown({ code: "KeyD", key: "ㅇ", metaKey: true, isComposing: true });
  expect(view().state.doc.toString()).toBe("x = 1\ny = 2\n");
  await keydown({ code: "KeyD", key: "ㅇ", metaKey: true });
  expect(view().state.doc.toString()).toBe("x = 1\nx = 1\ny = 2\n");
});

it("deletes the line once with ⌘⌫ on macOS and keeps ⌥⌫ as delete-word", async () => {
  await mount();
  await open(PY, 2, 4);
  // 한 번만 고친다 — CodeMirror 의 「줄 앞까지 지우기」 가 먼저 돌아도 끝 텍스트는 같으므로 고친 횟수를 센다.
  const dispatch = vi.spyOn(view(), "dispatch");
  await keydown({ code: "Backspace", key: "Backspace", metaKey: true });
  expect(view().state.doc.toString()).toBe("x = 1\n");
  const edits = dispatch.mock.calls.filter((args) =>
    args.some((a) => (a as { docChanged?: boolean }).docChanged || "changes" in (a as object)),
  );
  expect(edits).toHaveLength(1);
  view().dispatch({ selection: { anchor: 5 } });
  await keydown({ code: "Backspace", key: "Backspace", altKey: true });
  expect(view().state.doc.toString()).toBe("x = \n");
});

it("moves and duplicates lines with ⌥⇧↑↓ and ⌘D", async () => {
  await mount();
  await open(PY, 2, 1);
  await keydown({ code: "ArrowUp", key: "ArrowUp", altKey: true, shiftKey: true });
  expect(view().state.doc.toString()).toBe("y = 2\nx = 1\n");
  await keydown({ code: "ArrowDown", key: "ArrowDown", altKey: true, shiftKey: true });
  expect(view().state.doc.toString()).toBe("x = 1\ny = 2\n");
  await keydown({ code: "KeyD", key: "d", metaKey: true });
  expect(view().state.doc.toString()).toBe("x = 1\ny = 2\ny = 2\n");
});

it("on Windows, Ctrl+Y deletes the line instead of redoing", async () => {
  useUserAgent("Mozilla/5.0 (Windows NT 10.0; Win64; x64)");
  await mount();
  await open(PY, 1, 1);
  await keydown({ code: "KeyY", key: "y", ctrlKey: true });
  expect(view().state.doc.toString()).toBe("y = 2\n");
});

// Win/Linux 의 Edit 메뉴 「실행 복귀」 는 키맵 액션 `editor.redo` 다(src-tauri/src/menu.rs). muda 의 기본 항목은 Ctrl+Y 로 고정이고
// Windows 에서 고르면 그 키를 다시 보내 줄 삭제(Ctrl+Y)가 됐다.
it("on Windows, the Edit menu undoes and redoes through the keymap, showing Ctrl+Z and Ctrl+Shift+Z", async () => {
  useUserAgent("Mozilla/5.0 (Windows NT 10.0; Win64; x64)");
  await mount();
  await open(PY, 1, 1);
  expect(menu.accelerators["editor.undo"]).toBe("Control+KeyZ");
  expect(menu.accelerators["editor.redo"]).toBe("Control+Shift+KeyZ");
  await keydown({ code: "ArrowDown", key: "ArrowDown", altKey: true, shiftKey: true });
  expect(view().state.doc.toString()).toBe("y = 2\nx = 1\n");
  await chooseMenu("editor.undo");
  expect(view().state.doc.toString()).toBe("x = 1\ny = 2\n");
  await chooseMenu("editor.redo");
  expect(view().state.doc.toString()).toBe("y = 2\nx = 1\n");
});

it("undoes and redoes one step per key press, by ⌘Z and ⌘⇧Z", async () => {
  await mount();
  await open(PY, 1, 1);
  await keydown({ code: "KeyD", key: "d", metaKey: true });
  await keydown({ code: "KeyD", key: "d", metaKey: true });
  expect(view().state.doc.toString()).toBe("x = 1\nx = 1\nx = 1\ny = 2\n");
  await keydown({ code: "KeyZ", key: "z", metaKey: true });
  expect(view().state.doc.toString()).toBe("x = 1\nx = 1\ny = 2\n");
  await keydown({ code: "KeyZ", key: "z", metaKey: true, shiftKey: true });
  expect(view().state.doc.toString()).toBe("x = 1\nx = 1\nx = 1\ny = 2\n");
});

it("leaves Ctrl+Z in the find field to the field, and the menu's Undo does not undo the editor behind it", async () => {
  useUserAgent("Mozilla/5.0 (Windows NT 10.0; Win64; x64)");
  await mount();
  await open(PY, 1, 1);
  await keydown({ code: "ArrowDown", key: "ArrowDown", altKey: true, shiftKey: true });
  await keydown({ code: "KeyF", key: "f", ctrlKey: true });
  const input = document.querySelector<HTMLInputElement>(".cm-panels input")!;
  expect(document.activeElement).toBe(input);
  const event = new KeyboardEvent("keydown", {
    code: "KeyZ",
    key: "z",
    ctrlKey: true,
    bubbles: true,
    cancelable: true,
  });
  await act(async () => input.dispatchEvent(event));
  expect(event.defaultPrevented).toBe(false);
  await chooseMenu("editor.undo");
  expect(view().state.doc.toString()).toBe("y = 2\nx = 1\n");
});
