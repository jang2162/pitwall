// @vitest-environment happy-dom
// 내비게이션 기록의 웹뷰 시험(15-m4-plan M4-09 완료 기준): 프로젝트 창(App kind="project")을 가짜 Host 에 붙여 키(⌘[ · ⌘] · ⌘⇧E ·
// ⌘L · ⌘⇧O)와 마우스 4 · 5 버튼(합성 이벤트)으로 뒤로 · 앞으로를 본다. 정의 이동(M4-08)은 아직 없다 — 같은 위치로 가기
// (`navigateTo`)를 부르는 「파일로 이동」(⌘⇧O `main.rs:7`)이 그 자리를 대신한다.

import { EditorSelection, EditorState, StateEffect } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
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
  SearchFiles,
  SearchFilesParams,
} from "@pitwall/rpc-client";
import type { DockviewApi } from "dockview-react";
import { act as reactAct } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, vi } from "vitest";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../App";
import { resetWindowFileTree } from "../file-tree/project-view";
import { fakeTreeLayout } from "../file-tree/testing";
import type { KeymapNative } from "../keymap/native";
import { activeWorktreeStore } from "../open-with/active-worktree";
import { projectOpenInbox } from "../project-open";
import { act, endTest, it } from "../test-support/ended-test";
import { createThemeStore } from "../theme/theme-store";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";
import { editorPanels, tabParams } from "./editor-tabs";
import { cursorOf } from "./editor-workspace";
import { NAV_HISTORY_LIMIT } from "./navigation";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const host = vi.hoisted(() => ({
  files: new Map<string, string>(),
  states: new Map<string, unknown>(),
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
      server.onMessage((text) => {
        const m = JSON.parse(text) as { id?: number; method: string; params: never };
        const reply = (result: unknown) => send({ id: m.id, result });
        const fail = (message: string) =>
          send({ id: m.id, error: { code: -32005, message, data: null } });
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
          case "fs.list": {
            // 파일로 이동은 파일 트리의 루트(프로젝트 루트)에서 찾는다.
            const prefix = `${path}/`;
            const names = new Set(
              [...host.files.keys()]
                .filter((p) => p.startsWith(prefix))
                .map((p) => p.slice(prefix.length).split("/")[0]!),
            );
            if (names.size === 0) return fail(`not a folder: ${path}`);
            const entries: FsEntry[] = [...names].map((name) => ({
              name,
              kind: host.files.has(prefix + name) ? "file" : "dir",
              target: null,
              outside: false,
              size: 0,
              mtimeMs: 1,
              ignored: false,
            }));
            return reply({ path, entries });
          }
          case "fs.stat":
            if ([...host.files.keys()].some((p) => p.startsWith(`${path}/`)))
              return reply({
                path,
                kind: "dir",
                target: null,
                size: 0,
                mtimeMs: 1,
                readonly: false,
                sizeClass: null,
              } satisfies FsStat);
            if (!host.files.has(path)) return fail(`not found: ${path}`);
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
            if (!host.files.has(path)) return fail(`not found: ${path}`);
            return reply({
              path,
              size: host.files.get(path)!.length,
              mtimeMs: 1,
              offset: 0,
              eof: true,
              version: `1:${path}`,
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
              files: files.map(() => ({
                language: null,
                source: "extension",
                vendored: false,
                generated: false,
                documentation: false,
              })),
            } satisfies LangDetectResult);
          }
          case "search.files": {
            // 가짜 매칭: 상대 경로에 질의가 들어 있으면 맞는다.
            const params = m.params as SearchFilesParams;
            const hits = [...host.files.keys()]
              .map((p) => ({ path: p, relative: p.slice("/work/demo/".length) }))
              .filter((f) => f.relative.includes(params.query));
            return reply({
              files: hits.map((f) => ({
                path: f.path,
                root: "/work/demo",
                relative: f.relative,
                positions: [],
                score: 1,
                vendored: false,
                generated: false,
              })),
              matched: hits.length,
              indexed: host.files.size,
            } satisfies SearchFiles);
          }
          case "events.subscribe":
            return reply({ subscriptionId: next++ });
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

const native = {
  syncMenu: async () => {},
  onMenuAction: () => () => {},
  openSettings: async () => {},
  closeWindow: async () => {},
  guardClose: () => () => {},
} satisfies KeymapNative;

const LIB = "/work/demo/src/lib.rs";
const MAIN = "/work/demo/src/main.rs";

/** `n` 줄짜리 텍스트. 줄마다 그 줄 번호가 든다. */
function lines(n: number): string {
  return Array.from({ length: n }, (_, i) => `line ${i + 1} — some text`).join("\n");
}

let root: Root | undefined;
let dock: DockviewApi | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
  );
  vi.spyOn(console, "info").mockImplementation(() => {});
  // 파일로 이동은 파일 트리의 루트에서 찾는다 — 트리가 줄을 그리게 크기를 준다.
  fakeTreeLayout(600);
  resetWindowFileTree();
  window.history.replaceState(null, "", "/?window=project&project=p1");
  host.files = new Map([
    [LIB, lines(300)],
    [MAIN, lines(20)],
  ]);
  host.states = new Map();
  activeWorktreeStore.setState({ top: undefined });
  localStorage.clear();
});

afterEach(async () => {
  await endTest();
  await reactAct(async () => root?.unmount());
  root = undefined;
  dock = undefined;
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
  // 파일로 이동이 찾는 곳은 파일 트리의 루트다 — 루트를 읽을 때까지 기다린다.
  await vi.waitFor(() =>
    expect(document.querySelector('[data-path="/work/demo/src"]')).not.toBeNull(),
  );
}

/** 창이 받은 열기 요청(`pitwall <파일>:<줄>:<열>`) — 위치로 가기가 아니라 기록에 들지 않는다. */
async function openRequest(file: string, line?: number, col?: number) {
  await act(async () => projectOpenInbox.push({ file, line, col }));
  await settle();
}

async function press(init: KeyboardEventInit) {
  await act(async () => {
    const target = document.activeElement ?? document.body;
    target.dispatchEvent(
      new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }),
    );
    target.dispatchEvent(new KeyboardEvent("keyup", { bubbles: true, cancelable: true, ...init }));
  });
  await settle();
}

