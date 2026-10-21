// @vitest-environment happy-dom
// 프로젝트 창(App kind="project", `?project=p1`)의 전체에서 찾기를 가짜 Host 에 붙여 본다(12-m1-plan M1-20 완료 기준): ⌘⇧F 팝업의
// 옵션이 `search.text` 인자로 가고, 입력을 바꾸면 이전 스트림에 `stream.cancel` 이 가고, 상한을 넘으면 「더 있음」, 결과에서 Enter →
// 그 줄이 열린다. 고정(⌘Enter) · 「검색 결과」 툴 윈도우 · 파일 트리 「이 폴더에서 찾기」 · 미리보기의 일치 강조도 본다.

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
  SearchFile,
  SearchTextChunk,
  SearchTextParams,
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
import { createThemeStore } from "../theme/theme-store";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";
import { resetFindWindow } from "./find-in-files";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const host = vi.hoisted(() => ({
  /** 경로 → 파일 내용. 폴더는 `dirs`. */
  files: new Map<string, string>(),
  dirs: new Map<string, string[]>(),
  searches: [] as { streamId: number; params: SearchTextParams }[],
  cancelled: [] as number[],
  calls: [] as { method: string; params: Record<string, unknown> }[],
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
      server.onMessage((text) => {
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
          case "search.text": {
            const streamId = next++;
            host.searches.push({ streamId, params: m.params });
            return reply({ streamId });
          }
          case "stream.cancel": {
            const { streamId } = m.params as { streamId: number };
            host.cancelled.push(streamId);
            return send({ method: "stream.end", params: { streamId, status: "cancelled" } });
          }
          case "stream.credit":
            return;
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

const native: KeymapNative = {
  syncMenu: async () => {},
  onMenuAction: () => () => {},
  openSettings: async () => {},
  closeWindow: async () => {},
};

let root: Root | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
  );
  vi.spyOn(console, "info").mockImplementation(() => {});
  fakeTreeLayout(600);
  // 팝업의 결과 목록(listbox)도 가상화한다 — 트리처럼 보이는 높이를 준다.
  for (const prop of ["offsetHeight", "clientHeight"] as const)
    vi.spyOn(HTMLElement.prototype, prop, "get").mockImplementation(function (this: HTMLElement) {
      return ["tree", "listbox"].includes(this.getAttribute("role") ?? "") ? 600 : 0;
    });
  window.history.replaceState(null, "", "/?window=project&project=p1");
  host.files = new Map([
    ["/work/demo/src/lib.rs", "fn main() {}\n// TODO: one\nlet x = 1;\n// TODO two TODO\n"],
    ["/work/demo/src/util.rs", "// TODO util\n"],
  ]);
  host.dirs = new Map([
    ["/work/demo", ["src"]],
    ["/work/demo/src", ["lib.rs", "util.rs"]],
  ]);
  host.searches = [];
  host.cancelled = [];
  host.calls = [];
  activeWorktreeStore.setState({ top: undefined });
  resetWindowFileTree();
  resetFindWindow();
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

const openPopup = () => press({ code: "KeyF", key: "F", metaKey: true, shiftKey: true });
const popup = () => document.querySelector<HTMLElement>("[data-find-popup]");
const queryInput = () => popup()!.querySelector<HTMLInputElement>('input[aria-label="찾을 글자"]')!;

/** React 가 듣는 input 이벤트로 칸의 값을 바꾼다. */
async function type(input: HTMLInputElement, value: string) {
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await settle();
}

async function click(el: HTMLElement, type: "click" | "dblclick" | "contextmenu" = "click") {
  await act(async () => {
    el.dispatchEvent(
      new MouseEvent(type, {
        bubbles: true,
        cancelable: true,
        button: type === "contextmenu" ? 2 : 0,
      }),
    );
  });
  await settle();
}

const toggle = (label: string) =>
  popup()!.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!;

function result(path: string, lines: [number, string, number][]): SearchFile {
  return {
    path,
    lines: lines.map(([line, preview, start]) => ({
      line,
      preview,
      previewStart: 0,
      ranges: [{ start, end: start + 4 }],
      more: false,
    })),
  };
}

async function chunk(streamId: number, data: SearchTextChunk) {
  await act(async () => host.send({ method: "stream.chunk", params: { streamId, data } }));
  await settle();
}

async function finish(
  streamId: number,
  done: Partial<Extract<SearchTextChunk, { kind: "done" }>> = {},
) {
  await chunk(streamId, {
    kind: "done",
    matchedFiles: 0,
    lines: 0,
    truncated: false,
    searchedFiles: 0,
    binaryFiles: 0,
    largeFiles: 0,
    ...done,
  });
  await act(async () =>
    host.send({ method: "stream.end", params: { streamId, status: "completed" } }),
  );
  await settle();
}

/** 팝업을 열고 `query` 를 넣어 검색이 Host 에 닿을 때까지 기다린다. 그 검색을 돌려준다. */
async function search(query: string) {
  if (!popup()) await openPopup();
  const before = host.searches.length;
  await type(queryInput(), query);
  await vi.waitFor(() => expect(host.searches.length).toBe(before + 1));
  return host.searches.at(-1)!;
}

const rowTexts = () =>
  [...popup()!.querySelectorAll<HTMLElement>("[data-find-row]")].map((r) =>
    r.getAttribute("data-find-row"),
  );

it("sends the popup's regex, case and *.rs mask options as the search.text params", async () => {
  await mount();
  await openPopup();
  expect(popup()).not.toBeNull();
  expect(document.activeElement).toBe(queryInput());
  await click(toggle("정규식"));
  await click(toggle("대소문자 구분"));
  await type(popup()!.querySelector<HTMLInputElement>('input[aria-label="파일 마스크"]')!, "*.rs");
  const { params } = await search("TODO");
  expect(params).toEqual({
    projectId: "p1",
    worktree: null,
    paths: ["/work/demo"],
    query: "TODO",
    regex: true,
    matchCase: true,
    wholeWord: false,
    include: ["*.rs"],
    exclude: [],
    respectIgnore: true,
    excludeVendored: false,
    excludeGenerated: false,
    hidden: false,
  } satisfies SearchTextParams);
});

it("ends the previous Host stream when the input changes", async () => {
  await mount();
  const first = await search("TOD");
  expect(host.cancelled).toEqual([]);
  // 바꾼 즉시 닫는다 — 다음 검색이 시작되기(SEARCH_DELAY_MS) 전에.
  await act(async () => {
    const input = queryInput();
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, "TODO");
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await vi.waitFor(() => expect(host.cancelled).toEqual([first.streamId]), {
    timeout: 50,
    interval: 2,
  });
  await vi.waitFor(() => expect(host.searches).toHaveLength(2));
  expect(host.searches[1]!.params.query).toBe("TODO");
  // 늦게 온 이전 검색의 청크는 목록에 들지 않는다.
  await chunk(first.streamId, {
    kind: "files",
    files: [result("/work/demo/old.rs", [[1, "TOD", 0]])],
  });
  await chunk(host.searches[1]!.streamId, {
    kind: "files",
    files: [result("/work/demo/src/util.rs", [[1, "// TODO util", 3]])],
  });
  expect(rowTexts()).toEqual(["/work/demo/src/util.rs:1"]);
});

it("stops the search when the popup closes", async () => {
  await mount();
  const { streamId } = await search("TODO");
  await press({ code: "Escape", key: "Escape" });
  expect(popup()).toBeNull();
  expect(host.cancelled).toEqual([streamId]);
});

it("shows 「더 있음」 when the Host says there are more results than the limit", async () => {
  await mount();
  const { streamId } = await search("TODO");
  await chunk(streamId, {
    kind: "files",
    files: [result("/work/demo/src/lib.rs", [[2, "// TODO: one", 3]])],
  });
  expect(popup()!.querySelector("[data-testid=find-more]")).toBeNull();
  await finish(streamId, { matchedFiles: 1, lines: 10_000, truncated: true });
  expect(popup()!.querySelector("[data-testid=find-more]")?.textContent).toBe("더 있음");
  expect(popup()!.querySelector("[data-testid=find-status]")?.textContent).toContain(
    "1 줄 · 1 파일",
  );
});

it("opens the selected result at its line and column on Enter", async () => {
  await mount();
  const { streamId } = await search("TODO");
  await chunk(streamId, {
    kind: "files",
    files: [
      result("/work/demo/src/util.rs", [[1, "// TODO util", 3]]),
      result("/work/demo/src/lib.rs", [
        [2, "// TODO: one", 3],
        [4, "// TODO two TODO", 3],
      ]),
    ],
  });
  await finish(streamId, { matchedFiles: 2, lines: 3 });
  expect(rowTexts()).toEqual([
    "/work/demo/src/util.rs:1",
    "/work/demo/src/lib.rs:2",
    "/work/demo/src/lib.rs:4",
  ]);
  await press({ code: "ArrowDown", key: "ArrowDown" });
  await press({ code: "ArrowDown", key: "ArrowDown" });
  expect(popup()!.querySelector("[aria-selected=true]")?.getAttribute("data-find-row")).toBe(
    "/work/demo/src/lib.rs:4",
  );
  await press({ code: "Enter", key: "Enter" });
  expect(popup()).toBeNull();
  await vi.waitFor(() => expect(document.querySelectorAll(".cm-editor")).toHaveLength(1));
  const view = EditorView.findFromDOM(document.querySelector<HTMLElement>(".cm-editor")!)!;
  await vi.waitFor(() => {
    const head = view.state.selection.main.head;
    const line = view.state.doc.lineAt(head);
    expect([line.number, head - line.from + 1]).toEqual([4, 4]);
  });
  expect(host.calls.find((c) => c.method === "fs.stat")?.params).toMatchObject({
    path: "/work/demo/src/lib.rs",
    worktree: null,
  });
});

it("shows the selected file in the read-only preview with its matches marked", async () => {
  await mount();
  const { streamId } = await search("TODO");
  await chunk(streamId, {
    kind: "files",
    files: [
      result("/work/demo/src/lib.rs", [
        [2, "// TODO: one", 3],
        [4, "// TODO two TODO", 3],
      ]),
    ],
  });
  const preview = () =>
    popup()!.querySelector<HTMLElement>("[data-testid=find-preview] .cm-editor")!;
  await vi.waitFor(() =>
    expect(EditorView.findFromDOM(preview())!.state.doc.toString()).toBe(
      host.files.get("/work/demo/src/lib.rs"),
    ),
  );
  await vi.waitFor(() =>
    expect([...preview().querySelectorAll(".cm-pw-find-match")].map((m) => m.textContent)).toEqual([
      "TODO",
      "TODO",
    ]),
  );
  expect(EditorView.findFromDOM(preview())!.state.readOnly).toBe(true);
});

it("pins the results to the Search Results tool window with ⌘Enter and opens a line from there", async () => {
  await mount();
  const { streamId } = await search("TODO");
  await press({ code: "Enter", key: "Enter", metaKey: true });
  expect(popup()).toBeNull();
  // 고정한 검색은 멈추지 않고 툴 윈도우에서 이어 받는다.
  expect(host.cancelled).toEqual([]);
  const panel = () => document.querySelector<HTMLElement>('[data-tool-window="searchResults"]')!;
  await vi.waitFor(() => expect(panel()).not.toBeNull());
  await chunk(streamId, {
    kind: "files",
    files: [result("/work/demo/src/lib.rs", [[2, "// TODO: one", 3]])],
  });
  await finish(streamId, { matchedFiles: 1, lines: 1 });
  const items = () =>
    [...panel().querySelectorAll<HTMLElement>("[data-search-result]")].map((r) =>
      r.getAttribute("data-search-result"),
    );
  expect(items()).toEqual(["/work/demo/src/lib.rs", "/work/demo/src/lib.rs:2"]);
  expect(panel().textContent).toContain("「TODO」");

  const tree = panel().querySelector<HTMLElement>("[role=tree]")!;
  tree.focus();
  await press({ code: "ArrowDown", key: "ArrowDown" }, tree);
  await press({ code: "ArrowDown", key: "ArrowDown" }, tree);
  await press({ code: "Enter", key: "Enter" }, tree);
  await vi.waitFor(() =>
    expect(document.querySelector("[data-editor-tab-header]")?.textContent).toBe("lib.rs"),
  );
  // ← 는 줄에서 파일로, 다시 ← 는 접는다.
  await press({ code: "ArrowLeft", key: "ArrowLeft" }, tree);
  await press({ code: "ArrowLeft", key: "ArrowLeft" }, tree);
  expect(items()).toEqual(["/work/demo/src/lib.rs"]);
});

it("searches the folder picked from the file tree's context menu", async () => {
  await mount();
  const src = document.querySelector<HTMLElement>('[data-path="/work/demo/src"]')!;
  await click(src, "contextmenu");
  const item = [...document.querySelectorAll<HTMLElement>("[role=menuitem]")].find(
    (e) => e.textContent === "이 폴더에서 찾기",
  )!;
  await click(item);
  expect(popup()).not.toBeNull();
  expect(
    popup()!.querySelector<HTMLElement>('[role=radio][aria-checked="true"]')?.textContent,
  ).toBe("폴더");
  expect(popup()!.querySelector<HTMLInputElement>('input[aria-label^="찾을 폴더"]')!.value).toBe(
    "src",
  );
  const { params } = await search("TODO");
  expect(params.paths).toEqual(["/work/demo/src"]);
});

/** 값을 바꾸되 설정(SEARCH_DELAY_MS)을 기다리지 않는다. */
const typeNoWait = (input: HTMLInputElement, value: string) =>
  act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });

it("starts the due search when ⌘Enter pins right after an edit instead of leaving it searching", async () => {
  await mount();
  const first = await search("TOD");
  await chunk(first.streamId, {
    kind: "files",
    files: [result("/work/demo/src/util.rs", [[1, "// TOD util", 3]])],
  });
  // 지연 타이머가 가기 전에 고정한다.
  await typeNoWait(queryInput(), "TODO");
  await press({ code: "Enter", key: "Enter", metaKey: true });
  expect(popup()).toBeNull();
  await vi.waitFor(() => expect(host.searches).toHaveLength(2));
  expect(host.searches[1]!.params.query).toBe("TODO");
  const panel = () => document.querySelector<HTMLElement>('[data-tool-window="searchResults"]')!;
  await vi.waitFor(() => expect(panel()).not.toBeNull());
  // 이전 질의의 줄이 새 질의 제목 아래 남지 않는다.
  expect(panel().textContent).not.toContain("TOD util");
  await chunk(host.searches[1]!.streamId, {
    kind: "files",
    files: [result("/work/demo/src/lib.rs", [[2, "// TODO: one", 3]])],
  });
  await finish(host.searches[1]!.streamId, { matchedFiles: 1, lines: 1 });
  expect(panel().textContent).toContain("「TODO」");
  expect(panel().textContent).not.toContain("찾는 중");
});

it("shows a translated message when the Host's concurrent search limit is hit", async () => {
  await mount();
  const { streamId } = await search("TODO");
  await act(async () =>
    host.send({
      method: "stream.end",
      params: {
        streamId,
        status: "failed",
        error: {
          code: -32603,
          message: "internal error",
          data: "search is busy: 2 searches are already running",
        },
      },
    }),
  );
  await settle();
  const alert = popup()!.querySelector<HTMLElement>("[role=alert]")!;
  expect(alert.textContent).toBe("다른 검색이 돌고 있어 시작하지 못했다 — 잠시 뒤 다시 찾는다");
  expect(alert.textContent).not.toContain("busy");
});

// 한글 조합 중의 Enter · Esc 는 조합에 쓰이고 결과를 열거나 팝업을 닫지 않는다(ui/ime.ts). jsdom 계열은 실제 입력기가 없어
// 이벤트 꼴(isComposing · WebKit 의 keyCode 229)로 본다 — 실제 입력기는 「사람이 확인할 것」.
it("does not open a result or close the popup on Enter / Esc during IME composition", async () => {
  await mount();
  const { streamId } = await search("한글");
  await chunk(streamId, {
    kind: "files",
    files: [result("/work/demo/src/lib.rs", [[2, "// 한글 one", 3]])],
  });
  await finish(streamId, { matchedFiles: 1, lines: 1 });
  expect(rowTexts()).toEqual(["/work/demo/src/lib.rs:2"]);
  const input = queryInput();
  await press({ code: "Enter", key: "Enter", isComposing: true }, input);
  await press({ code: "Enter", key: "Enter", keyCode: 229 }, input);
  await press({ code: "Escape", key: "Escape", isComposing: true }, input);
  await press({ code: "Escape", key: "Escape", keyCode: 229 }, input);
  expect(popup()).not.toBeNull();
  expect(document.querySelectorAll("[data-editor-tab-header]")).toHaveLength(0);
  expect(host.cancelled).toEqual([]);
  // 조합이 끝난 Enter 는 연다.
  await press({ code: "Enter", key: "Enter" }, input);
  expect(popup()).toBeNull();
  await vi.waitFor(() =>
    expect(document.querySelector("[data-editor-tab-header]")?.textContent).toBe("lib.rs"),
  );
});
