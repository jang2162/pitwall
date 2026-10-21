// @vitest-environment happy-dom
// 프로젝트 창(App kind="project", `?project=p1`)의 Search Everywhere 를 가짜 Host 에 붙여 본다(12-m1-plan M1-19 완료 기준): ⌘⇧O 파일로
// 이동(`search.files` · 일치 글자 강조 · 최근 연 파일 먼저 · `파일:줄`), ⇧⇧ 의 전체 탭에 액션과 파일이 함께, ⌘⇧P 프로젝트 스위처가
// 그 창을 열거나 앞으로 가져오는 명령을 부르는 것, ⌘E 최근 파일, 한글 입력 상태의 키, 한 창에 팝업이 하나뿐인 것.

import type {
  ConfigView,
  FsEntry,
  FsRead,
  FsStat,
  HostInfo,
  KeymapFile,
  LangDetectResult,
  Project,
  ProjectTrust,
  Resolution,
  SearchFiles,
  SearchFilesParams,
} from "@pitwall/rpc-client";
import { EditorView } from "@codemirror/view";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../App";
import { resetWindowFileTree } from "../file-tree/project-view";
import { fakeTreeLayout } from "../file-tree/testing";
import type { KeymapNative } from "../keymap/native";
import { activeWorktreeStore } from "../open-with/active-worktree";
import { projectOpenInbox } from "../project-open";
import { createThemeStore } from "../theme/theme-store";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const host = vi.hoisted(() => ({
  /** 경로 → 파일 내용. 폴더는 `dirs`. */
  files: new Map<string, string>(),
  dirs: new Map<string, string[]>(),
  calls: [] as { method: string; params: Record<string, unknown> }[],
  /** 받은 `search.files` 인자. */
  fileSearches: [] as SearchFilesParams[],
  /** 있으면 `search.files` 의 답을 이 순서로 앞에 둔다(점수가 높은 것처럼). */
  ranked: [] as string[],
  send: (_m: object) => {},
}));

vi.mock("../tauri-transport", async () => {
  const { createMemoryTransportPair: pair } = await import("@pitwall/rpc-client");
  const { DEFAULT_KEYMAP_TOML } = await import("../keymap/default-keymap");
  const { i18nState } = await import("../i18n/testing");
  return {
    connectHost: async () => {
      const [client, server] = pair();
      let next = 1;
      const send = (m: object) => server.send(JSON.stringify({ jsonrpc: "2.0", ...m }));
      host.send = send;
      const other: Project = {
        id: "p2",
        name: "billing",
        host: "local",
        roots: ["/work/billing"],
        favorite: false,
        tags: [],
        configMode: "repository",
        slug: "billing",
        createdAt: 1,
        openedAt: 5,
      };
      const project: Project = {
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
      };
      server.onMessage(async (text) => {
        const m = JSON.parse(text) as { id?: number; method: string; params: never };
        const reply = (result: unknown) => send({ id: m.id, result });
        const fail = (message: string) =>
          send({ id: m.id, error: { code: -32004, message, data: null } });
        if (m.method.startsWith("fs.")) host.calls.push({ method: m.method, params: m.params });
        const path = (m.params as { path?: string } | undefined)?.path ?? "";
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
            return reply(project);
          case "project.list":
            return reply([other, project]);
          case "project.resolve":
            return reply({
              project,
              root: "/work/demo",
              path,
              worktree: { top: path, root: `${path}/demo` },
            } satisfies Resolution);
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
          case "fs.list": {
            const names = host.dirs.get(path);
            if (!names) return fail(`not a folder: ${path}`);
            const entries: FsEntry[] = names.map((name) => ({
              name,
              kind: host.dirs.has(`${path}/${name}`) ? "dir" : "file",
              target: null,
              outside: false,
              size: 0,
              mtimeMs: 1,
              ignored: false,
            }));
            return reply({ path, entries });
          }
          case "fs.stat": {
            const isDir = host.dirs.has(path);
            if (!isDir && !host.files.has(path)) return fail(`not found: ${path}`);
            return reply({
              path,
              kind: isDir ? "dir" : "file",
              target: null,
              size: host.files.get(path)?.length ?? 0,
              mtimeMs: 1,
              readonly: false,
              sizeClass: isDir ? null : "editable",
            } satisfies FsStat);
          }
          case "fs.read": {
            const text = host.files.get(path);
            if (text === undefined) return fail(`not found: ${path}`);
            return reply({
              path,
              size: text.length,
              mtimeMs: 1,
              offset: 0,
              eof: true,
              version: "v1",
              content: { kind: "text", text },
              judgment: {
                binary: false,
                encoding: "UTF-8",
                bom: null,
                lineEnding: "lf",
                longLines: false,
                sizeClass: "editable",
                editable: true,
                highlight: true,
              },
            } satisfies FsRead);
          }
          case "lang.detect": {
            const { files } = m.params as { files: { path: string }[] };
            return reply({
              files: files.map(() => ({
                language: null,
                source: "none",
                vendored: false,
                generated: false,
                documentation: false,
              })),
            } satisfies LangDetectResult);
          }
          case "events.subscribe":
            return reply({ subscriptionId: next++ });
          case "events.unsubscribe":
            return reply(null);
          case "search.files": {
            // 가짜 매칭: 상대 경로에 질의 글자가 차례로 들어 있으면 맞는다. `ranked` 의 것을 앞에 둔다.
            const params = m.params as SearchFilesParams;
            host.fileSearches.push(params);
            const { matchRecent } = await import("./file-query");
            const hits = [...host.files.keys()]
              .map((p) => ({ path: p, relative: p.slice("/work/demo/".length) }))
              .map((f) => ({ ...f, positions: matchRecent(f.relative, params.query) }))
              .filter((f) => f.positions)
              .sort((a, b) => {
                const rank = (p: string) => {
                  const i = host.ranked.indexOf(p);
                  return i < 0 ? host.ranked.length : i;
                };
                return rank(a.path) - rank(b.path) || a.relative.localeCompare(b.relative);
              });
            return reply({
              files: hits.slice(0, params.limit ?? 100).map((f) => ({
                path: f.path,
                root: "/work/demo",
                relative: f.relative,
                positions: f.positions!,
                score: 1,
                vendored: false,
                generated: false,
              })),
              matched: hits.length,
              indexed: host.files.size,
            } satisfies SearchFiles);
          }
          default:
            return m.id === undefined
              ? undefined
              : send({ id: m.id, error: { code: -32601, message: `no ${m.method}` } });
        }
      });
      return { transport: client, close: async () => {} };
    },
  };
});

