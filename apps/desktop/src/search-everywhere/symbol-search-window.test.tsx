// @vitest-environment happy-dom
// 심볼로 이동(⌘⌥O) · 클래스로 이동(⌘O) · Search Everywhere 의 심볼 탭(15-m4-plan M4-16)을 프로젝트 창(App kind="project")과 가짜
// Host 로 본다: 인덱스(`index.symbols.search`)와 서버(`lsp.workspaceSymbol`)를 합친 목록(겹침 제거 · 서버 먼저), 서버가 늦으면 인덱스
// 결과가 먼저, 클래스로 이동이 함수를 뺌, 고르면 위치로 가기(기록 — ⌘[ 로 돌아온다), 루트 밖 결과는 토큰 길(외부 소스 탭), 취소된
// 파일 목록의 다시 만들기, 「바쁨」 재시도, 서버가 준 이름은 글자로만, 팝업 Esc 가 멀티 커서를 접지 않음, 키 입력당 시간(50만 심볼).
// 창 틀(가짜 Host · 마운트)은 search-everywhere.test.tsx 와 같다.

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
  IndexSymbolMatch,
  IndexSymbolSearchParams,
  LspWorkspaceSymbol,
  LspWorkspaceSymbolParams,
  SearchFiles,
  SearchFilesParams,
} from "@pitwall/rpc-client";
import { EditorSelection } from "@codemirror/state";
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
import { FIRST_ROWS } from "./symbol-tab";

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
  /** 켜면 `search.files` 가 Host 의 「바쁨」 을 답한다. */
  busy: false,
  /** 사라진 워크트리 꼭대기. 이것을 실은 `fs.*` 는 Host 처럼 거절한다(crates/pw-fs/src/lib.rs `worktree_root`). */
  goneWorktrees: new Set<string>(),
  send: (_m: object) => {},
  /** 가짜 심볼 인덱스(`index.symbols.search` 가 이름에 질의 글자가 차례로 든 것을 고른다). */
  symbols: [] as IndexSymbolMatch[],
  /** 받은 `index.symbols.search` 인자. */
  symbolSearches: [] as IndexSymbolSearchParams[],
  /** 켜면 `index.symbols.search` 가 이 횟수만큼 「바쁨」 을 답한다. */
  symbolBusy: 0,
  /** 있으면 `index.symbols.search` 의 답에 이 루트를 `unindexed` 로 싣고 `index.status` 가 `suspended` 라 답한다. */
  suspended: undefined as string | undefined,
  /** `index.symbols.search` 를 계산한 시간(ms) — 성능 시험이 웹뷰 쪽 시간에서 뺀다. */
  indexMs: [] as number[],
  /** `index.symbols.search` 를 받은 시각. */
  indexAt: [] as number[],
  /** 서버의 답(질의 → 심볼). 없으면 서버가 없다(`servers: []`). */
  lsp: undefined as ((query: string) => LspWorkspaceSymbol[]) | undefined,
  /** 서버가 답하기까지(ms). */
  lspDelay: 0,
  lspSearches: [] as LspWorkspaceSymbolParams[],
  /** 루트 밖 토큰 → 글(`lsp.document.read`). */
  outside: new Map<string, string>(),
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
        if (m.method.startsWith("fs.") || m.method === "lsp.document.read")
          host.calls.push({ method: m.method, params: m.params });
        const path = (m.params as { path?: string } | undefined)?.path ?? "";
        const worktree = (m.params as { worktree?: string | null } | undefined)?.worktree;
        if (m.method.startsWith("fs.") && worktree && host.goneWorktrees.has(worktree))
          return send({
            id: m.id,
            error: {
              code: -32602,
              message: "invalid params",
              data: `\`${worktree}\` is not a worktree of project \`p1\``,
            },
          });
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
                lossy: false,
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
          case "index.symbols.search": {
            const params = m.params as IndexSymbolSearchParams;
            host.symbolSearches.push(params);
            if (host.symbolBusy > 0) {
              host.symbolBusy--;
              return send({
                id: m.id,
                error: {
                  code: -32603,
                  message: "internal error",
                  data: "the symbol index is busy: 4 requests are already running",
                },
              });
            }
            const started = performance.now();
            host.indexAt.push(started);
            const q = params.query.toLowerCase();
            const kinds = params.kinds ?? null;
            const limit = params.limit ?? 100;
            const hits: IndexSymbolMatch[] = [];
            let matched = 0;
            for (const s of host.symbols) {
              if (kinds && !kinds.includes(s.kind)) continue;
              const name = s.name.toLowerCase();
              let at = 0;
              for (const ch of q) {
                at = name.indexOf(ch, at);
                if (at < 0) break;
                at++;
              }
              if (at < 0) continue;
              matched++;
              if (hits.length < limit) hits.push(s);
            }
            host.indexMs.push(performance.now() - started);
            return reply({
              symbols: host.suspended ? [] : hits,
              matched: host.suspended ? 0 : matched,
              indexed: host.symbols.length,
              unindexed: host.suspended ? [host.suspended] : [],
            });
          }
          case "index.status":
            return reply({
              roots: [
                {
                  root: "/work/demo",
                  state: host.suspended ? "suspended" : "ready",
                  files: 1,
                  symbols: host.symbols.length,
                  pending: 0,
                },
              ],
            });
          case "lsp.workspaceSymbol": {
            const params = m.params as LspWorkspaceSymbolParams;
            host.lspSearches.push(params);
            const answer = host.lsp;
            if (!answer) return reply({ symbols: [], servers: [], failed: [], truncated: false });
            await new Promise((r) => setTimeout(r, host.lspDelay));
            return reply({
              symbols: answer(params.query),
              servers: ["rust-analyzer"],
              failed: [],
              truncated: false,
            });
          }
          case "lsp.document.read": {
            const { token } = m.params as { token: string };
            const text = host.outside.get(token);
            if (text === undefined) return fail("no such document token");
            return reply({
              text,
              uri: `file:///rust/${token}.rs`,
              path: `/rust/lib/${token}.rs`,
              server: "rust-analyzer",
            });
          }
          case "search.files": {
            // 가짜 매칭: 상대 경로에 질의 글자가 차례로 들어 있으면 맞는다. `ranked` 의 것을 앞에 둔다.
            const params = m.params as SearchFilesParams;
            host.fileSearches.push(params);
            // Host 의 바쁨(crates/pw-fs/src/index/mod.rs `turn`): RpcError::internal — 사유는 message 가 아니라 data 에 있다.
            if (host.busy)
              return send({
                id: m.id,
                error: {
                  code: -32603,
                  message: "internal error",
                  data: "file search is busy: 4 searches are already running",
                },
              });
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
  host.busy = false;
  host.symbols = [];
  host.symbolSearches = [];
  host.symbolBusy = 0;
  host.suspended = undefined;
  host.indexMs = [];
  host.indexAt = [];
  host.lsp = undefined;
  host.lspDelay = 0;
  host.lspSearches = [];
  host.outside = new Map();
  host.goneWorktrees = new Set();
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

const popup = () => document.querySelector<HTMLElement>("[data-search-everywhere]");
const input = () => popup()!.querySelector<HTMLInputElement>("[cmdk-input]")!;
const items = () => [...popup()!.querySelectorAll<HTMLElement>("[cmdk-item]")];
const values = () => items().map((i) => i.getAttribute("data-value"));

/** React 가 듣는 input 이벤트로 칸의 값을 바꾼다. */
async function type(value: string) {
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input(), value);
    input().dispatchEvent(new Event("input", { bubbles: true }));
  });
  await settle();
}

