// @vitest-environment happy-dom
// 프로젝트 창(App kind="project", `?project=p1`)을 가짜 Host 에 붙여 에디터 탭을 본다(12-m1-plan M1-09 완료 기준): 열기 요청이 탭과
// 커서 자리가 되고, 같은 파일은 그 탭을 다시 쓰고, ⌘⌥E 가 활성 탭의 파일 · 줄 · 열을 openWith.run 에 넘기고, ⌘\ 로 나눈 두 그룹이
// 스크롤을 따로 갖고, 탭 단축키(⌘W · ⌘⌥W · ⌃Tab)가 돈다. 편집 · 저장 · 외부 변경(M1-13 완료 기준 ①~⑤)도 여기서 본다.

import { EditorView } from "@codemirror/view";
import type {
  ConfigView,
  FsRead,
  FsStat,
  FsWriteParams,
  FsWritten,
  HostInfo,
  KeymapFile,
  LangDetectResult,
  OpenWithOutcome,
  Project,
  ProjectTrust,
} from "@pitwall/rpc-client";
import type { DockviewApi } from "dockview-react";
import { act, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../App";
import type { KeymapNative } from "../keymap/native";
import { LAYOUT_STORAGE_KEY } from "../layout/project-layout";
import { resetWindowFileTree } from "../file-tree/project-view";
import { activeWorktreeStore } from "../open-with/active-worktree";
import { projectOpenInbox } from "../project-open";
import { createThemeStore } from "../theme/theme-store";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";
import { EDITOR_PANEL } from "./editor-tabs";
import { cursorOf } from "./editor-workspace";
import { editorContext } from "./extensions";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const MIB = 1024 * 1024;

const host = vi.hoisted(() => ({
  /** 정규화한 경로 → 내용. */
  files: new Map<string, string>(),
  /** 요청 경로 → 정규화한 경로(심링크). */
  links: new Map<string, string>(),
  /** 정규화한 경로 → 크기(내용 길이 대신). */
  sizes: new Map<string, number>(),
  /** 정규화한 경로 → 수정 시각. `version` 은 수정 시각과 내용으로 정한다(pw-fs 와 같은 뜻). */
  mtimes: new Map<string, number>(),
  calls: [] as { method: string; params: Record<string, unknown> }[],
  /** 열린 연결의 이벤트 구독: 이벤트를 보내는 함수와 구독 id · 토픽. */
  subscribers: [] as { send(m: object): void; id: number; topics: string[] }[],
  /** 프로젝트 id → 저장된 프로젝트 상태(`project.state.*`, `$DATA/projects/<id>/state.json`). */
  states: new Map<string, unknown>(),
}));

/** 디스크의 파일을 바꾼다(외부 도구 · 에이전트). 감시 이벤트는 따로 `emit` 한다. */
function putFile(path: string, content: string) {
  host.files.set(path, content);
  host.mtimes.set(path, (host.mtimes.get(path) ?? 1) + 1);
}

const versionOf = (path: string) =>
  host.files.has(path) ? `${host.mtimes.get(path) ?? 1}:${host.files.get(path)}` : null;

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
}