const native = {
  syncMenu: async () => {},
  onMenuAction: () => () => {},
  openSettings: async () => {},
  closeWindow: async () => {},
  openProjectWindow: vi.fn<NonNullable<KeymapNative["openProjectWindow"]>>(async () => {}),
} satisfies KeymapNative;

let root: Root | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
  );
  vi.spyOn(console, "info").mockImplementation(() => {});
  fakeTreeLayout(600);
  window.history.replaceState(null, "", "/?window=project&project=p1");
  const lines = Array.from({ length: 30 }, (_, i) => `line ${i + 1}`).join("\n");
  host.files = new Map([
    ["/work/demo/src/main.rs", `${lines}\n`],
    ["/work/demo/src/lib.rs", "fn lib() {}\n"],
    ["/work/demo/src/settings.rs", "// settings\n"],
    ["/work/demo/docs/main.md", "# main\n"],
  ]);
  host.dirs = new Map([
    ["/work/demo", ["docs", "src"]],
    ["/work/demo/src", ["lib.rs", "main.rs", "settings.rs"]],
    ["/work/demo/docs", ["main.md"]],
  ]);
  host.calls = [];
  host.fileSearches = [];
  host.ranked = [];
  native.openProjectWindow.mockClear();
  activeWorktreeStore.setState({ top: undefined });
  resetWindowFileTree();
  localStorage.clear();
});

afterEach(() => {
  act(() => root?.unmount());
  root = undefined;
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

const settle = (ms = 20) => act(async () => new Promise((r) => setTimeout(r, ms)));

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
  // 찾는 곳은 파일 트리의 루트다 — 루트를 읽을 때까지 기다린다.
  await vi.waitFor(() =>
    expect(document.querySelector('[data-path="/work/demo/src"]')).not.toBeNull(),
  );
}

async function press(init: KeyboardEventInit, target?: Element | null) {
  await act(async () => {
    (target ?? document.activeElement ?? document.body).dispatchEvent(
      new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }),
    );
    document.body.dispatchEvent(new KeyboardEvent("keyup", { bubbles: true, ...init }));
  });
  await settle();
}