const back = () => press({ code: "BracketLeft", key: "[", metaKey: true });
const forward = () => press({ code: "BracketRight", key: "]", metaKey: true });
const recentLocations = () => press({ code: "KeyE", key: "E", metaKey: true, shiftKey: true });

async function mouse(type: "mousedown" | "auxclick", button: number) {
  await act(async () => {
    document.body.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true, button }));
  });
  await settle();
}

/** 활성 탭의 파일 · 커서. */
function where(): { path: string; line: number; column: number } {
  const panel = dock!.activePanel!;
  const view = EditorView.findFromDOM(
    panel.view.content.element.querySelector<HTMLElement>(".cm-editor")!,
  )!;
  return { path: tabParams(panel).path, ...cursorOf(view) };
}

function activeView(): EditorView {
  return EditorView.findFromDOM(
    dock!.activePanel!.view.content.element.querySelector<HTMLElement>(".cm-editor")!,
  )!;
}

/** 사람이 커서를 옮긴다(클릭 · 화살표 — 편집 없는 선택 바뀜). */
async function moveCursor(line: number) {
  const view = activeView();
  // 포커스된 뷰의 선택을 바꾸면 happy-dom 이 selectionchange 를 곧바로 보내 뷰가 갱신 중에 다시 갱신된다 — 포커스를 뺀다.
  (document.activeElement as HTMLElement | null)?.blur();
  await act(async () =>
    view.dispatch({ selection: { anchor: view.state.doc.line(line).from }, userEvent: "select" }),
  );
}

/** 문서 끝에 글자 하나를 붙인다 — 큰 이동의 연속을 끊는 편집(기록된 줄은 그대로). */
async function editAtEnd() {
  const view = activeView();
  (document.activeElement as HTMLElement | null)?.blur();
  await act(async () =>
    view.dispatch({
      changes: { from: view.state.doc.length, insert: "x" },
      userEvent: "input.type",
    }),
  );
}

const toasts = () =>
  [...document.querySelectorAll("[data-sonner-toast]")].map((t) => t.textContent ?? "");