const changed = (paths: string[], kinds: string[] = paths.map(() => "modified")) =>
  emit("fs.changed", { root: "/work/demo", worktree: null, paths, kinds, rescan: false });

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
        const fail = (message: string) =>
          send({ id: m.id, error: { code: -32005, message, data: null } });
        if (["fs.", "openWith.", "lang."].some((p) => m.method.startsWith(p)))
          // 인자 없는 메서드(`lang.list`)는 params 가 null 이다.
          host.calls.push({ method: m.method, params: m.params ?? {} });
        const path = (m.params as { path?: string } | undefined)?.path ?? "";
        const real = host.links.get(path) ?? path;
        const size = () => host.sizes.get(real) ?? host.files.get(real)!.length;
        const sizeClass = () =>
          size() > 50 * 1024 * 1024
            ? "tooLarge"
            : size() > 5 * 1024 * 1024
              ? "readOnly"
              : "editable";
        switch (m.method) {
          case "i18n.get":
            return reply(i18nState("ko"));
          case "config.get":
            return reply({
              snapshot: { files: [], notices: [] },
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
          case "project.state.get": {
            const { projectId } = m.params as { projectId: string };
            return reply({ state: host.states.get(projectId) ?? null, problem: null });
          }
          case "project.state.set": {
            const { projectId, state } = m.params as { projectId: string; state: unknown };
            host.states.set(projectId, structuredClone(state));
            return reply(null);
          }
          case "project.claimTrustPrompt":
            return reply(false);
          case "fs.stat":
            if (!host.files.has(real)) return fail(`not found: ${path}`);
            return reply({
              path: real,
              kind: host.links.has(path) ? "symlink" : "file",
              target: host.links.has(path) ? "file" : null,
              size: size(),
              mtimeMs: host.mtimes.get(real) ?? 1,
              readonly: false,
              sizeClass: sizeClass(),
            } satisfies FsStat);
          case "fs.read":
            if (!host.files.has(real)) return fail(`not found: ${path}`);
            return reply({
              path: real,
              size: size(),
              mtimeMs: host.mtimes.get(real) ?? 1,
              offset: 0,
              eof: true,
              version: versionOf(real),
              content: { kind: "text", text: host.files.get(real)! },
              judgment: {
                binary: false,
                encoding: "UTF-8",
                lossy: false,
                bom: null,
                lineEnding: host.files.get(real)!.includes("\r\n") ? "crlf" : "lf",
                longLines: false,
                sizeClass: sizeClass(),
                editable: sizeClass() === "editable",
                highlight: sizeClass() === "editable",
              },
            } satisfies FsRead);
          case "lang.detect": {
            const { files } = m.params as { files: { path: string }[] };
            return reply({
              files: files.map((f) => ({
                language: f.path.endsWith(".rs") ? "Rust" : null,
                source: "extension",
                vendored: false,
                generated: false,
                documentation: false,
              })),
            } satisfies LangDetectResult);
          }
          case "openWith.run":
            return reply({
              kind: "launched",
              tool: "vscode",
              label: "Visual Studio Code",
              argv: ["/usr/local/bin/code"],
              pid: 1,
            } satisfies OpenWithOutcome);
          case "fs.write": {
            const p = m.params as FsWriteParams;
            if (versionOf(p.path) !== (p.expectedVersion ?? null))
              return send({ id: m.id, error: { code: -32004, message: "conflict", data: null } });
            if (p.content.kind !== "text") return fail("text only");
            host.files.set(p.path, p.content.text);
            host.mtimes.set(p.path, (host.mtimes.get(p.path) ?? 1) + 1);
            return reply({
              path: p.path,
              size: p.content.text.length,
              mtimeMs: host.mtimes.get(p.path)!,
              version: versionOf(p.path)!,
            } satisfies FsWritten);
          }
          case "events.subscribe": {
            const id = next++;
            host.subscribers.push({ send, id, topics: (m.params as { topics: string[] }).topics });
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

/** 네이티브 메뉴에서 고른 액션을 보낸다(메뉴 accelerator 로 온 ⌘W 등). */
let menuListener: ((action: string) => void) | undefined;
const native = {
  syncMenu: async () => {},
  onMenuAction: (listener: (action: string) => void) => {
    menuListener = listener;
    return () => {
      if (menuListener === listener) menuListener = undefined;
    };
  },
  openSettings: async () => {},
  closeWindow: vi.fn<KeymapNative["closeWindow"]>(async () => {}),
  guardClose: (shouldClose: () => Promise<boolean>) => {
    closeGuard = shouldClose;
    return () => {
      if (closeGuard === shouldClose) closeGuard = undefined;
    };
  },
} satisfies KeymapNative;

/** 창의 닫기 요청을 받으면 묻는 함수(`guardClose`). */
let closeGuard: (() => Promise<boolean>) | undefined;

async function menuAction(action: string) {
  await act(async () => menuListener?.(action));
  await settle();
}

const LIB = "/work/demo/src/lib.rs";
const MAIN = "/work/demo/src/main.rs";
const README = "/work/demo/README.txt";

/** `n` 줄짜리 텍스트. 줄마다 그 줄 번호가 든다. */
function lines(n: number): string {
  return Array.from({ length: n }, (_, i) => `line ${i + 1} — some text`).join("\n");
}

let root: Root | undefined;
let themeStore: ReturnType<typeof createThemeStore> | undefined;
let dock: DockviewApi | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
  );
  vi.spyOn(console, "info").mockImplementation(() => {});
  window.history.replaceState(null, "", "/?window=project&project=p1");
  host.files = new Map([
    [LIB, lines(300)],
    [MAIN, lines(20)],
    [README, "hello"],
  ]);
  host.links = new Map([["/work/demo/link.rs", LIB]]);
  host.sizes = new Map();
  host.mtimes = new Map();
  host.calls = [];
  host.subscribers = [];
  host.states = new Map();
  activeWorktreeStore.setState({ top: undefined });
  native.closeWindow.mockClear();
  localStorage.clear();
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
  const store = (themeStore = createThemeStore({
    system: { current: () => "light", subscribe: () => () => {} },
    storage: { read: () => undefined, write: () => {} },
    theme: parseTheme(defaultTheme),
    apply: () => {},
  }));
  root = createRoot(element);
  await act(async () =>
    root!.render(
      <StrictMode>
        <ThemeStoreProvider store={store}>
          <App kind="project" native={native} onDockApi={(api) => (dock = api)} />
        </ThemeStoreProvider>
      </StrictMode>,
    ),
  );
  await settle();
}

/** 창이 받은 열기 요청(`pitwall <파일>:<줄>:<열>`). */
async function openRequest(file: string, line?: number, col?: number, worktree?: string) {
  await act(async () => projectOpenInbox.push({ file, line, col, worktree }));
  await settle();
}

async function keydown(init: KeyboardEventInit) {
  await act(async () => {
    (document.activeElement ?? document.body).dispatchEvent(
      new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }),
    );
  });
  await settle();
}

async function keyup(init: KeyboardEventInit) {
  await act(async () => {
    (document.activeElement ?? document.body).dispatchEvent(
      new KeyboardEvent("keyup", { bubbles: true, cancelable: true, ...init }),
    );
  });
  await settle();
}

async function press(code: string, mods: KeyboardEventInit = {}, key = code) {
  await keydown({ code, key, ...mods });
  await keyup({ code, key });
}

const editorTabs = () =>
  [...document.querySelectorAll<HTMLElement>("[data-editor-tab-header]")].map((e) => ({
    title: e.textContent,
    path: e.title,
    preview: e.dataset.preview !== undefined,
  }));
const activeTabTitle = () =>
  document.querySelector(".dv-active-tab [data-editor-tab-header]")?.textContent ?? undefined;
const views = () =>
  [...document.querySelectorAll<HTMLElement>(".cm-editor")].map((e) => EditorView.findFromDOM(e)!);
const runCalls = () => host.calls.filter((c) => c.method === "openWith.run").map((c) => c.params);

it("opens an open request as a tab with the cursor on that line and column", async () => {
  await mount();
  expect(document.querySelector("[data-testid=editor-empty]")).not.toBeNull();

  await openRequest(LIB, 120, 5);
  expect(editorTabs()).toEqual([{ title: "lib.rs", path: LIB, preview: false }]);
  expect(document.querySelector("[data-testid=editor-empty]")).toBeNull();
  const [view] = views();
  expect(cursorOf(view!)).toEqual({ line: 120, column: 5 });
  expect(view!.state.doc.line(120).text).toBe("line 120 — some text");
  expect(view!.hasFocus).toBe(true);
});

