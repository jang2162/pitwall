// @vitest-environment happy-dom
// 포맷(15-m4-plan M4-13)을 프로젝트 창(App kind="project")과 가짜 Host 로 본다 — `⌘⌥L` 이 서버의 편집을 문서에만 한 undo 로 넣고
// (디스크는 그대로) 커서가 같은 낱말에 남는다, 선택이 있으면 그 범위로 묻는다, 시한을 넘기면 문서가 그대로다, 늦은 답은 넣지
// 않는다, 떠 있는 서버가 포맷을 못 하면 「이 언어의 포매터가 없다」, 서버가 없으면(미신뢰) 조용하다. 창 틀(가짜 Host · 마운트)은
// lsp-edit-window.test.tsx 와 같고 파일은 시험용 디스크(edit-testing.ts)다 — 이 행은 그 디스크에 아무것도 쓰지 않아야 한다.
//
// CodeMirror 는 불러올 때 macOS 인지 정한다(navigator.platform) — 그래서 불러오기 전에 Mac 으로 둔다.

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
import { editorContext } from "../editor/extensions/context";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";
import { fakeLspServer } from "./lsp-testing";
import { fakeDisk } from "./edit-testing";

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
  /** 구문 분석 서비스(`syntax.*`). 이 시험은 쓰지 않는다. */
  syntax: undefined as { source: Record<string, (p: unknown) => Promise<unknown>> } | undefined,
  /** 서버 관리자(`lsp.document.*`). */
  lsp: undefined as ReturnType<typeof fakeLspServer> | undefined,
  /** 크기 등급을 바꿀 파일(5 MiB 초과의 읽기 전용 등). */
  sizeClass: new Map<string, "editable" | "readOnly">(),
  /** 외부로 연 주소(`openUrl`). */
  opened: [] as string[],
  /** 시험용 디스크(`fs.*`). */
  disk: undefined as unknown as ReturnType<typeof import("./edit-testing").fakeDisk>,
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
        // 기능 행의 Host RPC(`lsp.rename` 등)는 가짜 서버 관리자의 처리기로(`host.lsp.handle`).
        if (m.method.startsWith("lsp.") && !m.method.startsWith("lsp.document.") && host.lsp)
          return void host.lsp.services
            .call(m.method as never, m.params)
            .then(reply, (e: { code?: number; message: string; data?: unknown }) =>
              send({
                id: m.id,
                error: { code: e.code ?? -32603, message: e.message, data: e.data },
              }),
            );
        if (m.method.startsWith("fs.") && m.method !== "fs.watch") {
          const p = m.params as {
            path: string;
            from: string;
            to: string;
            paths: string[];
            content: never;
            bom?: boolean;
            expectedVersion?: string | null;
          };
          const fs = host.disk.fs;
          const run = {
            "fs.stat": () => fs.stat(p.path),
            "fs.read": () => fs.read(p.path),
            "fs.write": () =>
              fs.write(p.path, p.content, p.bom ?? false, p.expectedVersion ?? null),
            "fs.create": () => fs.create(p.path),
            "fs.rename": () => fs.rename(p.from, p.to),
            "fs.trash": () => fs.trash(p.paths[0]!),
          }[m.method];
          if (run)
            return void run().then(reply, (e: { code?: number; message: string }) =>
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

const ROOT = "/work/demo";
/** 열어 둘 파일(깨끗한 문서). */
const MAIN = "/work/demo/src/main.rs";
/** 열지 않는 파일. */
const LIB = "/work/demo/src/lib.rs";
const MAIN_TEXT = "fn  old( ){}\nfn main(){ old() ;}\n";
const LIB_TEXT = "pub use crate::old;\n";

let root: Root | undefined;

function useUserAgent(ua: string) {
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(ua);
}

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  useUserAgent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)");
  vi.spyOn(console, "info").mockImplementation(() => {});
  window.history.replaceState(null, "", "/?window=project&project=p1");
  host.files = new Map();
  host.disk = fakeDisk(ROOT, { [MAIN]: MAIN_TEXT, [LIB]: LIB_TEXT });
  host.settings = {};
  host.subscribers = [];
  host.syntax = undefined;
  host.lsp = fakeLspServer();
  host.sizeClass = new Map();
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

/** 이 시험의 문서 능력 칸: 포맷 · rename 을 rust-analyzer 가 맡는다. */
const FEATURES = {
  formatting: { server: "rust-analyzer", options: true, optionsTruncated: false },
  rename: { server: "rust-analyzer", options: true, optionsTruncated: false },
};

const at = (line: number, character: number) => ({ line, character });
const range = (l1: number, c1: number, l2: number, c2: number) => ({
  start: at(l1, c1),
  end: at(l2, c2),
});

/** `MAIN` 을 열고 커서를 둘째 줄 `main` 안에 둔다(능력 칸이 찰 때까지). */
async function openMain(features: object = FEATURES) {
  host.lsp!.setFeatures(features as never);
  await mount();
  await open(MAIN, 1, 5);
  await emit("lsp.status", { projectId: "p1", instances: [] });
  const deadline = Date.now() + 5000;
  while (!view().state.facet(editorContext)?.lsp?.capabilities.rename && Date.now() < deadline)
    await settle();
  expect(view().state.facet(editorContext)?.lsp?.capabilities.rename).toBeDefined();
  // 둘째 줄 「fn main(){」 의 `main` 가운데(m-a|i-n).
  await act(async () => view().focus());
}

/** `⌘⌥L` — macOS 의 ⌥ 는 글자를 바꾼다(`¬`). */
const formatKey = () =>
  keydown({ key: "¬", code: "KeyL", metaKey: true, altKey: true }, view().contentDOM);
const undoKey = () => keydown({ key: "z", code: "KeyZ", metaKey: true }, view().contentDOM);
/** `check` 가 참이 될 때까지(최대 5 s — 기기 부하에서도). */
const until = async (check: () => boolean) => {
  const deadline = Date.now() + 5000;
  while (!check() && Date.now() < deadline) await settle();
  expect(check()).toBe(true);
};

/** 서버(rust-analyzer 의 rustfmt)가 낸 결과 — 문서를 통째로 바꾸는 편집 하나로 준다(ruff 같은 서버). */
const FORMATTED = "fn old() {}\nfn main() {\n    old();\n}\n";
const WHOLE = { range: range(0, 0, 2, 0), newText: FORMATTED };

const formatted = (edits: unknown[] = [WHOLE]) => ({
  server: "rust-analyzer",
  range: false,
  staleVersion: false,
  edits,
});

/** 시험에서 새로 뜬 토스트 글(sonner 의 토스트는 앞 시험의 것이 전역에 남는다). */
function toasts() {
  const before = new Set(
    [...document.querySelectorAll("[data-sonner-toast]")].map((t) => t.textContent ?? ""),
  );
  return () =>
    [...document.querySelectorAll("[data-sonner-toast]")]
      .map((t) => t.textContent ?? "")
      .filter((t) => !before.has(t));
}

const placeCursor = (anchor: number, head = anchor) =>
  act(async () => view().dispatch({ selection: { anchor, head } }));

it("puts the formatter's edits into the document as one undo step, keeps the cursor on its word and writes nothing to disk (⌘⌥L)", async () => {
  host.lsp!.handle("lsp.format", async () => formatted());
  await openMain();
  await placeCursor(MAIN_TEXT.indexOf("main") + 2);
  await formatKey();
  await until(() => view().state.doc.toString() === FORMATTED);
  // 커서는 같은 낱말(`main`)의 같은 자리에 남는다.
  const head = view().state.selection.main.head;
  expect(head).toBe(FORMATTED.indexOf("main") + 2);
  const word = view().state.wordAt(head)!;
  expect(view().state.sliceDoc(word.from, word.to)).toBe("main");
  // 옵션 둘만, 선택이 없으니 범위 없이.
  const asked = host.lsp!.called("call:lsp.format") as Record<string, unknown>[];
  expect(asked).toHaveLength(1);
  expect(asked[0]!.options).toEqual({ tabSize: 4, insertSpaces: true });
  expect(asked[0]).not.toHaveProperty("range");
  // 디스크에는 아무것도 쓰지 않았다 — 저장은 ⌘S.
  expect(host.disk.text(MAIN)).toBe(MAIN_TEXT);
  expect(host.disk.calls).toEqual([]);
  // ⌘Z 한 번이 포맷 전체를 되돌린다.
  await undoKey();
  expect(view().state.doc.toString()).toBe(MAIN_TEXT);
});

it("asks for the selection's range with the document's indentation", async () => {
  host.lsp!.handle("lsp.format", async () => formatted([]));
  host.disk = fakeDisk(ROOT, { [MAIN]: "fn a() {\n  b();\n    c();\n  d();\n}\n" });
  await openMain();
  const text = view().state.doc.toString();
  await placeCursor(text.indexOf("b();"), text.indexOf("d();"));
  await formatKey();
  await until(() => host.lsp!.called("call:lsp.format").length === 1);
  expect(host.lsp!.called("call:lsp.format")[0]).toMatchObject({
    range: { from: text.indexOf("b();"), to: text.indexOf("d();") },
    options: { tabSize: 2, insertSpaces: true },
  });
  // 바꿀 것이 없다는 답 — 문서 그대로, 알림 없음.
  expect(view().state.doc.toString()).toBe(text);
});

it("leaves the document as it was and says so when formatting runs past the time limit", async () => {
  host.lsp!.handle("lsp.format", async () => {
    throw Object.assign(new Error("the language server did not answer in time"), {
      code: 5003,
      data: { server: "rust-analyzer", timedOut: true },
    });
  });
  await openMain();
  const fresh = toasts();
  await formatKey();
  await until(() => fresh().some((t) => t.includes("10초 안에 끝나지 않아")));
  expect(view().state.doc.toString()).toBe(MAIN_TEXT);
  expect(host.disk.calls).toEqual([]);
});

it("does not put a late answer into a document that changed while formatting", async () => {
  let answer!: (value: unknown) => void;
  host.lsp!.handle("lsp.format", () => new Promise((resolve) => (answer = resolve)));
  await openMain();
  const fresh = toasts();
  await formatKey();
  await until(() => host.lsp!.called("call:lsp.format").length === 1);
  // 답을 기다리는 사이 사람이 친다.
  await act(async () =>
    view().dispatch({ changes: { from: 0, insert: "// x\n" }, userEvent: "input.type" }),
  );
  await act(async () => answer(formatted()));
  await settle();
  expect(view().state.doc.toString()).toBe(`// x\n${MAIN_TEXT}`);
  await until(() => fresh().some((t) => t.includes("포맷하는 사이 문서가 바뀌어")));
});

it("says there is no formatter when the running server cannot format", async () => {
  host.lsp!.handle("lsp.format", async () => formatted());
  await openMain({ rename: FEATURES.rename });
  const fresh = toasts();
  await formatKey();
  await until(() => fresh().some((t) => t.includes("이 언어의 포매터가 없다")));
  expect(host.lsp!.called("call:lsp.format")).toEqual([]);
  expect(view().state.doc.toString()).toBe(MAIN_TEXT);
});

it("stays quiet without a server for the document (an untrusted project starts none — M4-D8)", async () => {
  host.lsp!.handle("lsp.format", async () => formatted());
  await mount();
  await open(MAIN, 1, 5);
  await act(async () => view().focus());
  const fresh = toasts();
  await formatKey();
  expect(host.lsp!.called("call:lsp.format")).toEqual([]);
  expect(view().state.doc.toString()).toBe(MAIN_TEXT);
  expect(fresh()).toEqual([]);
});