async function openTab(path: string, worktree?: string) {
  await act(async () => projectOpenInbox.push({ file: path, ...(worktree && { worktree }) }));
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

// ---- 심볼 ------------------------------------------------------------------------------------------------------------------------

function sym(
  name: string,
  kind: IndexSymbolMatch["kind"],
  file: string,
  line: number,
  container: string | null = null,
): IndexSymbolMatch {
  const at = {
    start: { line: line - 1, character: 3 },
    end: { line: line - 1, character: 3 + name.length },
  };
  return {
    name,
    kind,
    container,
    path: `/work/demo/${file}`,
    root: "/work/demo",
    relative: file,
    language: "Rust",
    range: at,
    selectionRange: at,
    positions: [],
    score: 1,
  };
}

function lspSym(
  name: string,
  kind: number,
  file: string | null,
  line: number,
  extra: { container?: string; token?: string; path?: string } = {},
): LspWorkspaceSymbol {
  return {
    name,
    kind,
    container: extra.container ?? null,
    server: "rust-analyzer",
    language: "Rust",
    relative: file,
    location: {
      path: extra.path ?? `/work/demo/${file}`,
      token: extra.token ?? null,
      line,
      column: 4,
      endLine: line,
      endColumn: 4 + name.length,
      preview: null,
    },
  };
}

const gotoSymbol = () => press({ code: "KeyO", key: "ø", metaKey: true, altKey: true });
const gotoClass = () => press({ code: "KeyO", key: "o", metaKey: true });
const rows = () =>
  items()
    .filter((i) => i.hasAttribute("data-symbol-source"))
    .map(
      (i) =>
        `${i.getAttribute("data-symbol-source")}:${i.querySelector("[data-symbol-name]")!.textContent}`,
    );
/** 고른 줄(`source:이름`). */
const selectedRow = () => {
  const item = popup()!.querySelector<HTMLElement>('[cmdk-item][aria-selected="true"]');
  return (
    item &&
    `${item.getAttribute("data-symbol-source")}:${item.querySelector("[data-symbol-name]")?.textContent}`
  );
};
const selectedTab = () => popup()!.querySelector("[role=tab][aria-selected=true]")?.textContent;
const notes = () =>
  [...popup()!.querySelectorAll("[role=status],[role=alert]")].map((n) => n.textContent);

it("merges the index and the server with ⌘⌥O — one row per symbol, server first — and goes there, ⌘[ back", async () => {
  host.symbols = [
    sym("Widget", "struct", "src/lib.rs", 1),
    sym("widget_id", "function", "src/main.rs", 5),
  ];
  host.lsp = () => [
    lspSym("Widget", 23, "src/lib.rs", 1),
    lspSym("WidgetKind", 10, "src/main.rs", 12, { container: "<img src=x onerror=alert(1)>" }),
  ];
  await mount();
  await openTab("/work/demo/src/main.rs");
  await gotoSymbol();
  expect(popup()?.getAttribute("data-search-everywhere")).toBe("symbols");
  expect(selectedTab()).toBe("심볼");
  await type("wid");
  await vi.waitFor(() =>
    expect(rows()).toEqual(["lsp:Widget", "lsp:WidgetKind", "index:widget_id"]),
  );
  expect(host.symbolSearches.at(-1)).toEqual({
    projectId: "p1",
    worktree: null,
    query: "wid",
    kinds: null,
    limit: 50,
    offset: null,
  });
  expect(host.lspSearches.at(-1)).toEqual({
    projectId: "p1",
    worktree: null,
    query: "wid",
    kinds: null,
    limit: 50,
  });
  // 서버가 준 컨테이너는 글자로만 그린다.
  expect(popup()!.querySelector("img")).toBeNull();
  expect(items()[1]!.querySelector("[data-symbol-container]")!.textContent).toBe(
    "<img src=x onerror=alert(1)>",
  );
  expect(items()[0]!.textContent).toContain("구조체");
  expect(items()[0]!.textContent).toContain("lib.rs · 1줄");
  await press({ code: "ArrowDown", key: "ArrowDown" });
  await press({ code: "Enter", key: "Enter" });
  expect(popup()).toBeNull();
  await vi.waitFor(() => expect(editorLine()).toEqual([12, 4]));
  // 위치로 가기라 떠난 자리가 기록에 들었다.
  await press(
    { code: "BracketLeft", key: "[", metaKey: true },
    document.querySelector(".cm-content"),
  );
  await vi.waitFor(() => expect(editorLine()).toEqual([1, 1]));
});

it("shows only types with ⌘O, keeps the symbols tab lit and asks both sources for type kinds", async () => {
  host.symbols = [
    sym("make_widget", "function", "src/lib.rs", 2),
    sym("Widget", "struct", "src/lib.rs", 1),
    sym("WidgetTrait", "trait", "src/lib.rs", 8),
  ];
  host.lsp = () => [
    lspSym("widget_new", 12, "src/lib.rs", 4),
    lspSym("WidgetKind", 10, "src/main.rs", 12),
  ];
  await mount();
  await gotoClass();
  expect(popup()?.getAttribute("data-search-everywhere")).toBe("classes");
  expect(popup()!.getAttribute("aria-label")).toBe("클래스로 이동");
  expect(selectedTab()).toBe("심볼");
  await type("wid");
  await vi.waitFor(() =>
    expect(rows()).toEqual(["lsp:WidgetKind", "index:Widget", "index:WidgetTrait"]),
  );
  expect(host.symbolSearches.at(-1)!.kinds).toEqual([
    "class",
    "struct",
    "interface",
    "trait",
    "enum",
    "typeAlias",
  ]);
  expect(host.lspSearches.at(-1)!.kinds).toEqual([5, 10, 11, 23, 26]);
  // Tab 은 심볼 탭 다음(액션)으로 간다.
  await press({ code: "Tab", key: "Tab" }, input());
  expect(popup()?.getAttribute("data-search-everywhere")).toBe("actions");
});

it("draws the index results first when the server is slow and puts the server's on top when they come", async () => {
  host.symbols = [
    sym("Widget", "struct", "src/lib.rs", 1),
    sym("wide", "function", "src/lib.rs", 3),
  ];
  host.lsp = () => [
    lspSym("WidgetKind", 10, "src/main.rs", 12),
    lspSym("Widget", 23, "src/lib.rs", 1),
  ];
  host.lspDelay = 800;
  await mount();
  await gotoSymbol();
  await type("wid");
  await vi.waitFor(() => expect(rows()).toEqual(["index:Widget", "index:wide"]));
  expect(notes()).toContain("언어 서버의 결과를 기다리는 중…");
  expect(selectedRow()).toBe("index:Widget");
  await vi.waitFor(() => expect(rows()).toEqual(["lsp:WidgetKind", "lsp:Widget", "index:wide"]), {
    timeout: 3000,
  });
  expect(notes()).not.toContain("언어 서버의 결과를 기다리는 중…");
  // 첫 줄에서 옮기지 않았으면 새 첫 줄을 고른다.
  expect(selectedRow()).toBe("lsp:WidgetKind");
});

it("keeps the row the user moved to when the server's results come late on top", async () => {
  host.symbols = [
    sym("Widget", "struct", "src/lib.rs", 1),
    sym("wide", "function", "src/main.rs", 7),
  ];
  host.lsp = () => [
    lspSym("WidgetKind", 10, "src/main.rs", 12),
    lspSym("Widget", 23, "src/lib.rs", 1),
  ];
  host.lspDelay = 800;
  await mount();
  await gotoSymbol();
  await type("wid");
  await vi.waitFor(() => expect(rows()).toEqual(["index:Widget", "index:wide"]));
  await press({ code: "ArrowDown", key: "ArrowDown" });
  expect(selectedRow()).toBe("index:wide");
  await vi.waitFor(() => expect(rows()).toEqual(["lsp:WidgetKind", "lsp:Widget", "index:wide"]), {
    timeout: 3000,
  });
  // 서버 결과가 위에 붙어도 고른 줄은 그대로 — Enter 가 옮겨 둔 심볼을 연다.
  expect(selectedRow()).toBe("index:wide");
  await press({ code: "Enter", key: "Enter" });
  expect(popup()).toBeNull();
  await vi.waitFor(() => expect(editorLine()).toEqual([7, 4]));
});

it("opens a library symbol outside the root read-only through its token", async () => {
  const VEC = "/rust/library/alloc/src/vec/mod.rs";
  host.lsp = () => [lspSym("Vec", 23, null, 2, { token: "tok-vec", path: VEC })];
  host.outside.set("tok-vec", "// vec\npub struct Vec;\n");
  await mount();
  await gotoSymbol();
  await type("vec");
  await vi.waitFor(() => expect(rows()).toEqual(["lsp:Vec"]));
  expect(items()[0]!.textContent).toContain("외부");
  await press({ code: "Enter", key: "Enter" });
  await vi.waitFor(() =>
    expect(document.querySelector('[data-outside-source="tok-vec"]')).not.toBeNull(),
  );
  expect(host.calls.filter((c) => c.method === "lsp.document.read").map((c) => c.params)).toEqual([
    { token: "tok-vec" },
  ]);
  // 루트 안 경로로 읽지 않는다.
  expect(
    host.calls.filter(
      (c) => c.method.startsWith("fs.") && (c.params as { path?: string }).path === VEC,
    ),
  ).toEqual([]);
  const view = EditorView.findFromDOM(
    document.querySelector<HTMLElement>('[data-outside-source="tok-vec"] .cm-editor')!,
  )!;
  expect(view.state.readOnly).toBe(true);
  await vi.waitFor(() =>
    expect(view.state.doc.lineAt(view.state.selection.main.head).number).toBe(2),
  );
});

it("says the file list was cancelled and rebuilds it through the file search", async () => {
  host.symbols = [sym("Widget", "struct", "src/lib.rs", 1)];
  host.suspended = "/work/demo";
  await mount();
  await gotoSymbol();
  await type("wid");
  await vi.waitFor(() =>
    expect(popup()!.querySelector("[data-symbol-rebuild]")?.textContent).toBe(
      "파일 목록이 취소돼 심볼을 찾지 못했다다시 만들기",
    ),
  );
  expect(rows()).toEqual([]);
  host.suspended = undefined;
  const before = host.symbolSearches.length;
  await press({ code: "Enter", key: "Enter" });
  await vi.waitFor(() => expect(rows()).toEqual(["index:Widget"]));
  expect(host.fileSearches.at(-1)).toEqual({
    projectId: "p1",
    worktree: null,
    query: "",
    limit: 1,
  });
  expect(host.symbolSearches.length).toBeGreaterThan(before);
  // 팝업은 열린 채다.
  expect(popup()).not.toBeNull();
});

it("keeps the results while the symbol index is busy and asks again", async () => {
  host.symbols = [
    sym("Widget", "struct", "src/lib.rs", 1),
    sym("wide", "function", "src/lib.rs", 3),
  ];
  await mount();
  await gotoSymbol();
  await type("w");
  await vi.waitFor(() => expect(rows()).toEqual(["index:Widget", "index:wide"]));
  host.symbolBusy = 1;
  await type("wid");
  await vi.waitFor(() => expect(notes()).toContain("심볼 인덱스가 바쁘다 — 잠시 뒤 다시 찾는다"));
  expect(rows()).toEqual(["index:Widget", "index:wide"]);
  await vi.waitFor(() => expect(rows()).toEqual(["index:Widget", "index:wide"]));
  await vi.waitFor(() =>
    expect(host.symbolSearches.map((s) => s.query)).toEqual(["w", "wid", "wid"]),
  );
  await vi.waitFor(() =>
    expect(notes()).not.toContain("심볼 인덱스가 바쁘다 — 잠시 뒤 다시 찾는다"),
  );
});

it("draws only the visible rows first into an empty list and the rest after that frame", async () => {
  host.symbols = Array.from({ length: 40 }, (_, i) =>
    sym(`widget${String(i).padStart(2, "0")}`, "function", "src/lib.rs", i + 1),
  );
  await mount();
  await gotoSymbol();
  // 목록의 줄 수가 바뀔 때마다 적는다 — 처음 든 줄 수가 보이는 몫(FIRST_ROWS)이고, 나머지는 그 뒤에 든다.
  const counts: number[] = [];
  const observer = new MutationObserver(() => {
    const n = rows().length;
    if (n !== counts.at(-1)) counts.push(n);
  });
  observer.observe(popup()!, { childList: true, subtree: true });
  try {
    await type("widget");
    await vi.waitFor(() => expect(rows()).toHaveLength(40));
  } finally {
    observer.disconnect();
  }
  expect(counts.filter((n) => n > 0)).toEqual([FIRST_ROWS, 40]);
  expect(selectedRow()).toBe("index:widget00");
  // 앞이 빈 목록이 아니면(이어 친 키) 한꺼번에 그린다.
  counts.length = 0;
  observer.observe(popup()!, { childList: true, subtree: true });
  try {
    // 가짜 Host 는 글자를 차례로 맞춘다 — widget00 – 09 · widget10 · 20 · 30.
    await type("widget0");
    await vi.waitFor(() => expect(rows()).toHaveLength(13));
  } finally {
    observer.disconnect();
  }
  expect(counts.filter((n) => n > 0 && n < 13)).toEqual([]);
});

it("lists symbols as a group of the all tab with a way to the symbols tab", async () => {
  host.symbols = Array.from({ length: 9 }, (_, i) =>
    sym(`widget${i}`, "function", "src/lib.rs", i + 1),
  );
  await mount();
  await shiftShift();
  await type("widget");
  await vi.waitFor(() => expect(rows()).toHaveLength(6));
  const group = popup()!
    .querySelector("[cmdk-group] [data-symbol-source]")!
    .closest("[cmdk-group]")!;
  expect(group.querySelector("[cmdk-group-heading]")!.textContent).toBe("심볼");
  expect(values()).toContain("more:symbols");
});

it("closes with Esc without folding the editor's cursors (M4-14)", async () => {
  await mount();
  await openTab("/work/demo/src/main.rs");
  const view = EditorView.findFromDOM(document.querySelector<HTMLElement>(".cm-editor")!)!;
  await act(async () => {
    view.dispatch({
      selection: EditorSelection.create([EditorSelection.cursor(0), EditorSelection.cursor(8)]),
    });
    view.focus();
  });
  await gotoSymbol();
  expect(popup()).not.toBeNull();
  await press({ code: "Escape", key: "Escape" });
  expect(popup()).toBeNull();
  await settle();
  expect(view.state.selection.ranges).toHaveLength(2);
});

/** 한 키 입력: 칸을 `text` 로 바꾼 때부터 그 질의의 결과가 DOM 에 든 순간(`data-symbol-query`)까지. 폴링 · act 의 시간은 세지 않는다. */
async function keystroke(text: string): Promise<{ total: number; host: number }> {
  const before = host.indexMs.length;
  let drawn: number | undefined;
  const seen = () =>
    popup()!.querySelector(`[data-symbol-query="${text}"]`) !== null &&
    host.indexMs.length > before;
  const observer = new MutationObserver(() => {
    if (drawn === undefined && seen()) drawn = performance.now();
  });
  observer.observe(popup()!, { subtree: true, childList: true, attributes: true });
  const started = performance.now();
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input(), text);
    input().dispatchEvent(new Event("input", { bubbles: true }));
  });
  while (drawn === undefined) {
    await act(async () => new Promise((r) => setTimeout(r, 1)));
    if (performance.now() - started > 20_000) throw new Error(`no rows for ${text}`);
  }
  observer.disconnect();
  return { total: drawn - started, host: host.indexMs.at(-1)! };
}