it("gives the view the detected language and reconfigures it on a theme change without a new view", async () => {
  await mount();
  await openRequest(LIB, 4, 1, "/work/demo-wt");
  const detect = host.calls.find((c) => c.method === "lang.detect")!.params;
  expect(detect).toMatchObject({ projectId: "p1", worktree: "/work/demo-wt" });
  expect((detect.files as { path: string; head: string }[])[0]).toMatchObject({ path: LIB });
  const [view] = views();
  expect(view!.state.facet(editorContext)).toMatchObject({ language: "Rust", appearance: "light" });

  await act(async () => themeStore!.getState().setPreference("dark"));
  await settle();
  expect(views()).toEqual([view]);
  expect(view!.state.facet(editorContext)).toMatchObject({ language: "Rust", appearance: "dark" });
  expect(cursorOf(view!)).toEqual({ line: 4, column: 1 });

  // 탭 params 가 바뀌어도(고정) 뷰를 다시 만들지 않는다.
  await runAction("탭 고정 / 고정 해제");
  expect(views()).toEqual([view]);
});

it("reuses the open tab when the same file comes again, even through a symlink", async () => {
  await mount();
  await openRequest(LIB, 120, 5);
  await openRequest(MAIN);
  expect(activeTabTitle()).toBe("main.rs");

  await openRequest("/work/demo/link.rs", 7, 2);
  expect(editorTabs().map((t) => t.title)).toEqual(["lib.rs", "main.rs"]);
  expect(activeTabTitle()).toBe("lib.rs");
  const view = EditorView.findFromDOM(
    document.querySelector<HTMLElement>(".dv-groupview .cm-editor")!,
  )!;
  expect(view.state.doc.line(1).text).toBe("line 1 — some text");
  expect(views().map((v) => cursorOf(v))).toContainEqual({ line: 7, column: 2 });
});

it("passes the active tab's file and cursor to openWith.run on ⌘⌥E", async () => {
  await mount();
  await openRequest(LIB, 120, 5);
  await press("KeyE", { metaKey: true, altKey: true }, "´");
  expect(runCalls()).toEqual([
    { projectId: "p1", tool: null, worktree: null, file: LIB, line: 120, column: 5 },
  ]);
});

it("passes the request's worktree to file reads and to openWith.run", async () => {
  await mount();
  await openRequest(LIB, 2, 1, "/work/demo-wt");
  expect(host.calls.find((c) => c.method === "fs.stat")!.params).toMatchObject({
    projectId: "p1",
    worktree: "/work/demo-wt",
    path: LIB,
  });
  await press("KeyE", { metaKey: true, altKey: true }, "´");
  expect(runCalls()[0]).toMatchObject({ worktree: "/work/demo-wt", file: LIB, line: 2 });
});

it("reads queued open requests in their own worktree, switching the tab set for each", async () => {
  // 리뷰의 재현: 창이 마운트되기 전에 두 워크트리의 요청이 쌓인다. 요청마다 그 워크트리로 바꾼다(M1-25) — 문서도 그 요청의
  // 워크트리로 읽는다.
  const statsOf = (path: string) =>
    host.calls.filter((c) => c.method !== "openWith.run" && c.params.path === path);
  projectOpenInbox.push({ worktree: "/work/wt1", file: LIB, line: 3 });
  projectOpenInbox.push({ worktree: "/work/wt2", file: MAIN });
  await mount();
  expect(editorTabs().map((t) => t.title)).toEqual(["main.rs"]);
  expect(activeWorktreeStore.getState().top).toBe("/work/wt2");
  // StrictMode 는 개발 중에 문서를 두 번 읽는다 — 몇 번이든 모두 그 요청의 워크트리다.
  const scopes = (path: string) =>
    new Set(statsOf(path).map((c) => `${c.method} ${c.params.worktree}`));
  expect(scopes(LIB)).toEqual(new Set(["fs.stat /work/wt1", "fs.read /work/wt1"]));
  expect(scopes(MAIN)).toEqual(new Set(["fs.stat /work/wt2", "fs.read /work/wt2"]));

  // wt1 로 돌아오면 wt1 의 탭(커서 줄 포함)이 돌아오고, ⌘⌥E · 「Finder 에서 보기」 는 그 탭의 워크트리로 나간다.
  await act(async () => projectOpenInbox.push({ worktree: "/work/wt1" }));
  await settle();
  expect(editorTabs().map((t) => t.title)).toEqual(["lib.rs"]);
  expect(cursorOf(views()[0]!).line).toBe(3);
  await press("KeyE", { metaKey: true, altKey: true }, "´");
  await runAction("Finder 에서 보기");
  expect(runCalls().map((c) => [c.tool, c.worktree, c.file])).toEqual([
    [null, "/work/wt1", LIB],
    ["finder", "/work/wt1", LIB],
  ]);
});

it("opens the project without a file when no editor tab is open", async () => {
  await mount();
  await press("KeyE", { metaKey: true, altKey: true }, "´");
  expect(runCalls()).toEqual([{ projectId: "p1", tool: null, worktree: null }]);
});

it("splits the active tab to the right, and the two views of one file scroll on their own", async () => {
  await mount();
  await openRequest(LIB, 1, 1);
  await press("Backslash", { metaKey: true }, "\\");
  expect(editorTabs().map((t) => t.title)).toEqual(["lib.rs", "lib.rs"]);
  const [a, b] = views();
  expect(a).not.toBe(b);
  // 문서는 하나다 — 두 뷰가 같은 텍스트를 나눈다.
  expect(a!.state.doc).toBe(b!.state.doc);
  a!.scrollDOM.scrollTop = 900;
  a!.dispatch({ selection: { anchor: a!.state.doc.line(200).from } });
  expect(b!.scrollDOM.scrollTop).toBe(0);
  expect(cursorOf(b!).line).toBe(1);
});

it("closes the active tab on ⌘W, the other unpinned tabs on ⌘⌥W, and shows the empty area at the end", async () => {
  await mount();
  await openRequest(LIB);
  await openRequest(MAIN);
  await openRequest(README);
  // main.rs 를 고정하고 README 로 돌아간다.
  await openRequest(MAIN);
  await runAction("탭 고정 / 고정 해제");
  await openRequest(README);

  await press("KeyW", { metaKey: true, altKey: true }, "∑");
  expect(editorTabs().map((t) => t.title)).toEqual(["main.rs", "README.txt"]);

  await press("KeyW", { metaKey: true }, "w");
  expect(editorTabs().map((t) => t.title)).toEqual(["main.rs"]);
  await press("KeyW", { metaKey: true }, "w");
  expect(editorTabs()).toEqual([]);
  expect(document.querySelector("[data-testid=editor-empty]")).not.toBeNull();
});