/** Shift 를 두 번 눌렀다 뗀다. `init` 은 두 번 모두에 붙는다(IME 상태 등). */
async function shiftShift(init: KeyboardEventInit = {}) {
  for (let i = 0; i < 2; i++) {
    await act(async () => {
      document.activeElement?.dispatchEvent(
        new KeyboardEvent("keydown", {
          bubbles: true,
          code: "ShiftLeft",
          key: "Shift",
          shiftKey: true,
          ...init,
        }),
      );
      document.activeElement?.dispatchEvent(
        new KeyboardEvent("keyup", { bubbles: true, code: "ShiftLeft", key: "Shift", ...init }),
      );
    });
  }
  await settle();
}

const gotoFile = (key = "O") => press({ code: "KeyO", key, metaKey: true, shiftKey: true });
const popup = () => document.querySelector<HTMLElement>("[data-search-everywhere]");
const input = () => popup()!.querySelector<HTMLInputElement>("[cmdk-input]")!;
const items = () => [...popup()!.querySelectorAll<HTMLElement>("[cmdk-item]")];
const values = () => items().map((i) => i.getAttribute("data-value"));
const selected = () =>
  popup()!.querySelector("[cmdk-item][aria-selected=true]")?.getAttribute("data-value");

/** React 가 듣는 input 이벤트로 칸의 값을 바꾼다. */
async function type(value: string) {
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input(), value);
    input().dispatchEvent(new Event("input", { bubbles: true }));
  });
  await settle();
}

async function openTab(path: string) {
  await act(async () => projectOpenInbox.push({ file: path }));
  await vi.waitFor(() =>
    expect(
      [...document.querySelectorAll("[data-editor-tab-header]")].some(
        (t) => t.textContent === path.slice(path.lastIndexOf("/") + 1),
      ),
    ).toBe(true),
  );
  await settle();
}

const editorLine = () => {
  const view = EditorView.findFromDOM(
    document.querySelector<HTMLElement>(".dv-active-group .cm-editor, .cm-editor")!,
  )!;
  const head = view.state.selection.main.head;
  const line = view.state.doc.lineAt(head);
  return [line.number, head - line.from + 1];
};

it("finds files with ⌘⇧O through search.files and marks the matched characters", async () => {
  await mount();
  await gotoFile();
  expect(popup()?.getAttribute("data-search-everywhere")).toBe("files");
  expect(document.querySelectorAll("[cmdk-dialog]")).toHaveLength(1);
  await type("mars");
  await vi.waitFor(() => expect(values()).toEqual(["file:/work/demo/src/main.rs"]));
  expect(host.fileSearches.at(-1)).toEqual({
    projectId: "p1",
    worktree: null,
    query: "mars",
    limit: 50,
  });
  const marks = [...items()[0]!.querySelectorAll("mark[data-match]")].map((m) => m.textContent);
  // 파일 이름 main.rs 의 m · a · r · s 가 칠해진다.
  expect(marks.join("")).toBe("mars");
  expect(items()[0]!.textContent).toContain("main.rs");
});

it("opens main.rs at line 12 when 「main.rs:12」 is entered", async () => {
  await mount();
  await gotoFile();
  await type("main.rs:12");
  await vi.waitFor(() => expect(values()[0]).toBe("file:/work/demo/src/main.rs"));
  // 줄을 뗀 이름으로 찾는다.
  expect(host.fileSearches.at(-1)?.query).toBe("main.rs");
  expect(items()[0]!.textContent).toContain("12줄");
  await press({ code: "Enter", key: "Enter" });
  expect(popup()).toBeNull();
  await vi.waitFor(() => expect(document.querySelectorAll(".cm-editor")).toHaveLength(1));
  await vi.waitFor(() => expect(editorLine()).toEqual([12, 1]));
  expect(host.calls.find((c) => c.method === "fs.stat")?.params).toMatchObject({
    path: "/work/demo/src/main.rs",
    worktree: null,
  });
});

it("puts recently opened files first", async () => {
  await mount();
  await openTab("/work/demo/docs/main.md");
  // Host 는 src/main.rs 를 더 높게 친다.
  host.ranked = ["/work/demo/src/main.rs", "/work/demo/docs/main.md"];
  await gotoFile();
  await type("main");
  await vi.waitFor(() =>
    expect(values()).toEqual(["file:/work/demo/docs/main.md", "file:/work/demo/src/main.rs"]),
  );
  expect(items()[0]!.textContent).toContain("최근");
});