const ms = (v: number) => v.toFixed(1);
function spread(label: string, values: number[]): string {
  const sorted = [...values].sort((a, b) => a - b);
  return `${label} median ${ms(sorted[sorted.length >> 1]!)} max ${ms(sorted.at(-1)!)}`;
}

it("measures a keystroke over 500,000 index symbols: the round trip with the Host's index time, its webview share, and the first search", async () => {
  host.symbols = [];
  for (let file = 0; file < 20_000; file++) {
    const path = `src/m${file}.rs`;
    for (let j = 0; j < 25; j++)
      host.symbols.push(
        sym(
          j === 0 ? `Item${file}` : `handleItem${file}x${j}`,
          j === 0 ? "struct" : "method",
          path,
          j + 1,
        ),
      );
  }
  expect(host.symbols).toHaveLength(500_000);
  await mount();
  const query = "handleItem1234x5";
  // 창을 연 뒤 첫 검색(코드 예열 — 사람이 창을 열고 처음 누르는 ⌘⌥O · ⌘O 의 실제 경로)의 첫 키: 대기 없이 그 키 하나만 잰다.
  await gotoSymbol();
  const first = await keystroke(query.slice(0, 1));
  for (let i = 2; i <= query.length; i++) await keystroke(query.slice(0, i));
  await press({ code: "Escape", key: "Escape" });
  // 팝업을 한 번 쓴 뒤 다시 연 팝업의 키 입력마다. 다시 연 팝업도 목록을 처음부터 만든다(닫으면 결과 줄이 사라진다).
  await gotoSymbol();
  const total: number[] = [];
  const hostShare: number[] = [];
  const webview: number[] = [];
  for (let i = 1; i <= query.length; i++) {
    const k = await keystroke(query.slice(0, i));
    total.push(k.total);
    hostShare.push(k.host);
    webview.push(k.total - k.host);
  }
  // 「Pitwall 쪽 왕복」(결정 M4-D19 — 서버 응답 밖): 입력 → 그 질의의 결과가 DOM 에 든 순간. Host 몫은 가짜 Host 가 50만 개를
  // JS 로 훑은 시간이라 실제 색인(pw-index, M4-15 release 실측 키당 최대 11 ~ 22 ms)의 대역일 뿐이고, IPC 는 메모리 전송이다 —
  // 실제 Host · IPC · WebKit 을 합친 판정은 M4-19(release 번들 실측)가 한다.
  console.log(
    `[perf] symbol first search after the window opened, first key: round trip ${ms(first.total)} ms (fake Host ${ms(first.host)}, webview ${ms(first.total - first.host)})`,
  );
  console.log(
    `[perf] symbol keystroke ms (popup used once): ${spread("round trip", total)}; ${spread("fake Host", hostShare)}; ${spread("webview", webview)} (${webview.map(ms).join(" ")})`,
  );
  // 회귀 가드는 웹뷰 몫만, 단독 측정에서만 한다(`PW_PERF=1 pnpm --filter @pitwall/desktop exec vitest run
  // src/search-everywhere/symbol-search-window.test.tsx -t 500,000`, CONTRIBUTING 「명령」) — 데스크톱 시험 150 파일이 함께 도는
  // 기본 실행에서는 다른 일꾼의 CPU · GC 가 중앙값까지 30 ms 를 넘겨(실측 34.6 ms) 흔들린다. 완료 기준(키 입력당 Pitwall 쪽
  // 30 ms)의 판정이 아니다 — 그것은 M4-19.
  if (process.env.PW_PERF) expect(Math.max(...webview)).toBeLessThan(30);
}, 120_000);