it("closes the window on ⌘W only when no editor tab is left", async () => {
  await mount();
  await openRequest(LIB);
  await press("KeyW", { metaKey: true }, "w");
  expect(editorTabs()).toEqual([]);
  expect(native.closeWindow).not.toHaveBeenCalled();
  await press("KeyW", { metaKey: true }, "w");
  expect(native.closeWindow).toHaveBeenCalledTimes(1);
});

it("closes one tab when ⌘W arrives both as a key and as the menu accelerator", async () => {
  await mount();
  await openRequest(LIB);
  await openRequest(MAIN);
  await keydown({ code: "KeyW", key: "w", metaKey: true });
  await menuAction("window.close");
  expect(editorTabs().map((t) => t.title)).toEqual(["lib.rs"]);
  expect(native.closeWindow).not.toHaveBeenCalled();
});

it("closes the tab, not the window, when ⌘W during IME composition comes only from the menu", async () => {
  await mount();
  await openRequest(LIB);
  await openRequest(MAIN);
  // 키맵은 조합 중인 키를 보지 않는다(packages/keymap engine.ts) — 키로는 아무것도 닫히지 않는다.
  await keydown({ code: "KeyW", key: "w", metaKey: true, isComposing: true });
  expect(editorTabs()).toHaveLength(2);
  await menuAction("window.close");
  expect(editorTabs().map((t) => t.title)).toEqual(["lib.rs"]);
  expect(native.closeWindow).not.toHaveBeenCalled();
});

it("closes nothing on ⌘W while a popup of the window is open", async () => {
  await mount();
  await openRequest(LIB);
  const header = document.querySelector<HTMLElement>("[data-editor-tab-header]")!;
  await act(async () => {
    header.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
  });
  await settle();
  expect(document.querySelector("[role=menu]")).not.toBeNull();
  await keydown({ code: "KeyW", key: "w", metaKey: true });
  await menuAction("window.close");
  expect(editorTabs()).toHaveLength(1);
  expect(native.closeWindow).not.toHaveBeenCalled();
});

it("switches to the previously used tab with ⌃Tab when Control is released", async () => {
  await mount();
  await openRequest(LIB);
  await openRequest(MAIN);
  await openRequest(README);
  expect(activeTabTitle()).toBe("README.txt");

  await keydown({ code: "Tab", key: "Tab", ctrlKey: true });
  expect(document.querySelector("[role=listbox]")?.textContent).toContain("main.rs");
  await keyup({ code: "ControlLeft", key: "Control" });
  expect(document.querySelector("[role=listbox]")).toBeNull();
  expect(activeTabTitle()).toBe("main.rs");

  // ⌃ 를 누른 채 Tab 을 한 번 더 누르면 그다음(가장 오래된) 탭이다.
  await keydown({ code: "Tab", key: "Tab", ctrlKey: true });
  await keydown({ code: "Tab", key: "Tab", ctrlKey: true });
  await keyup({ code: "ControlLeft", key: "Control" });
  expect(activeTabTitle()).toBe("lib.rs");
});

it("says why a file could not be opened", async () => {
  await mount();
  await openRequest("/work/demo/gone.rs", 3);
  expect(editorTabs()).toEqual([]);
  const toasts = [...document.querySelectorAll("[data-sonner-toast]")].map((t) => t.textContent);
  expect(toasts.some((t) => t?.includes("파일을 열지 못했다: /work/demo/gone.rs"))).toBe(true);
});

it("does not read a file larger than 50 MiB and opens a 5 MiB+ file read-only", async () => {
  host.sizes.set(MAIN, 60 * MIB);
  host.sizes.set(LIB, 6 * MIB);
  await mount();
  await openRequest(MAIN);
  expect(document.querySelector("[role=status]")?.textContent).toContain("50 MiB 를 넘는 파일");
  expect(host.calls.filter((c) => c.method === "fs.read")).toEqual([]);

  await openRequest(LIB);
  expect(document.body.textContent).toContain("5 MiB 를 넘는 파일이라 읽기 전용으로 연다");
  expect(views()[0]!.state.readOnly).toBe(true);
});

// ── 프로젝트 · 워크트리 상태 복원(12-m1-plan M1-25, 결정 M1-D19) ──

/** 가짜 Host 에 저장된 프로젝트 상태. */
const savedState = (project = "p1") =>
  host.states.get(project) as
    | {
        layout?: unknown;
        main?: { tabs: { path: string; cursor?: { line: number } }[]; active?: number };
        worktrees?: Record<string, { tabs: { path: string }[] }>;
      }
    | undefined;

/** 앱을 끄고 다시 띄운다(창의 웹뷰가 새로 뜬다). 저장된 것은 가짜 Host 에 남는다. */
async function restart(project = "p1") {
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
  activeWorktreeStore.setState({ top: undefined });
  resetWindowFileTree();
  window.history.replaceState(null, "", `/?window=project&project=${project}`);
  await mount();
}

it("brings tabs back once from the worktree's tab set, never from the saved layout", async () => {
  await mount();
  await openRequest(LIB);
  // 레이아웃 저장본에는 탭이 든다(dockview 의 toJSON 그대로). 읽을 때 뺀다 — 탭은 워크트리 묶음이 되살린다.
  await vi.waitFor(
    () => {
      expect(JSON.stringify(savedState()?.layout)).toContain(`"${EDITOR_PANEL}"`);
      expect(savedState()?.main?.tabs.map((t) => t.path)).toEqual([LIB]);
    },
    { timeout: 2000 },
  );
  await restart();
  expect(editorTabs().map((t) => t.title)).toEqual(["lib.rs"]);
  // 옛 localStorage 레이아웃은 쓰지 않는다.
  expect(localStorage.getItem(LAYOUT_STORAGE_KEY)).toBeNull();
});