it("shows actions and files together in the All tab of ⇧⇧", async () => {
  await mount();
  await shiftShift();
  expect(popup()?.getAttribute("data-search-everywhere")).toBe("all");
  const tabs = [...popup()!.querySelectorAll('[role="tab"]')].map((t) => t.textContent);
  expect(tabs).toEqual(["전체", "파일", "액션", "프로젝트"]);
  await type("settings");
  await vi.waitFor(() => expect(values()).toContain("file:/work/demo/src/settings.rs"));
  expect(values()).toContain("action:settings.open");
  const headings = [...popup()!.querySelectorAll("[cmdk-group-heading]")].map((h) => h.textContent);
  expect(headings).toEqual(["파일", "액션"]);
});

it("opens ⇧⇧ and ⌘⇧O while the IME is in Korean, but not while composing", async () => {
  await mount();
  await shiftShift({ isComposing: true });
  expect(popup()).toBeNull();
  await press({ code: "KeyO", key: "ㅐ", metaKey: true, shiftKey: true, isComposing: true });
  expect(popup()).toBeNull();

  // 한글 입력 상태의 ⌘⇧O: key 는 「ㅐ」, code 는 KeyO.
  await gotoFile("ㅐ");
  expect(popup()?.getAttribute("data-search-everywhere")).toBe("files");
  await press({ code: "Escape", key: "Escape" });
  expect(popup()).toBeNull();
  await shiftShift();
  expect(popup()?.getAttribute("data-search-everywhere")).toBe("all");
});

it("cycles the tabs with Tab and keeps a single popup when another search key is pressed", async () => {
  await mount();
  await gotoFile();
  await press({ code: "Tab", key: "Tab" }, input());
  expect(popup()?.getAttribute("data-search-everywhere")).toBe("actions");
  await press({ code: "Tab", key: "Tab" }, input());
  expect(popup()?.getAttribute("data-search-everywhere")).toBe("projects");
  await press({ code: "Tab", key: "Tab", shiftKey: true }, input());
  expect(popup()?.getAttribute("data-search-everywhere")).toBe("actions");
  // 조합 중의 Tab 은 탭을 돌지 않는다.
  await press({ code: "Tab", key: "Tab", isComposing: true }, input());
  expect(popup()?.getAttribute("data-search-everywhere")).toBe("actions");
  // 팝업이 열린 채 ⌘⇧A 를 눌러도 두 번째 팝업이 생기지 않고 입력 칸이 포커스를 지킨다.
  await press({ code: "KeyA", key: "A", metaKey: true, shiftKey: true }, input());
  expect(document.querySelectorAll("[cmdk-dialog]")).toHaveLength(1);
  expect(document.activeElement).toBe(input());
});

it("switches to another project's window with ⌘⇧P, and only closes the popup for this one", async () => {
  await mount();
  await press({ code: "KeyP", key: "P", metaKey: true, shiftKey: true });
  expect(popup()?.getAttribute("data-search-everywhere")).toBe("projects");
  await vi.waitFor(() => expect(values()).toEqual(["project:p2", "project:p1"]));
  expect(items()[1]!.textContent).toContain("이 창");
  await press({ code: "ArrowDown", key: "ArrowDown" });
  await press({ code: "Enter", key: "Enter" });
  expect(popup()).toBeNull();
  await settle();
  expect(native.openProjectWindow).not.toHaveBeenCalled();

  await press({ code: "KeyP", key: "P", metaKey: true, shiftKey: true });
  await type("bill");
  await vi.waitFor(() => expect(values()).toEqual(["project:p2"]));
  await press({ code: "Enter", key: "Enter" });
  await settle();
  expect(native.openProjectWindow).toHaveBeenCalledExactlyOnceWith("p2");
});

it("lists recent files with ⌘E, selects the previous one, and opens it on Enter", async () => {
  await mount();
  await openTab("/work/demo/src/lib.rs");
  await openTab("/work/demo/src/main.rs");
  await press({ code: "KeyE", key: "E", metaKey: true });
  expect(popup()?.getAttribute("data-search-everywhere")).toBe("recentFiles");
  expect(popup()!.querySelector('[role="tablist"]')).toBeNull();
  expect(values()).toEqual(["file:/work/demo/src/main.rs", "file:/work/demo/src/lib.rs"]);
  await vi.waitFor(() => expect(selected()).toBe("file:/work/demo/src/lib.rs"));
  await press({ code: "Enter", key: "Enter" });
  expect(popup()).toBeNull();
  await vi.waitFor(() =>
    expect(document.querySelector(".dv-active-tab [data-editor-tab-header]")?.textContent).toBe(
      "lib.rs",
    ),
  );
  // Host 에 묻지 않는다.
  expect(host.fileSearches).toEqual([]);
});
