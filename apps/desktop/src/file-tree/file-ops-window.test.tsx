// @vitest-environment happy-dom
// 프로젝트 창의 파일 조작(12-m1-plan M1-21 완료 기준): 가짜 Host(메모리 디스크 — `fs.create` · `fs.rename` · `fs.copy` · `fs.trash` 가
// Host 와 같은 규칙: 같은 이름이면 `CONFLICT` 이고 아무것도 바꾸지 않는다)에 붙여, 조작마다 트리 · 디스크 · 열린 탭이 일치하는지
// 본다. 이름 바꾸기(⇧F6) · 드래그 이동 · ⌥+드래그 복사(포인터 이벤트) · 드래그 취소 · 삭제(⌫, 확인 · 휴지통 없음 → 영구 삭제 확인) · 이름 겹침 · Finder 드롭.

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
} from "@pitwall/rpc-client";
import { EditorView } from "@codemirror/view";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../App";
import type { KeymapNative } from "../keymap/native";
import { activeWorktreeStore } from "../open-with/active-worktree";
import { getDocLanguage, PLAIN_TEXT } from "../editor/language";
import { fakeSyntaxServer, miniHighlight } from "../editor/syntax-testing";
import { createThemeStore } from "../theme/theme-store";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";
import type { DropImported } from "./file-ops";
import { resetWindowFileTree } from "./project-view";
import { fakeTreeLayout } from "./testing";
import type { FileDrop } from "./tree-drop";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const ROOT = "/work/demo";

const host = vi.hoisted(() => ({
  /** 파일 경로 → 내용. */
  files: new Map<string, string>(),
  /** 폴더 경로. 루트 포함. */
  dirs: new Set<string>(),
  calls: [] as { method: string; params: Record<string, unknown> }[],
  /** 휴지통이 없는 환경(`permanent` 가 아니면 `unavailable`). */
  noTrash: false,
  drop: undefined as ((drop: FileDrop) => void) | undefined,
  imports: [] as Record<string, unknown>[],
  importResult: undefined as DropImported | undefined,
  /** 구문 분석 서비스(`syntax.*`). `Rust` 만 문법이 있다. */
  syntax: undefined as ReturnType<typeof fakeSyntaxServer> | undefined,
}));

vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({
    listen: async (event: string, handler: (e: { payload: FileDrop }) => void) => {
      if (event === "pitwall://file-drop") host.drop = (payload) => handler({ payload });
      return () => {};
    },
  }),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: async (command: string, args: Record<string, unknown>) => {
    if (command !== "import_drop") throw new Error(`unexpected ${command}`);
    host.imports.push(args);
    return host.importResult;
  },
}));