it("restores each project's layout and five tabs with the active tab and cursor line after a restart", async () => {
  const files = [1, 2, 3, 4, 5].map((n) => `/work/demo/src/f${n}.rs`);
  for (const f of files) host.files.set(f, lines(50));
  await mount();
  for (const [i, f] of files.entries()) await openRequest(f, 10 + i);
  await openRequest(files[2]!, 33, 4);
  await act(async () => dock!.getPanel("project")!.api.close());
  await settle();
  await vi.waitFor(
    () => {
      expect(savedState()?.main?.tabs).toHaveLength(5);
      expect(savedState()?.main?.active).toBe(2);
      expect(savedState()?.main?.tabs[2]?.cursor?.line).toBe(33);
      expect(JSON.stringify(savedState()?.layout)).toContain('"hidden":{"project"');
    },
    { timeout: 2000 },
  );

  await restart("p2");
  expect(editorTabs()).toEqual([]);
  expect(dock!.getPanel("project")).toBeDefined();
  await openRequest(MAIN, 7);
  await vi.waitFor(() => expect(savedState("p2")?.main?.tabs).toHaveLength(1), { timeout: 2000 });

  await restart("p1");
  expect(editorTabs().map((t) => t.path)).toEqual(files);
  expect(activeTabTitle()).toBe("f3.rs");
  const active = views().find((v) => v.hasFocus) ?? views()[2]!;
  expect(cursorOf(active)).toEqual({ line: 33, column: 4 });
  expect(dock!.getPanel("project")).toBeUndefined();

  await restart("p2");
  expect(editorTabs().map((t) => t.path)).toEqual([MAIN]);
  expect(cursorOf(views()[0]!).line).toBe(7);
  expect(dock!.getPanel("project")).toBeDefined();
});

it("switches to the other worktree's tabs and brings the first worktree's tabs back", async () => {
  await mount();
  await openRequest(LIB, 5, 1, "/work/wt1");
  await openRequest(README, undefined, undefined, "/work/wt1");
  await openRequest(MAIN, 2, 1, "/work/wt2");
  expect(editorTabs().map((t) => t.title)).toEqual(["main.rs"]);

  await act(async () => projectOpenInbox.push({ worktree: "/work/wt1" }));
  await settle();
  expect(editorTabs().map((t) => t.title)).toEqual(["lib.rs", "README.txt"]);
  expect(activeTabTitle()).toBe("README.txt");
  expect(activeWorktreeStore.getState().top).toBe("/work/wt1");

  await act(async () => projectOpenInbox.push({ worktree: "/work/wt2" }));
  await settle();
  expect(editorTabs().map((t) => t.title)).toEqual(["main.rs"]);
  expect(cursorOf(views()[0]!).line).toBe(2);
  // 파일 없이 메인 체크아웃으로 돌아오는 요청은 없다(폴더만 다시 열면 바꾸지 않는다) — 파일을 열면 메인 체크아웃이다.
  await openRequest(README);
  expect(activeWorktreeStore.getState().top).toBeUndefined();
  expect(editorTabs().map((t) => t.title)).toEqual(["README.txt"]);
});

it("asks before switching worktrees over unsaved edits: Cancel stays, Don't Save switches", async () => {
  await mount();
  await openRequest(README, undefined, undefined, "/work/wt1");
  await typeInto(views()[0]!, 5, "!");
  await act(async () => projectOpenInbox.push({ worktree: "/work/wt2", file: MAIN }));
  await settle();
  expect(dialog()?.textContent).toContain("README.txt");
  await clickButton(dialog()!, "취소");
  expect(editorTabs().map((t) => t.path)).toEqual([README]);
  expect(activeWorktreeStore.getState().top).toBe("/work/wt1");
  expect(dirtyTabs()).toEqual([README]);

  await act(async () => projectOpenInbox.push({ worktree: "/work/wt2", file: MAIN }));
  await settle();
  await clickButton(dialog()!, "저장 안 함");
  expect(editorTabs().map((t) => t.title)).toEqual(["main.rs"]);
  expect(activeWorktreeStore.getState().top).toBe("/work/wt2");
  expect(writes()).toEqual([]);

  await act(async () => projectOpenInbox.push({ worktree: "/work/wt1" }));
  await settle();
  expect(editorTabs().map((t) => t.title)).toEqual(["README.txt"]);
  expect(views()[0]!.state.doc.toString()).toBe("hello");
});

it("writes the tab set and layout at once when the window is about to close (quit included)", async () => {
  await mount();
  await openRequest(LIB, 40, 2);
  // 모아 쓰기(0.2초 · 0.3초)를 기다리지 않는다 — 닫기 확인이 마저 쓴다.
  expect(savedState()).toBeUndefined();
  let closed: boolean | undefined;
  await act(async () => {
    closed = await closeGuard!();
  });
  expect(closed).toBe(true);
  expect(savedState()?.main?.tabs).toEqual([
    expect.objectContaining({ path: LIB, cursor: { line: 40, column: 2 } }),
  ]);
  expect(savedState()?.layout).toBeDefined();
});

it("skips a saved tab whose file is gone without an open error", async () => {
  await mount();
  await openRequest(LIB);
  await openRequest(MAIN);
  await vi.waitFor(() => expect(savedState()?.main?.tabs).toHaveLength(2), { timeout: 2000 });
  host.files.delete(MAIN);
  host.calls = [];
  await restart();
  expect(editorTabs().map((t) => t.title)).toEqual(["lib.rs"]);
  expect(host.calls.filter((c) => c.method === "fs.read" && c.params.path === MAIN)).toEqual([]);
  expect(document.body.textContent).not.toContain("main.rs");
});

it("opens the default layout when the saved project state is broken", async () => {
  for (const broken of [
    { version: 1, layout: { version: 1, layout: { grid: 3, panels: {} } } },
    { version: 99, layout: {} },
    { version: 1, main: { tabs: "nope" } },
  ]) {
    host.states.set("p1", broken);
    await restart();
    expect(dock!.getPanel("project")).toBeDefined();
    expect(dock!.getPanel("terminal")).toBeDefined();
    expect(editorTabs()).toEqual([]);
  }
});