/** 줄로 이동(⌘L) 대화상자에 `text` 를 넣고 이동한다. */
async function gotoLine(text: string) {
  await press({ code: "KeyL", key: "l", metaKey: true });
  const input = document.querySelector<HTMLInputElement>("[data-testid=goto-line-dialog] input")!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, text);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await press({ code: "Enter", key: "Enter" });
}

/** 파일로 이동(⌘⇧O)에 `query`(`이름:줄`)를 넣고 첫 결과를 연다. */
async function gotoFile(query: string) {
  await press({ code: "KeyO", key: "O", metaKey: true, shiftKey: true });
  const popup = () => document.querySelector<HTMLElement>("[data-search-everywhere]");
  // 칸은 그때그때 찾는다 — 팝업이 열린 뒤 다시 그려지면 앞서 찾은 칸은 문서에서 떨어져 있다.
  const input = () => popup()!.querySelector<HTMLInputElement>("[cmdk-input]")!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input(), query);
    input().dispatchEvent(new Event("input", { bubbles: true }));
  });
  await settle();
  await vi.waitFor(() => expect(popup()!.querySelector("[cmdk-item]")).not.toBeNull());
  await press({ code: "Enter", key: "Enter" });
  await vi.waitFor(() => expect(popup()).toBeNull());
  await settle();
}

const recentRows = () =>
  [...document.querySelectorAll<HTMLElement>("[data-testid=recent-locations] [role=option]")].map(
    (o) => o.textContent,
  );

it("goes back to where a jump to another file left and forward to the same place again", async () => {
  await mount();
  await openRequest(LIB, 50, 3);
  await gotoFile("main.rs:7");
  await vi.waitFor(() => expect(where()).toEqual({ path: MAIN, line: 7, column: 1 }));

  await back();
  await vi.waitFor(() => expect(where()).toEqual({ path: LIB, line: 50, column: 3 }));
  await forward();
  await vi.waitFor(() => expect(where()).toEqual({ path: MAIN, line: 7, column: 1 }));
  // 처음 연 요청(CLI)은 기록하지 않는다 — 더 뒤로 갈 곳이 없다.
  await back();
  await back();
  await vi.waitFor(() => expect(where()).toEqual({ path: LIB, line: 50, column: 3 }));
});

it("records ⌘L and moves the recorded place with an edit that deletes lines above it", async () => {
  await mount();
  await openRequest(LIB, 100, 1);
  // 열 줄 안쪽의 ⌘L 도 기록한다 — 큰 이동(10줄 넘게)이 아니어도 위치로 가기라서.
  await gotoLine("105");
  expect(where()).toMatchObject({ path: LIB, line: 105 });
  await back();
  await vi.waitFor(() => expect(where()).toMatchObject({ path: LIB, line: 100 }));
  await forward();
  await vi.waitFor(() => expect(where()).toMatchObject({ path: LIB, line: 105 }));
  await gotoLine("250");
  expect(where()).toMatchObject({ path: LIB, line: 250 });

  // 1~10줄을 지운다 — 기록된 105줄의 글자는 95줄로 올라간다.
  const view = activeView();
  await act(async () =>
    view.dispatch({
      changes: { from: 0, to: view.state.doc.line(11).from },
      userEvent: "delete",
    }),
  );
  await back();
  await vi.waitFor(() => expect(where()).toMatchObject({ path: LIB, line: 95 }));
  expect(activeView().state.doc.line(95).text).toBe("line 105 — some text");
  // 앞으로는 ⌘L 로 간 자리(그사이 열 줄 위로)다.
  await forward();
  await vi.waitFor(() => expect(where()).toMatchObject({ path: LIB, line: 240 }));
});

it("opens a closed file again when going back to it", async () => {
  await mount();
  await openRequest(LIB, 40, 1);
  await gotoFile("main.rs:5");
  await vi.waitFor(() => expect(where().path).toBe(MAIN));
  const lib = editorPanels(dock!).find((p) => tabParams(p).path === LIB)!;
  await act(async () => lib.api.close());
  await settle();
  expect(editorPanels(dock!).map((p) => tabParams(p).path)).toEqual([MAIN]);

  await back();
  await vi.waitFor(() => expect(where()).toEqual({ path: LIB, line: 40, column: 1 }));
});