vi.mock("../tauri-transport", async () => {
  const { createMemoryTransportPair: pair } = await import("@pitwall/rpc-client");
  const { DEFAULT_KEYMAP_TOML } = await import("../keymap/default-keymap");
  const { i18nState } = await import("../i18n/testing");
  const parentOf = (p: string) => p.slice(0, p.lastIndexOf("/"));
  const exists = (p: string) => host.files.has(p) || host.dirs.has(p);
  const under = (p: string, dir: string) => p === dir || p.startsWith(`${dir}/`);
  return {
    connectHost: async () => {
      const [client, server] = pair();
      const send = (m: object) => server.send(JSON.stringify({ jsonrpc: "2.0", ...m }));
      const project: Project = {
        id: "p1",
        name: "demo",
        host: "local",
        roots: [ROOT],
        favorite: false,
        tags: [],
        configMode: "repository",
        slug: "demo",
        createdAt: 1,
        openedAt: null,
      };
      server.onMessage((text) => {
        const m = JSON.parse(text) as { id?: number; method: string; params: never };
        const params = (m.params ?? {}) as Record<string, unknown>;
        const reply = (result: unknown) => send({ id: m.id, result });
        const error = (code: number, message: string, data: unknown = null) =>
          send({ id: m.id, error: { code, message, data } });
        const conflict = (path: string) => error(-32004, `\`${path}\` already exists`, { path });
        if (m.method.startsWith("fs.")) host.calls.push({ method: m.method, params });
        if (m.method.startsWith("syntax.")) {
          const source = host.syntax!.source;
          const run = {
            "syntax.open": source.open,
            "syntax.edit": source.edit,
            "syntax.highlights": source.highlights,
            "syntax.close": source.close,
          }[m.method] as ((p: unknown) => Promise<unknown>) | undefined;
          if (!run) return error(-32601, `no ${m.method}`);
          return void run(params).then(reply, (e: { code?: number; message: string }) =>
            error(e.code ?? -32603, e.message),
          );
        }
        const path = typeof params.path === "string" ? params.path : "";
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
            return reply({ project, root: ROOT, path, worktree: null } satisfies Resolution);
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
            if (!host.dirs.has(path)) return error(-32005, `not found: ${path}`);
            const names = [...host.dirs, ...host.files.keys()]
              .filter((p) => p !== path && parentOf(p) === path)
              .sort();
            const entries: FsEntry[] = names.map((p) => ({
              name: p.slice(p.lastIndexOf("/") + 1),
              kind: host.dirs.has(p) ? "dir" : "file",
              target: null,
              outside: false,
              size: 0,
              mtimeMs: 1,
              ignored: false,
            }));
            return reply({ path, entries });
          }
          case "fs.stat": {
            if (!exists(path)) return error(-32005, `not found: ${path}`);
            const text = host.files.get(path);
            return reply({
              path,
              kind: text === undefined ? "dir" : "file",
              target: null,
              size: text?.length ?? 0,
              mtimeMs: 1,
              readonly: false,
              sizeClass: text === undefined ? null : "editable",
            } satisfies FsStat);
          }
          case "fs.read": {
            const text = host.files.get(path);
            if (text === undefined) return error(-32005, `not found: ${path}`);
            return reply({
              path,
              size: text.length,
              mtimeMs: 1,
              offset: 0,
              eof: true,
              version: `v-${text}`,
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
          case "fs.create": {
            if (exists(path)) return conflict(path);
            if (params.kind === "dir") host.dirs.add(path);
            else host.files.set(path, "");
            return reply({ path });
          }
          case "fs.rename":
          case "fs.copy": {
            const from = params.from as string;
            const to = params.to as string;
            if (exists(to)) return conflict(to);
            const move = m.method === "fs.rename";
            for (const d of [...host.dirs].filter((d) => under(d, from))) {
              host.dirs.add(to + d.slice(from.length));
              if (move) host.dirs.delete(d);
            }
            for (const [f, text] of [...host.files].filter(([f]) => under(f, from))) {
              host.files.set(to + f.slice(from.length), text);
              if (move) host.files.delete(f);
            }
            return reply(move ? { from, to } : { to, files: 1, dirs: 0, links: 0, skipped: [] });
          }
          case "fs.trash": {
            const paths = params.paths as string[];
            if (host.noTrash && !params.permanent)
              return reply({
                removed: [],
                failed: paths.map((p) => ({
                  path: p,
                  reason: "unavailable",
                  message: "no trash on this volume",
                })),
              });
            for (const p of paths) {
              for (const d of Array.from(host.dirs)) if (under(d, p)) host.dirs.delete(d);
              for (const f of Array.from(host.files.keys())) if (under(f, p)) host.files.delete(f);
            }
            return reply({ removed: paths, failed: [] });
          }
          case "lang.detect": {
            // 확장자 `.rs` 만 언어가 있다(구문 강조가 이름 바꾸기를 따라가는지 본다).
            const { files } = params as { files: { path: string }[] };
            return reply({
              files: files.map((f) => ({
                language: f.path.endsWith(".rs") ? "Rust" : null,
                source: f.path.endsWith(".rs") ? "extension" : "none",
                vendored: false,
                generated: false,
                documentation: false,
              })),
            } satisfies LangDetectResult);
          }
          case "events.subscribe":
            return reply({ subscriptionId: 1 });
          case "events.unsubscribe":
            return reply(null);
          default:
            return error(-32601, `no ${m.method}`);
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

function disk(files: Record<string, string>) {
  host.files = new Map(Object.entries(files));
  host.dirs = new Set([ROOT]);
  for (const f of host.files.keys())
    for (
      let p = f.slice(0, f.lastIndexOf("/"));
      p.length > ROOT.length;
      p = p.slice(0, p.lastIndexOf("/"))
    )
      host.dirs.add(p);
}

let root: Root | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
  );
  vi.spyOn(console, "info").mockImplementation(() => {});
  fakeTreeLayout(600);
  window.history.replaceState(null, "", "/?window=project&project=p1");
  disk({
    [`${ROOT}/Cargo.toml`]: "[package]",
    [`${ROOT}/src/lib.rs`]: "pub fn lib() {}",
    [`${ROOT}/src/main.rs`]: "fn main() {}",
    [`${ROOT}/src/deep/leaf.rs`]: "// leaf",
  });
  host.calls = [];
  host.noTrash = false;
  host.drop = undefined;
  host.imports = [];
  host.importResult = undefined;
  host.syntax = fakeSyntaxServer({ grammars: { Rust: miniHighlight } });
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
  await vi.waitFor(() => expect(row(`${ROOT}/src`)).not.toBeNull());
}

const tree = () => document.querySelector<HTMLElement>("[role=tree]")!;
const row = (path: string) => document.querySelector<HTMLElement>(`[data-path="${path}"]`);
const rowPaths = () =>
  [...document.querySelectorAll<HTMLElement>("[role=treeitem]")].map((r) =>
    r.getAttribute("data-path"),
  );
const selectedRow = () =>
  document
    .querySelector<HTMLElement>("[role=treeitem][aria-selected=true]")
    ?.getAttribute("data-path");
const tabTitles = () =>
  [...document.querySelectorAll<HTMLElement>("[data-editor-tab-header]")].map((t) => t.textContent);
const toasts = () =>
  [...document.querySelectorAll("[data-sonner-toast]")].map((t) => t.textContent ?? "");
const fsCalls = (method: string) => host.calls.filter((c) => c.method === method);

async function mouse(el: Element, type: "click" | "dblclick" | "contextmenu") {
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

async function press(target: Element, init: KeyboardEventInit) {
  await act(async () => {
    target.dispatchEvent(
      new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }),
    );
    document.body.dispatchEvent(new KeyboardEvent("keyup", { bubbles: true, ...init }));
  });
  await settle();
}

/** 트리에 포커스를 두고 키를 누른다(대화상자가 닫힌 뒤 포커스가 어디로 갔든). */
async function pressInTree(init: KeyboardEventInit) {
  await act(async () => tree().focus());
  await press(tree(), init);
}

/** 트리의 행을 골라 트리에 포커스를 둔다. */
async function selectRow(path: string) {
  await act(async () => tree().focus());
  await mouse(row(path)!, "click");
  await act(async () => tree().focus());
}

/** 폴더를 펼치고 파일을 고정 탭으로 연다. */
async function openPinned(path: string) {
  await mouse(row(path)!, "dblclick");
  await vi.waitFor(() => expect(document.querySelector(".cm-editor")).not.toBeNull());
}

function editorView(): EditorView {
  const views = [...document.querySelectorAll<HTMLElement>(".cm-editor")];
  const view = EditorView.findFromDOM(views.at(-1)!);
  if (!view) throw new Error("no editor view");
  return view;
}

/**
 * 사용자가 친 것처럼 뷰에 넣는다. 포커스를 먼저 뺀다 — happy-dom 은 `selectionchange` 를 선택을 쓰는 자리에서 바로 내서, 포커스된
 * 뷰가 자기 갱신 중에 다시 갱신하려다 던진다(editor-window.test.tsx 의 `typeInto` 와 같다).
 */
async function typeInEditor(text: string) {
  (document.activeElement as HTMLElement | null)?.blur();
  await act(async () => {
    editorView().dispatch({
      changes: { from: 0, insert: text },
      selection: { anchor: text.length },
      userEvent: "input.type",
    });
  });
  await settle();
}

/** 이름 대화상자에 이름을 넣고 Enter. */
async function answerName(name: string) {
  const input = await vi.waitFor(() => {
    const el = document.querySelector<HTMLInputElement>("[data-testid=file-name-dialog] input");
    if (!el) throw new Error("no name dialog");
    return el;
  });
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, name);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await press(input, { key: "Enter", code: "Enter" });
}

/** 확인 대화상자의 단추를 누른다. 그 대화상자의 글자를 돌려준다. */
async function answerConfirm(label: string): Promise<string> {
  const dialog = await vi.waitFor(() => {
    const el = document.querySelector<HTMLElement>("[data-testid=file-confirm-dialog]");
    if (!el) throw new Error("no confirm dialog");
    return el;
  });
  const text = dialog.textContent ?? "";
  const button = [...dialog.querySelectorAll("button")].find((b) => b.textContent === label);
  if (!button) throw new Error(`no button ${label}: ${text}`);
  await mouse(button, "click");
  return text;
}

async function pointer(el: Element, type: string, init: PointerEventInit = {}) {
  await act(async () => {
    el.dispatchEvent(
      new PointerEvent(type, { bubbles: true, cancelable: true, pointerId: 1, ...init }),
    );
  });
}

/** 포인터로 `from` 행을 눌러 `to` 행 위로 끈다(문턱을 넘게). 놓지는 않는다. */
async function dragOver(from: string, to: string, altKey = false) {
  await pointer(row(from)!, "pointerdown", { button: 0, clientX: 10, clientY: 10 });
  await pointer(row(to)!, "pointermove", { clientX: 10, clientY: 40, altKey });
}

/** 드래그: `from` 행을 끌어 `to` 행에 놓는다. */
async function drag(from: string, to: string, altKey = false) {
  await dragOver(from, to, altKey);
  // 놓을 폴더의 행에 테두리가 보이고, 커서가 이동 · 복사를 보인다.
  expect(row(to)!.hasAttribute("data-drop-target")).toBe(true);
  expect(tree().getAttribute("data-dragging")).toBe(altKey ? "copy" : "move");
  await pointer(row(to)!, "pointerup", { clientX: 10, clientY: 40, altKey });
  expect(tree().hasAttribute("data-dragging")).toBe(false);
  await settle(40);
}

it("renames an open file with ⇧F6: the disk, the tree and the tab follow and the unsaved edit stays", async () => {
  await mount();
  await mouse(row(`${ROOT}/src`)!, "dblclick");
  await openPinned(`${ROOT}/src/lib.rs`);
  await typeInEditor("// edited\n");
  const reads = fsCalls("fs.read").length;

  await selectRow(`${ROOT}/src/lib.rs`);
  await pressInTree({ key: "F6", code: "F6", shiftKey: true });
  const input = document.querySelector<HTMLInputElement>("[data-testid=file-name-dialog] input")!;
  expect(input.value).toBe("lib.rs");
  await answerName("core.rs");

  expect(fsCalls("fs.rename").at(-1)?.params).toMatchObject({
    from: `${ROOT}/src/lib.rs`,
    to: `${ROOT}/src/core.rs`,
  });
  expect([...host.files.keys()]).toContain(`${ROOT}/src/core.rs`);
  expect(host.files.has(`${ROOT}/src/lib.rs`)).toBe(false);
  expect(rowPaths()).toContain(`${ROOT}/src/core.rs`);
  expect(rowPaths()).not.toContain(`${ROOT}/src/lib.rs`);
  expect(selectedRow()).toBe(`${ROOT}/src/core.rs`);
  expect(tabTitles().some((t) => t?.includes("core.rs"))).toBe(true);
  expect(tabTitles().some((t) => t?.includes("lib.rs"))).toBe(false);
  // 같은 문서가 따라갔다 — 미저장 편집이 남고, 새 경로를 다시 읽지 않았다. 배너가 없다.
  expect(editorView().state.doc.toString()).toBe("// edited\npub fn lib() {}");
  expect(fsCalls("fs.read").length).toBe(reads);
  expect(document.querySelector("[data-testid=disk-banner]")).toBeNull();
});

it("moves the highlight session and the document language with a renamed file", async () => {
  const before = `${ROOT}/src/lib.rs`;
  const after = `${ROOT}/src/core.rs`;
  const syntax = host.syntax!;
  await mount();
  await mouse(row(`${ROOT}/src`)!, "dblclick");
  await openPinned(before);
  await vi.waitFor(() => expect(document.querySelector(".pw-syntax-keyword")).not.toBeNull());
  expect(getDocLanguage(before)).toBe("Rust");
  expect(syntax.docs.size).toBe(1);
  const [first] = [...syntax.docs.keys()];

  await selectRow(before);
  await pressInTree({ key: "F6", code: "F6", shiftKey: true });
  await answerName("core.rs");
  await vi.waitFor(() => expect(tabTitles().some((t) => t?.includes("core.rs"))).toBe(true));

  // 옛 경로에는 언어도 세션도 남지 않는다 — 옛 세션의 서비스 문서는 닫혔고 새 경로의 것 하나만 열려 있다.
  expect(getDocLanguage(before)).toBe(PLAIN_TEXT);
  await vi.waitFor(() => expect(getDocLanguage(after)).toBe("Rust"));
  await vi.waitFor(() => expect(syntax.called("close")).toContainEqual({ docId: first }));
  expect(syntax.docs.size).toBe(1);
  expect(syntax.docs.has(first!)).toBe(false);

  // 편집이 새 세션으로 가고 강조가 따라온다.
  await typeInEditor("// edited\n");
  await vi.waitFor(() =>
    expect([...syntax.docs.values()].map((d) => d.text)).toEqual(["// edited\npub fn lib() {}"]),
  );
  await vi.waitFor(() =>
    expect(document.querySelector(".pw-syntax-comment")?.textContent).toBe("// edited"),
  );

  // 언어가 없는 이름으로 바꾸면 강조를 멈추고 세션을 닫는다.
  await selectRow(after);
  await pressInTree({ key: "F6", code: "F6", shiftKey: true });
  await answerName("core.txt");
  await vi.waitFor(() => expect(getDocLanguage(`${ROOT}/src/core.txt`)).toBe(PLAIN_TEXT));
  await vi.waitFor(() => expect(syntax.docs.size).toBe(0));
  await vi.waitFor(() => expect(document.querySelector("[class*=pw-syntax-]")).toBeNull());
  expect(editorView().state.doc.toString()).toBe("// edited\npub fn lib() {}");
});

it("moves a folder by drag with its open tab, and copies with ⌥+drag", async () => {
  await mount();
  await mouse(row(`${ROOT}/src`)!, "dblclick");
  await mouse(row(`${ROOT}/src/deep`)!, "dblclick");
  await openPinned(`${ROOT}/src/deep/leaf.rs`);

  // 폴더를 루트 행(빈 자리와 같다)으로: /work/demo/src/deep → /work/demo/deep.
  await drag(`${ROOT}/src/deep`, ROOT);
  expect(fsCalls("fs.rename").at(-1)?.params).toMatchObject({
    from: `${ROOT}/src/deep`,
    to: `${ROOT}/deep`,
  });
  expect(host.files.has(`${ROOT}/deep/leaf.rs`)).toBe(true);
  expect(rowPaths()).toContain(`${ROOT}/deep`);
  expect(rowPaths()).not.toContain(`${ROOT}/src/deep`);
  // 옮긴 폴더는 펼친 채로 새 자리에 나온다.
  await vi.waitFor(() => expect(rowPaths()).toContain(`${ROOT}/deep/leaf.rs`));
  // 탭이 새 경로를 따라갔다: 「프로젝트 뷰에서 선택」 이 새 자리의 행을 고른다.
  const selectIn = document.querySelector<HTMLElement>(
    'button[aria-label="프로젝트 뷰에서 선택"]',
  )!;
  await mouse(selectIn, "click");
  await vi.waitFor(() => expect(selectedRow()).toBe(`${ROOT}/deep/leaf.rs`));
  expect(document.querySelector("[data-testid=disk-banner]")).toBeNull();

  // ⌥+드래그: 파일을 폴더 행에 놓으면 복사한다. 원본은 남는다.
  await drag(`${ROOT}/Cargo.toml`, `${ROOT}/src`, true);
  expect(fsCalls("fs.copy").at(-1)?.params).toMatchObject({
    from: `${ROOT}/Cargo.toml`,
    to: `${ROOT}/src/Cargo.toml`,
  });
  expect(host.files.get(`${ROOT}/src/Cargo.toml`)).toBe("[package]");
  expect(host.files.has(`${ROOT}/Cargo.toml`)).toBe(true);
  await vi.waitFor(() => expect(rowPaths()).toContain(`${ROOT}/src/Cargo.toml`));
});

it("cancels a tree drag with Esc or by letting go outside the tree, and a short move is a click", async () => {
  await mount();
  const before = host.calls.length;

  await dragOver(`${ROOT}/Cargo.toml`, `${ROOT}/src`);
  expect(row(`${ROOT}/src`)!.hasAttribute("data-drop-target")).toBe(true);
  await act(async () => {
    document.body.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Escape",
        code: "Escape",
        bubbles: true,
        cancelable: true,
      }),
    );
  });
  expect(row(`${ROOT}/src`)!.hasAttribute("data-drop-target")).toBe(false);
  expect(tree().hasAttribute("data-dragging")).toBe(false);
  await pointer(row(`${ROOT}/src`)!, "pointerup", { clientX: 10, clientY: 40 });
  await settle(40);

  // 트리 밖으로 끌면 놓을 곳이 없고, 거기서 떼면 아무것도 하지 않는다.
  await dragOver(`${ROOT}/Cargo.toml`, `${ROOT}/src`);
  await pointer(document.body, "pointermove", { clientX: 900, clientY: 40 });
  expect(document.querySelector("[data-drop-target]")).toBeNull();
  expect(tree().getAttribute("data-dragging")).toBe("move");
  await pointer(document.body, "pointerup", { clientX: 900, clientY: 40 });
  await settle(40);
  expect(tree().hasAttribute("data-dragging")).toBe(false);

  // 문턱 전에 떼면 끌기가 아니다.
  await pointer(row(`${ROOT}/Cargo.toml`)!, "pointerdown", { button: 0, clientX: 10, clientY: 10 });
  await pointer(row(`${ROOT}/src`)!, "pointermove", { clientX: 11, clientY: 11 });
  expect(tree().hasAttribute("data-dragging")).toBe(false);
  await pointer(row(`${ROOT}/src`)!, "pointerup", { clientX: 11, clientY: 11 });
  await settle(40);

  expect(host.calls.slice(before).filter((c) => c.method !== "fs.list")).toEqual([]);
  expect(host.files.has(`${ROOT}/Cargo.toml`)).toBe(true);
  expect(host.files.has(`${ROOT}/src/Cargo.toml`)).toBe(false);
});

it("copies with ⌥ pressed during a drag even when the key arrives as IME composition", async () => {
  await mount();
  await dragOver(`${ROOT}/Cargo.toml`, `${ROOT}/src`);
  expect(tree().getAttribute("data-dragging")).toBe("move");
  // 한글 입력기에서 ⌥ 는 조합 중인 키로 올 수 있다 — 수식키 상태로 판정한다.
  await act(async () => {
    const down = new KeyboardEvent("keydown", {
      key: "Alt",
      code: "AltLeft",
      altKey: true,
      isComposing: true,
      bubbles: true,
      cancelable: true,
    });
    Object.defineProperty(down, "keyCode", { value: 229 });
    document.body.dispatchEvent(down);
  });
  expect(tree().getAttribute("data-dragging")).toBe("copy");
  await pointer(row(`${ROOT}/src`)!, "pointerup", { clientX: 10, clientY: 40, altKey: true });
  await settle(40);
  expect(fsCalls("fs.copy").at(-1)?.params).toMatchObject({
    from: `${ROOT}/Cargo.toml`,
    to: `${ROOT}/src/Cargo.toml`,
  });
  expect(fsCalls("fs.rename")).toEqual([]);
  expect(host.files.get(`${ROOT}/src/Cargo.toml`)).toBe("[package]");
  expect(host.files.has(`${ROOT}/Cargo.toml`)).toBe(true);
});

it("reports a taken name as an error and changes nothing", async () => {
  await mount();
  await mouse(row(`${ROOT}/src`)!, "contextmenu");
  const item = [...document.querySelectorAll<HTMLElement>("[role=menuitem]")].find(
    (e) => e.textContent === "새 파일…",
  )!;
  await mouse(item, "click");
  await answerName("lib.rs");
  expect(fsCalls("fs.create").at(-1)?.params).toMatchObject({ path: `${ROOT}/src/lib.rs` });
  expect(host.files.get(`${ROOT}/src/lib.rs`)).toBe("pub fn lib() {}");
  await vi.waitFor(() =>
    expect(toasts().some((t) => t.includes("「lib.rs」 이(가) 이미 있다"))).toBe(true),
  );

  // 이름 바꾸기도 같은 이름이면 그대로다.
  await mouse(row(`${ROOT}/src`)!, "dblclick");
  await selectRow(`${ROOT}/src/main.rs`);
  await pressInTree({ key: "F6", code: "F6", shiftKey: true });
  await answerName("lib.rs");
  expect(host.files.get(`${ROOT}/src/main.rs`)).toBe("fn main() {}");
  expect(host.files.get(`${ROOT}/src/lib.rs`)).toBe("pub fn lib() {}");
  expect(rowPaths()).toContain(`${ROOT}/src/main.rs`);

  // 쓸 수 없는 이름은 대화상자가 받지 않는다.
  await pressInTree({ key: "F6", code: "F6", shiftKey: true });
  await answerName("a/b");
  expect(document.querySelector("[data-testid=file-name-dialog] [role=alert]")?.textContent).toBe(
    "이름에 / 나 \\ 를 쓸 수 없다",
  );
});

it("asks before deleting, closes the clean tab and keeps the unsaved one with the deleted banner", async () => {
  await mount();
  await mouse(row(`${ROOT}/src`)!, "dblclick");
  await openPinned(`${ROOT}/src/lib.rs`);
  await typeInEditor("// unsaved\n");
  await openPinned(`${ROOT}/src/main.rs`);
  expect(tabTitles()).toHaveLength(2);

  await selectRow(`${ROOT}/src`);
  await pressInTree({ key: "Backspace", code: "Backspace" });
  // 취소하면 아무것도 지우지 않는다.
  await answerConfirm("취소");
  expect(fsCalls("fs.trash")).toHaveLength(0);

  await pressInTree({ key: "Backspace", code: "Backspace" });
  const text = await answerConfirm("휴지통으로 옮기기");
  expect(text).toContain("「src」 을(를) 휴지통으로 옮긴다");
  expect(text).toContain("lib.rs");
  expect(fsCalls("fs.trash").at(-1)?.params).toMatchObject({
    paths: [`${ROOT}/src`],
    permanent: false,
  });
  expect([...host.files.keys()].some((f) => f.startsWith(`${ROOT}/src`))).toBe(false);
  expect(rowPaths()).not.toContain(`${ROOT}/src`);
  // 미저장이 없는 main.rs 탭은 닫혔고, lib.rs 탭은 「지워짐」 배너와 함께 남는다.
  expect(tabTitles()).toHaveLength(1);
  expect(tabTitles()[0]).toContain("lib.rs");
  expect(document.querySelector("[data-testid=disk-banner]")).not.toBeNull();
});

it("deletes permanently only after a second confirmation when there is no trash", async () => {
  host.noTrash = true;
  await mount();
  await selectRow(`${ROOT}/Cargo.toml`);
  await pressInTree({ key: "Backspace", code: "Backspace" });
  await answerConfirm("휴지통으로 옮기기");
  // 두 번째 확인을 취소하면 남는다.
  const text = await answerConfirm("취소");
  expect(text).toContain("no trash on this volume");
  expect(host.files.has(`${ROOT}/Cargo.toml`)).toBe(true);
  expect(fsCalls("fs.trash").map((c) => c.params.permanent)).toEqual([false]);

  await pressInTree({ key: "Backspace", code: "Backspace" });
  await answerConfirm("휴지통으로 옮기기");
  await answerConfirm("영구 삭제");
  expect(fsCalls("fs.trash").map((c) => c.params.permanent)).toEqual([false, false, true]);
  expect(host.files.has(`${ROOT}/Cargo.toml`)).toBe(false);
  await vi.waitFor(() => expect(rowPaths()).not.toContain(`${ROOT}/Cargo.toml`));
});

it("imports a Finder drop on a folder row into that folder by the drop's number", async () => {
  await mount();
  await mouse(row(`${ROOT}/src`)!, "dblclick");
  expect(host.drop).toBeDefined();
  host.importResult = {
    copied: [`${ROOT}/src/one.txt`],
    conflicts: ["/Users/me/Desktop/lib.rs"],
    skipped: [],
    links: 0,
    cancelled: false,
  };
  vi.spyOn(tree(), "getBoundingClientRect").mockReturnValue(new DOMRect(0, 0, 300, 600));
  document.elementFromPoint = () => row(`${ROOT}/src`);
  vi.spyOn(window, "devicePixelRatio", "get").mockReturnValue(2);
  host.files.set(`${ROOT}/src/one.txt`, "one");

  await act(async () =>
    host.drop!({
      id: 7,
      paths: ["/Users/me/Desktop/one.txt", "/Users/me/Desktop/lib.rs"],
      x: 40,
      y: 60,
    }),
  );
  await settle(40);
  expect(host.imports).toEqual([
    {
      dropId: 7,
      indices: null,
      projectId: "p1",
      worktree: null,
      dir: `${ROOT}/src`,
      title: "드롭한 항목 복사",
    },
  ]);
  await vi.waitFor(() => expect(rowPaths()).toContain(`${ROOT}/src/one.txt`));
  expect(toasts().some((t) => t.includes("같은 이름이 있어 복사하지 않았다"))).toBe(true);

  // 트리 밖의 드롭은 받지 않는다(터미널 등).
  await act(async () => host.drop!({ id: 8, paths: ["/x"], x: 2000, y: 60 }));
  await settle();
  expect(host.imports).toHaveLength(1);
});