it("moves the old localStorage layout into the first project once and removes it", async () => {
  await mount();
  await act(async () => dock!.getPanel("project")!.api.close());
  await vi.waitFor(
    () => expect(JSON.stringify(savedState()?.layout)).toContain('"hidden":{"project"'),
    { timeout: 2000 },
  );
  const old = JSON.stringify(savedState()!.layout);
  host.states = new Map();
  localStorage.setItem(LAYOUT_STORAGE_KEY, old);

  await restart("p1");
  expect(dock!.getPanel("project")).toBeUndefined();
  await vi.waitFor(() => expect(localStorage.getItem(LAYOUT_STORAGE_KEY)).toBeNull(), {
    timeout: 2000,
  });
  expect(JSON.stringify(savedState("p1")?.layout)).toBe(old);

  await restart("p2");
  expect(dock!.getPanel("project")).toBeDefined();
});

// ── 레이아웃 액션(10-ui 「툴 윈도우 · 창」, M1-25) ──

/** 툴 윈도우의 내용에 포커스를 둔다(클릭한 것처럼). */
async function focusToolWindow(id: string) {
  await act(async () => dock!.getPanel(id)!.api.setActive());
  await settle();
  await act(async () => document.querySelector<HTMLElement>(`[data-tool-window="${id}"]`)!.focus());
}

const focusedToolWindowId = () =>
  document.activeElement?.closest("[data-tool-window]")?.getAttribute("data-tool-window");

it("goes back to the editor on Esc from a tool window, but not from an input, a popup or IME composition", async () => {
  await mount();
  await openRequest(MAIN, 2, 1);
  const [view] = views();
  await focusToolWindow("notifications");
  expect(view!.hasFocus).toBe(false);

  // 한글 조합 중의 Esc 는 조합만 취소한다 — 키맵은 보지 않는다.
  await keydown({ code: "Escape", key: "Escape", isComposing: true });
  expect(focusedToolWindowId()).toBe("notifications");
  // 툴 윈도우 안의 입력 칸(검색 칸 등)의 Esc 는 그 칸의 것이다.
  const input = document.createElement("input");
  document.querySelector("[data-tool-window=notifications]")!.append(input);
  await act(async () => input.focus());
  await keydown({ code: "Escape", key: "Escape" });
  expect(document.activeElement).toBe(input);
  input.remove();
  await focusToolWindow("notifications");
  // 창의 팝업(액션 찾기)이 열려 있으면 Esc 는 팝업이 받는다.
  await press("KeyA", { metaKey: true, shiftKey: true }, "a");
  expect(document.querySelector("[cmdk-input]")).not.toBeNull();
  await act(async () =>
    document.querySelector<HTMLElement>("[data-tool-window=notifications]")!.focus(),
  );
  await keydown({ code: "Escape", key: "Escape" });
  expect(view!.hasFocus).toBe(false);
  await settle();

  await focusToolWindow("notifications");
  await press("Escape");
  expect(view!.hasFocus).toBe(true);
  expect(activeTabTitle()).toBe("main.rs");
});

it("jumps to the last tool window on F12 and hides every tool window dock on ⌘⇧F12", async () => {
  await mount();
  await openRequest(MAIN);
  await focusToolWindow("notifications");
  await press("Escape");
  expect(views()[0]!.hasFocus).toBe(true);

  await press("F12");
  await act(async () => new Promise((r) => requestAnimationFrame(() => r(undefined))));
  expect(dock!.activePanel?.id).toBe("notifications");
  expect(focusedToolWindowId()).toBe("notifications");

  // 숨긴 툴 윈도우도 F12 가 다시 연다.
  await act(async () => dock!.getPanel("notifications")!.api.close());
  await settle();
  await press("F12");
  expect(dock!.getPanel("notifications")).toBeDefined();

  // 툴 윈도우 바로 띄운 것도 센다 — 포커스는 에디터에 남아 있어도.
  await act(async () => views()[0]!.focus());
  await act(async () =>
    document.querySelector<HTMLButtonElement>('nav button[aria-label="에이전트 활동"]')!.click(),
  );
  await act(async () => views()[0]!.focus());
  await press("F12");
  expect(dock!.activePanel?.id).toBe("agent");

  await press("F12", { metaKey: true, shiftKey: true });
  for (const edge of ["left", "right", "bottom"] as const)
    expect(dock!.getEdgeGroup(edge)?.isCollapsed() ?? true).toBe(true);
});

it("moves the edge of the focused tool window's dock with ⌃⌥⇧ arrows, only from a tool window", async () => {
  await mount();
  await openRequest(MAIN);
  // happy-dom 은 레이아웃을 하지 않아 dock 의 실제 폭이 바뀌지 않는다 — 요청한 크기를 본다.
  const right = dock!.getEdgeGroup("right")!;
  const left = dock!.getEdgeGroup("left")!;
  const sized = (group: typeof right) => vi.spyOn(group, "setSize").mockImplementation(() => {});
  const rightSizes = sized(right);
  const leftSizes = sized(left);
  await focusToolWindow("notifications");
  const mods = { ctrlKey: true, altKey: true, shiftKey: true };
  await press("ArrowLeft", mods, "ArrowLeft");
  await press("ArrowRight", mods, "ArrowRight");
  // 오른쪽 dock 에 엇갈린 ↑ 는 아무것도 하지 않는다.
  await press("ArrowUp", mods, "ArrowUp");
  const w = right.width;
  expect(rightSizes.mock.calls).toEqual([
    [{ width: Math.max(60, w + 20) }],
    [{ width: Math.max(60, w - 20) }],
  ]);

  await focusToolWindow("project");
  await press("ArrowRight", mods, "ArrowRight");
  expect(leftSizes.mock.calls).toEqual([[{ width: Math.max(60, left.width + 20) }]]);

  // 에디터에서는 이 키를 가져가지 않는다(Win/Linux 의 Ctrl+Shift+←→ 는 단어 선택이다).
  await act(async () => views()[0]!.focus());
  await press("ArrowLeft", mods, "ArrowLeft");
  expect(leftSizes).toHaveBeenCalledTimes(1);
  expect(rightSizes).toHaveBeenCalledTimes(2);
});