it("records a cursor move of more than 10 lines, not a smaller one nor a move by an edit", async () => {
  await mount();
  await openRequest(LIB, 100, 1);
  await moveCursor(110);
  await moveCursor(120);
  // 열 줄 이하의 이동 둘 — 기록 없음.
  await recentLocations();
  expect(recentRows()).toEqual([]);
  await press({ code: "Escape", key: "Escape" });

  // 편집이 커서를 스무 줄 민다(붙여 넣기) — 기록 없음.
  const view = activeView();
  const at = view.state.doc.line(120).from;
  const paste = "x\n".repeat(20);
  await act(async () =>
    view.dispatch({
      changes: { from: at, insert: paste },
      selection: { anchor: at + paste.length },
      userEvent: "input.paste",
    }),
  );
  expect(where().line).toBe(140);
  // 열한 줄 — 기록한다.
  await moveCursor(151);
  await recentLocations();
  expect(recentRows()).toHaveLength(1);
  expect(recentRows()[0]).toContain("lib.rs:140");
  await press({ code: "Escape", key: "Escape" });
  await back();
  await vi.waitFor(() => expect(where()).toMatchObject({ path: LIB, line: 140 }));
});

it(`keeps the last ${NAV_HISTORY_LIMIT} places in Recent Locations, newest first, and opens the chosen one`, async () => {
  await mount();
  await openRequest(LIB, 299, 1);
  // 큰 이동 101번: 첫 기록(299줄)이 빠진다. 이어지는 큰 이동은 한 칸이라 이동마다 편집으로 연속을 끊는다.
  for (let i = 0; i < NAV_HISTORY_LIMIT + 1; i++) {
    await moveCursor(i % 2 ? 1 + (i % 40) : 150 + (i % 40));
    await editAtEnd();
  }
  await recentLocations();
  const rows = recentRows();
  expect(rows).toHaveLength(NAV_HISTORY_LIMIT);
  expect(rows.some((r) => r?.includes("lib.rs:299"))).toBe(false);
  // 가장 최근 기록은 마지막 이동 전의 자리(i = 99 → 1 + 99 % 40 = 20줄)다.
  expect(rows[0]).toContain("lib.rs:20");
  expect(rows[0]).toContain("line 20 — some text");

  // ↓ 한 번 · ↩ — 둘째 것(i = 98 → 150 + 18 = 168줄)으로 간다.
  await press({ code: "ArrowDown", key: "ArrowDown" });
  await press({ code: "Enter", key: "Enter" });
  expect(document.querySelector("[data-testid=recent-locations]")).toBeNull();
  await vi.waitFor(() => expect(where()).toMatchObject({ path: LIB, line: 168 }));
});

it("goes back on mouse button 4 and forward on button 5, once per press", async () => {
  await mount();
  await openRequest(LIB, 50, 1);
  await gotoFile("main.rs:7");
  await vi.waitFor(() => expect(where().path).toBe(MAIN));
  await gotoLine("18");
  // 기록: LIB 50 · MAIN 7. 한 누름이 두 번 돌면 한 칸을 더 간다.

  // mousedown 과 그 누름의 auxclick 이 함께 와도 한 번이다.
  await mouse("mousedown", 3);
  await mouse("auxclick", 3);
  await vi.waitFor(() => expect(where()).toMatchObject({ path: MAIN, line: 7 }));
  await settle();
  expect(where()).toMatchObject({ path: MAIN, line: 7 });
  // auxclick 만 오는 웹뷰.
  await mouse("auxclick", 3);
  await vi.waitFor(() => expect(where()).toMatchObject({ path: LIB, line: 50 }));
  await mouse("mousedown", 4);
  await mouse("auxclick", 4);
  await vi.waitFor(() => expect(where()).toMatchObject({ path: MAIN, line: 7 }));
  await settle();
  expect(where()).toMatchObject({ path: MAIN, line: 7 });
  await mouse("mousedown", 3);
  await mouse("auxclick", 3);
  await vi.waitFor(() => expect(where()).toMatchObject({ path: LIB, line: 50 }));
  // 다른 버튼은 아무것도 하지 않는다.
  await mouse("mousedown", 1);
  await mouse("auxclick", 1);
  expect(where()).toMatchObject({ path: LIB, line: 50 });
});