/** 액션 찾기(⌘⇧A)에서 `label` 로 시작하는 항목을 고른다. */
async function runAction(label: string) {
  await press("KeyA", { metaKey: true, shiftKey: true }, "a");
  const input = document.querySelector<HTMLInputElement>("[cmdk-input]")!;
  await act(async () => {
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
    setter.call(input, label);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await settle();
  const item = [...document.querySelectorAll<HTMLElement>("[cmdk-item]")].find((e) =>
    e.textContent?.startsWith(label),
  );
  if (!item) throw new Error(`액션 ${label} 이 없다`);
  await act(async () => item.click());
  await settle();
}

// ── 편집 · 저장 · 외부 변경(12-m1-plan M1-13) ──

/**
 * 사용자가 친 것처럼 뷰에 넣는다(happy-dom 은 contenteditable 입력을 흉내 내지 않는다). 포커스를 먼저 뺀다 — happy-dom 은
 * `selectionchange` 를 브라우저와 달리 선택을 쓰는 자리에서 바로 내서, 포커스된 뷰가 자기 갱신 중에 다시 갱신하려다 던진다.
 */
async function typeInto(view: EditorView, at: number, insert: string) {
  (document.activeElement as HTMLElement | null)?.blur();
  await act(async () =>
    view.dispatch({
      changes: { from: at, insert },
      selection: { anchor: at + insert.length },
      userEvent: "input.type",
    }),
  );
  await settle();
}

const writes = () => host.calls.filter((c) => c.method === "fs.write").map((c) => c.params);
const dirtyTabs = () =>
  [...document.querySelectorAll<HTMLElement>("[data-editor-tab-header][data-dirty]")].map(
    (e) => e.title,
  );
const banner = () => document.querySelector<HTMLElement>("[data-testid=disk-banner]");
const dialog = () => document.querySelector<HTMLElement>("[data-testid=unsaved-dialog]");

async function clickButton(scope: ParentNode, text: string) {
  const button = [...scope.querySelectorAll<HTMLButtonElement>("button")].find(
    (b) => b.textContent === text,
  );
  if (!button) throw new Error(`단추 ${text} 가 없다`);
  await act(async () => button.click());
  await settle();
}

it("edits, marks the tab unsaved and saves on ⌘S with the version it read, once for key and menu", async () => {
  await mount();
  await openRequest(MAIN, 2, 1);
  const [view] = views();
  expect(view!.state.readOnly).toBe(false);
  const read = versionOf(MAIN);
  await typeInto(view!, view!.state.doc.line(2).from, "// ");
  expect(dirtyTabs()).toEqual([MAIN]);

  await keydown({ code: "KeyS", key: "s", metaKey: true });
  await menuAction("editor.save");
  expect(writes()).toHaveLength(1);
  expect(writes()[0]).toMatchObject({
    projectId: "p1",
    path: MAIN,
    expectedVersion: read,
    content: { kind: "text" },
  });
  expect(host.files.get(MAIN)!.split("\n")[1]).toBe("// line 2 — some text");
  expect(dirtyTabs()).toEqual([]);
});

it("keeps undo history, the selection and unsaved text when a tab moves to another group", async () => {
  await mount();
  await openRequest(LIB, 10, 1);
  const before = views()[0]!;
  await typeInto(before, before.state.doc.line(10).from, "edited ");
  await act(async () =>
    before.dispatch({
      selection: { anchor: before.state.doc.line(12).from, head: before.state.doc.line(12).to },
    }),
  );
  const panel = dock!.panels.find((p) => p.api.component === EDITOR_PANEL)!;
  await act(async () => {
    const group = dock!.addGroup({ referenceGroup: panel.group.id, direction: "right" });
    panel.api.moveTo({ group, position: "center" });
  });
  await settle();
  expect(dock!.groups.filter((g) => g.panels.includes(panel))).toHaveLength(1);

  const [after] = views();
  expect(views()).toHaveLength(1);
  expect(after!.state.doc.line(10).text).toBe("edited line 10 — some text");
  expect(after!.state.selection.main).toMatchObject({
    from: after!.state.doc.line(12).from,
    to: after!.state.doc.line(12).to,
  });
  expect(dirtyTabs()).toEqual([LIB]);

  // undo 기록이 옮긴 뒤에도 이어진다. undo 는 창의 키맵 액션(`editor.undo`)이고 키맵은 userAgent 로 macOS 다 — ⌘Z(포커스는
  // typeInto 의 이유로 뺀 채 키를 보내므로 활성 탭이 대상이다).
  after!.contentDOM.blur();
  await act(async () => {
    after!.contentDOM.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "z",
        code: "KeyZ",
        metaKey: true,
        bubbles: true,
        cancelable: true,
      }),
    );
  });
  await settle();
  expect(after!.state.doc.line(10).text).toBe("line 10 — some text");
  expect(dirtyTabs()).toEqual([]);
});

it("shows an edit in the other group's view of the same file at once", async () => {
  await mount();
  await openRequest(LIB, 1, 1);
  await press("Backslash", { metaKey: true }, "\\");
  const [a, b] = views();
  await typeInto(a!, 0, "x");
  expect(b!.state.doc.line(1).text).toBe("xline 1 — some text");
  expect(dirtyTabs()).toEqual([LIB, LIB]);
});

it("asks before closing the last tab of an unsaved file: Cancel keeps it, Don't Save closes", async () => {
  await mount();
  await openRequest(MAIN);
  await openRequest(README);
  const view = views().find((v) => v.state.doc.toString() === "hello")!;
  await typeInto(view, 5, "!");

  await press("KeyW", { metaKey: true }, "w");
  expect(dialog()?.textContent).toContain("README.txt");
  await clickButton(dialog()!, "취소");
  expect(dialog()).toBeNull();
  expect(editorTabs().map((t) => t.path)).toEqual([MAIN, README]);

  await press("KeyW", { metaKey: true }, "w");
  await clickButton(dialog()!, "저장 안 함");
  expect(editorTabs().map((t) => t.path)).toEqual([MAIN]);
  expect(writes()).toEqual([]);
  expect(host.files.get(README)).toBe("hello");
});