it("keeps one slot for a run of big moves (PageDown pressed again and again) and goes back to its start", async () => {
  await mount();
  await openRequest(LIB, 100, 1);
  for (const line of [150, 200, 250]) await moveCursor(line);
  await recentLocations();
  expect(recentRows()).toHaveLength(1);
  expect(recentRows()[0]).toContain("lib.rs:100");
  await press({ code: "Escape", key: "Escape" });
  await back();
  await vi.waitFor(() => expect(where()).toMatchObject({ path: LIB, line: 100 }));
});

it("does not record adding or removing cursors, and ⌘[ leaves the cursors alone", async () => {
  await mount();
  await openRequest(LIB, 100, 1);
  const view = activeView();
  const at = (line: number) => view.state.doc.line(line).from;
  (document.activeElement as HTMLElement | null)?.blur();
  // 여러 커서는 M4-14 가 켠다 — 이 브랜치에서는 시험 뷰에만 켠다.
  await act(async () =>
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.allowMultipleSelections.of(true)),
    }),
  );
  // ⌥⇧+클릭처럼 20줄 아래에 커서를 더한다 — 새 커서가 주 범위다.
  await act(async () =>
    view.dispatch({
      selection: EditorSelection.create(
        [EditorSelection.cursor(at(100)), EditorSelection.cursor(at(120))],
        1,
      ),
      userEvent: "select.pointer",
    }),
  );
  await back();
  expect(activeView().state.selection.ranges).toHaveLength(2);
  // 더한 커서를 뺀다(남은 커서는 20줄 위).
  await act(async () =>
    view.dispatch({ selection: EditorSelection.single(at(100)), userEvent: "select" }),
  );
  await recentLocations();
  expect(recentRows()).toEqual([]);
});

it("takes the focus with Recent Locations, so keys do not edit the document under it", async () => {
  await mount();
  await openRequest(LIB, 100, 1);
  await gotoLine("150");
  const text = activeView().state.doc.toString();
  await recentLocations();
  const list = document.querySelector("[data-testid=recent-locations]");
  expect(document.activeElement).toBe(list);
  await press({ code: "Backspace", key: "Backspace" });
  expect(activeView().state.doc.toString()).toBe(text);
  // 한글 조합을 끝낸 ↩(keyCode 229)는 고르지 않는다.
  await press({ code: "Enter", key: "Enter", keyCode: 229 } as KeyboardEventInit);
  expect(document.querySelector("[data-testid=recent-locations]")).not.toBeNull();
  await press({ code: "Escape", key: "Escape" });
  expect(document.querySelector("[data-testid=recent-locations]")).toBeNull();
  await vi.waitFor(() =>
    expect(document.activeElement?.classList.contains("cm-content")).toBe(true),
  );
  expect(where()).toMatchObject({ path: LIB, line: 150 });
});

it("skips a deleted file's place when going back without an error notice", async () => {
  await mount();
  await openRequest(LIB, 40, 1);
  await gotoFile("main.rs:5");
  await vi.waitFor(() => expect(where().path).toBe(MAIN));
  const lib = editorPanels(dock!).find((p) => tabParams(p).path === LIB)!;
  await act(async () => lib.api.close());
  host.files.delete(LIB);
  await back();
  await settle();
  expect(where()).toMatchObject({ path: MAIN, line: 5 });
  expect(toasts().filter((t) => t.includes("lib.rs"))).toEqual([]);
});

it("ignores the mouse back button while a popup of the window is open", async () => {
  await mount();
  await openRequest(LIB, 50, 1);
  await gotoFile("main.rs:7");
  await vi.waitFor(() => expect(where().path).toBe(MAIN));
  await recentLocations();
  await mouse("mousedown", 3);
  await mouse("auxclick", 3);
  expect(where()).toMatchObject({ path: MAIN, line: 7 });
  expect(document.querySelector("[data-testid=recent-locations]")).not.toBeNull();
});