it("saves and then closes when Save is chosen, and asks also for the tab's close button", async () => {
  await mount();
  await openRequest(README);
  await typeInto(views()[0]!, 5, "!");
  const close = document.querySelector<HTMLElement>(
    "[data-editor-tab-header] .dv-default-tab-action",
  )!;
  await act(async () => close.click());
  await settle();
  expect(dialog()).not.toBeNull();
  await clickButton(dialog()!, "저장");
  expect(host.files.get(README)).toBe("hello!");
  expect(editorTabs()).toEqual([]);
});

it("does not ask when another tab still shows the unsaved file", async () => {
  await mount();
  await openRequest(README);
  await press("Backslash", { metaKey: true }, "\\");
  await typeInto(views()[0]!, 0, "> ");
  await press("KeyW", { metaKey: true }, "w");
  expect(dialog()).toBeNull();
  expect(editorTabs()).toHaveLength(1);
  expect(dirtyTabs()).toEqual([README]);
});

it("asks before the window closes with an unsaved tab (⑤)", async () => {
  await mount();
  await openRequest(README);
  expect(await act(() => closeGuard!())).toBe(true);

  await typeInto(views()[0]!, 0, "> ");
  let answer: Promise<boolean> | undefined;
  await act(async () => {
    answer = closeGuard!();
  });
  await settle();
  expect(dialog()?.textContent).toContain("README.txt");
  await clickButton(dialog()!, "취소");
  expect(await answer).toBe(false);

  await act(async () => {
    answer = closeGuard!();
  });
  await settle();
  await clickButton(dialog()!, "저장 안 함");
  expect(await answer).toBe(true);
});

it("shows an external change within 500 ms and keeps the cursor line when nothing is unsaved (①)", async () => {
  await mount();
  await openRequest(LIB, 150, 4);
  const [view] = views();
  const next = host.files.get(LIB)!.split("\n");
  next[9] = "line 10 — changed by an agent";
  next[279] = "line 280 — changed by an agent";
  putFile(LIB, next.join("\n"));

  const started = performance.now();
  await changed([LIB]);
  await vi.waitFor(() => expect(view!.state.doc.line(10).text).toBe(next[9]), { timeout: 500 });
  const took = performance.now() - started;
  expect(took).toBeLessThan(500);
  expect(cursorOf(view!)).toEqual({ line: 150, column: 4 });
  expect(views()).toEqual([view]);
  expect(banner()).toBeNull();
});

it("shows the banner for an external change over unsaved edits, and Compare shows both side by side (②)", async () => {
  await mount();
  await openRequest(MAIN);
  const [view] = views();
  await typeInto(view!, 0, "mine ");
  putFile(MAIN, "theirs\n");
  await changed([MAIN]);
  await vi.waitFor(() => expect(banner()?.dataset.disk).toBe("changed"));
  expect(view!.state.doc.line(1).text).toBe("mine line 1 — some text");

  await clickButton(banner()!, "비교");
  const compare = document.querySelector<HTMLElement>("[data-testid=disk-compare]")!;
  const sides = [...compare.querySelectorAll<HTMLElement>(".cm-editor")].map((e) =>
    EditorView.findFromDOM(e)!.state.doc.toString(),
  );
  expect(sides).toEqual([view!.state.doc.toString(), "theirs\n"]);

  await clickButton(banner()!, "디스크 것 받기");
  expect(banner()).toBeNull();
  expect(document.querySelector("[data-testid=disk-compare]")).toBeNull();
  expect(view!.state.doc.toString()).toBe("theirs\n");
  expect(dirtyTabs()).toEqual([]);
});

it("stops ⌘S with a conflict and the banner after a change the watch missed (③)", async () => {
  await mount();
  await openRequest(MAIN);
  await typeInto(views()[0]!, 0, "mine ");
  putFile(MAIN, "theirs\n"); // fs.changed 없이

  await press("KeyS", { metaKey: true }, "s");
  await vi.waitFor(() => expect(banner()?.dataset.disk).toBe("changed"));
  expect(host.files.get(MAIN)).toBe("theirs\n");
  expect(dirtyTabs()).toEqual([MAIN]);

  await clickButton(banner()!, "내 것 유지");
  await press("KeyS", { metaKey: true }, "s");
  expect(host.files.get(MAIN)!.startsWith("mine line 1")).toBe(true);
  expect(dirtyTabs()).toEqual([]);
});

it("keeps CRLF line endings when a CRLF file is edited and saved (④)", async () => {
  host.files.set(README, "one\r\ntwo\r\n");
  await mount();
  await openRequest(README);
  const [view] = views();
  await typeInto(view!, view!.state.doc.line(2).to, "!");
  await press("KeyS", { metaKey: true }, "s");
  expect(host.files.get(README)).toBe("one\r\ntwo!\r\n");
});

it("does not raise the banner for its own save coming back as fs.changed", async () => {
  await mount();
  await openRequest(MAIN);
  await typeInto(views()[0]!, 0, "x");
  await press("KeyS", { metaKey: true }, "s");
  await changed([MAIN]);
  await settle();
  expect(banner()).toBeNull();
  expect(dirtyTabs()).toEqual([]);
});

it("turns a preview tab into a normal tab when its file gets an unsaved edit", async () => {
  await mount();
  await openRequest(README);
  // 파일 트리(M1-14)의 미리보기 열기 대신 탭 params 를 미리보기로 둔다.
  const panel = dock!.panels.find((p) => p.api.component === EDITOR_PANEL)!;
  await act(async () => panel.api.updateParameters({ ...panel.params, preview: true }));
  await settle();
  expect(editorTabs()[0]).toMatchObject({ preview: true });
  await typeInto(views()[0]!, 0, "x");
  expect(editorTabs()[0]).toMatchObject({ preview: false });
});
