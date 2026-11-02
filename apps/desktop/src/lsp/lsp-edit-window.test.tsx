// @vitest-environment happy-dom
// rename · 코드 액션 · 일괄 편집(15-m4-plan M4-12)을 프로젝트 창(App kind="project")과 가짜 Host 로 본다 — `⇧F6` 의 인라인 입력 →
// `lsp.rename` → 미리보기 → 디스크 → 알림의 [되돌리기], `Esc` 는 `preventDefault`(M4-14), 파일 트리 포커스의 `⇧F6` 은 rename 이 아니다
// (⑧), `⌥↩` 의 목록 → resolve → 명령 → 그 명령의 흐름으로 온 서버의 편집만 적용(⑦), 진단 팝업의 「빠른 수정」 은 그 서버의 진단만,
// 맡을 서버가 없으면(미신뢰) 조용하다. 창 틀(가짜 Host · 마운트)은 lsp-window.test.tsx 와 같고, 파일은 시험용 디스크(edit-testing.ts)다.
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
import { toast } from "sonner";
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
import { currentPopup, openPopup } from "./popup";

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
            .then(reply, (e: { code?: number; message: string }) =>
              send({ id: m.id, error: { code: e.code ?? -32603, message: e.message } }),
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
const MAIN_TEXT = "fn old() {}\nfn main() { old(); }\n";
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

/** 이 시험의 문서 능력 칸: rename(prepare 포함) · 코드 액션을 rust-analyzer 가 맡는다. */
const FEATURES = {
  rename: { server: "rust-analyzer", options: { prepareProvider: true }, optionsTruncated: false },
  code_action: { server: "rust-analyzer", options: true, optionsTruncated: false },
};

const at = (line: number, character: number) => ({ line, character });
const range = (l1: number, c1: number, l2: number, c2: number) => ({
  start: at(l1, c1),
  end: at(l2, c2),
});

/** `MAIN` 을 열고 커서를 `old` 안에 둔다(능력 칸이 찰 때까지). */
async function openMain() {
  host.lsp!.setFeatures(FEATURES as never);
  await mount();
  await open(MAIN, 1, 5);
  await emit("lsp.status", { projectId: "p1", instances: [] });
  const deadline = Date.now() + 5000;
  while (!view().state.facet(editorContext)?.lsp?.capabilities.rename && Date.now() < deadline)
    await settle();
  expect(view().state.facet(editorContext)?.lsp?.capabilities.rename).toBeDefined();
  await act(async () => view().focus());
}

const shiftF6 = (target?: Element | null) =>
  keydown({ key: "F6", code: "F6", shiftKey: true }, target ?? view().contentDOM);
const optionEnter = () => keydown({ key: "Enter", code: "Enter", altKey: true }, view().contentDOM);
const button = (text: string) =>
  [...document.querySelectorAll<HTMLButtonElement>("button")].find(
    (b) => b.textContent?.trim() === text,
  );
/** `check` 가 참이 될 때까지(최대 5 s — 기기 부하에서도). */
const until = async (check: () => boolean) => {
  const deadline = Date.now() + 5000;
  while (!check() && Date.now() < deadline) await settle();
  expect(check()).toBe(true);
};

function renameHandlers() {
  host.lsp!.handle("lsp.rename.prepare", async () => ({
    server: "rust-analyzer",
    staleVersion: false,
    target: { kind: "range", from: 3, to: 6, placeholder: "old" },
  }));
  host.lsp!.handle("lsp.rename", async (params) => ({
    server: "rust-analyzer",
    staleVersion: false,
    edit: {
      changes: [
        {
          kind: "text",
          path: MAIN,
          version: (params as { version: number }).version,
          edits: [
            { range: range(0, 3, 0, 6), newText: "neo" },
            { range: range(1, 12, 1, 15), newText: "neo" },
          ],
        },
        {
          kind: "text",
          path: LIB,
          version: null,
          edits: [{ range: range(0, 15, 0, 18), newText: "neo" }],
        },
      ],
    },
  }));
}

it("renames with ⇧F6 through the inline input, the preview and the disk, and undoes it from the notice", async () => {
  renameHandlers();
  await openMain();
  await shiftF6();
  const input = document.querySelector<HTMLInputElement>(".cm-lsp-rename-input");
  expect(input?.value).toBe("old");
  expect(host.lsp!.called("call:lsp.rename.prepare")).toEqual([
    expect.objectContaining({ offset: view().state.selection.main.head }),
  ]);
  input!.value = "neo";
  await keydown({ key: "Enter", code: "Enter" }, input);
  expect(host.lsp!.called("call:lsp.rename")).toEqual([
    expect.objectContaining({ offset: 3, newName: "neo" }),
  ]);
  // 둘 이상이라 미리보기 — 아직 아무것도 바뀌지 않았다.
  await until(() => document.querySelector("[data-testid='lsp-edit-preview']") !== null);
  expect(host.disk.text(LIB)).toBe(LIB_TEXT);
  expect(view().state.doc.toString()).toBe(MAIN_TEXT);
  await act(async () => button("적용")!.click());
  await until(() => host.disk.text(LIB) === "pub use crate::neo;\n");
  expect(host.disk.text(MAIN)).toBe("fn neo() {}\nfn main() { neo(); }\n");
  expect(view().state.doc.toString()).toBe("fn neo() {}\nfn main() { neo(); }\n");
  await until(() => document.body.textContent!.includes("2개 파일을 바꿨다"));
  await act(async () => button("되돌리기")!.click());
  await until(() => host.disk.text(LIB) === LIB_TEXT);
  expect(host.disk.text(MAIN)).toBe(MAIN_TEXT);
  expect(view().state.doc.toString()).toBe(MAIN_TEXT);
});

it("cancels the rename input on Esc with the press marked handled, so the cursors stay (M4-14)", async () => {
  renameHandlers();
  await openMain();
  await shiftF6();
  const input = document.querySelector<HTMLInputElement>(".cm-lsp-rename-input")!;
  const esc = new KeyboardEvent("keydown", {
    key: "Escape",
    code: "Escape",
    bubbles: true,
    cancelable: true,
  });
  await act(async () => input.dispatchEvent(esc));
  await settle();
  expect(esc.defaultPrevented).toBe(true);
  expect(document.querySelector(".cm-lsp-rename-input")).toBeNull();
  expect(host.lsp!.called("call:lsp.rename")).toEqual([]);
  // 한글 조합 중의 ↩ 는 이름을 정하지 않는다.
  await shiftF6();
  const again = document.querySelector<HTMLInputElement>(".cm-lsp-rename-input")!;
  again.value = "새이름";
  await keydown({ key: "Enter", code: "Enter", isComposing: true }, again);
  expect(host.lsp!.called("call:lsp.rename")).toEqual([]);
  expect(document.querySelector(".cm-lsp-rename-input")).not.toBeNull();
});

it("leaves ⇧F6 to the file tree when the tree has focus (⑧)", async () => {
  renameHandlers();
  await openMain();
  const tree = document.createElement("div");
  tree.tabIndex = 0;
  tree.setAttribute("data-keymap-context", "fileTree");
  document.body.append(tree);
  await act(async () => tree.focus());
  await shiftF6(tree);
  expect(host.lsp!.called("call:lsp.rename.prepare")).toEqual([]);
  await act(async () => view().focus());
  await shiftF6();
  expect(host.lsp!.called("call:lsp.rename.prepare")).toHaveLength(1);
});

it("runs a code action's command with ⌥↩ and applies only the server edit of its own flow (⑦)", async () => {
  let finish: (() => void) | undefined;
  let flowId: string | undefined;
  host.lsp!.handle("lsp.codeAction.list", async () => ({
    server: "rust-analyzer",
    staleVersion: false,
    truncated: false,
    actions: [
      {
        title: "Extract",
        kind: "refactor",
        isPreferred: false,
        disabled: null,
        raw: { title: "Extract" },
      },
      {
        title: "Fix it",
        kind: "quickfix",
        isPreferred: true,
        disabled: null,
        raw: { title: "Fix it", data: 1 },
      },
    ],
  }));
  host.lsp!.handle("lsp.codeAction.resolve", async () => ({
    server: "rust-analyzer",
    edit: null,
    command: { title: "Fix it", command: "fix.it" },
  }));
  host.lsp!.handle("lsp.command.execute", (params) => {
    flowId = (params as { flowId: string }).flowId;
    return new Promise((resolve) => {
      finish = () => resolve({ server: "rust-analyzer", result: null });
    });
  });
  host.lsp!.handle("lsp.applyEdit.answer", async () => null);
  await openMain();
  await optionEnter();
  const items = [...document.querySelectorAll<HTMLButtonElement>(".pw-lsp-code-action")];
  expect(items.map((b) => b.textContent)).toEqual(["Extract", "Fix it"]);
  expect(host.lsp!.called("call:lsp.codeAction.list")).toEqual([
    expect.objectContaining({ trigger: "invoked", diagnostics: null }),
  ]);
  await act(async () => items[1]!.click());
  await until(() => flowId !== undefined);
  const edit = {
    changes: [
      {
        kind: "text",
        path: LIB,
        version: null,
        edits: [{ range: range(0, 0, 0, 0), newText: "// fixed\n" }],
      },
    ],
  };
  // 다른 흐름(이 창이 연 것이 아님)의 편집은 적용하지도 답하지도 않는다.
  await emit("lsp.applyEdit", {
    flowId: "someone-else",
    requestId: 7,
    server: "rust-analyzer",
    label: null,
    edit,
  });
  expect(host.disk.text(LIB)).toBe(LIB_TEXT);
  await emit("lsp.applyEdit", {
    flowId,
    requestId: 1,
    server: "rust-analyzer",
    label: "Fix it",
    edit,
  });
  await until(() => host.lsp!.called("call:lsp.applyEdit.answer").length > 0);
  expect(host.lsp!.called("call:lsp.applyEdit.answer")).toEqual([
    { flowId, requestId: 1, applied: true, failureReason: null },
  ]);
  expect(host.disk.text(LIB)).toBe("// fixed\n" + LIB_TEXT);
  await act(async () => finish!());
  await settle();
  // 명령이 끝난 뒤 같은 흐름으로 온 편집은 받지 않는다.
  await emit("lsp.applyEdit", { flowId, requestId: 2, server: "rust-analyzer", label: null, edit });
  expect(host.lsp!.called("call:lsp.applyEdit.answer")).toHaveLength(1);
  expect(host.disk.text(LIB)).toBe("// fixed\n" + LIB_TEXT);
});

it("asks for quick fixes with only the diagnostics of the code action server from a diagnostic popup", async () => {
  host.lsp!.handle("lsp.codeAction.list", async () => ({
    server: "rust-analyzer",
    staleVersion: false,
    truncated: false,
    actions: [],
  }));
  await openMain();
  const mine = { range: range(0, 3, 0, 6), message: "unused", source: "rustc" };
  const other = { range: range(0, 3, 0, 6), message: "style", source: "clippy-server" };
  await act(async () =>
    openPopup(view(), {
      kind: "diagnostic",
      pos: 3,
      end: 6,
      body: { kind: "text", text: "unused" },
      data: { lsp: [other, mine], servers: ["other-server", "rust-analyzer"], version: 0 },
    }),
  );
  await act(async () => button("빠른 수정")!.click());
  await until(() => host.lsp!.called("call:lsp.codeAction.list").length > 0);
  expect(host.lsp!.called("call:lsp.codeAction.list")).toEqual([
    expect.objectContaining({
      from: 3,
      to: 6,
      only: ["quickfix"],
      diagnostics: { version: 0, items: [mine] },
    }),
  ]);
  await until(() => document.body.textContent!.includes("이 자리에는 코드 액션이 없다"));
  expect(currentPopup(view())).toBeNull();
});

/** 지금 떠 있는 알림의 글. sonner 의 토스트는 앞 시험의 것이 전역에 남는다 — 견줄 때는 시험 앞의 것을 빼거나 먼저 걷는다. */
const toastTexts = () =>
  [...document.querySelectorAll("[data-sonner-toast]")].map((t) => t.textContent ?? "");
/** `check` 가 참이 될 때까지(최대 2 s — 시험 시한 5 s 안에서 단언이 먼저 실패하게) 기다린다. 단언하지 않는다. */
const waitFor = async (check: () => boolean) => {
  const deadline = Date.now() + 2000;
  while (!check() && Date.now() < deadline) await settle();
};
/**
 * 앞 시험의 토스트를 걷는다. sonner 는 전역 상태이고 토스트 id 가 `등급:제목`(notifications.tsx)이라, 같은 제목이 남아 있으면 새
 * 알림이 그 자리를 덮어써 새로 뜬 것으로 보이지 않는다.
 */
async function clearToasts() {
  await act(async () => void toast.dismiss());
  await waitFor(() => toastTexts().length === 0);
  expect(toastTexts()).toEqual([]);
}

/** `run` 동안 처리되지 않은 거부를 모은다. */
async function rejectionsDuring(run: () => Promise<void>): Promise<unknown[]> {
  const rejections: unknown[] = [];
  const listener = (reason: unknown) => void rejections.push(reason);
  process.on("unhandledRejection", listener);
  try {
    await run();
    // 처리 안 된 거부는 마이크로태스크가 빈 뒤에 알려진다.
    for (let i = 0; i < 5; i++) await settle();
  } finally {
    process.off("unhandledRejection", listener);
  }
  return rejections;
}

it.each([
  { code: 5003, message: "the language server did not answer prepareRename within 10 s" },
  { code: -32603, message: "prepareRename broke on the server" },
])(
  "says once that the rename failed when prepare fails, leaving no unhandled rejection ($code)",
  async (error) => {
    renameHandlers();
    host.lsp!.handle("lsp.rename.prepare", () => Promise.reject(error));
    await openMain();
    await clearToasts();
    const rejections = await rejectionsDuring(async () => {
      await shiftF6();
      await waitFor(() => toastTexts().length > 0);
    });
    expect(rejections).toEqual([]);
    expect(toastTexts()).toEqual([
      expect.stringContaining(`이름을 바꾸지 못했다: ${error.message}`),
    ]);
    expect(document.querySelector(".cm-lsp-rename-input")).toBeNull();
    expect(host.lsp!.called("call:lsp.rename")).toEqual([]);
  },
);

/** prepare 가 까닭 없이 거절(`null`)하고 `lsp.status.get` 이 `instances` 를 줄 때 새로 뜬 알림. */
async function renameRefusedWhile(
  instances: {
    server: string;
    progress: { title: string; message: null; percentage: number } | null;
  }[],
): Promise<string[]> {
  renameHandlers();
  host.lsp!.handle("lsp.rename.prepare", async () => ({
    server: "rust-analyzer",
    staleVersion: false,
    target: { kind: "invalid", message: null },
  }));
  host.lsp!.handle("lsp.status.get", async () => ({
    projectId: "p1",
    instances: instances.map((i) => ({ ...i, root: "/repo", state: { kind: "ready" } })),
  }));
  await openMain();
  await clearToasts();
  await shiftF6();
  await waitFor(() => toastTexts().length > 0);
  expect(document.querySelector(".cm-lsp-rename-input")).toBeNull();
  return toastTexts();
}

it("says the language server is not ready yet when prepare answers null while it indexes", async () => {
  const shown = await renameRefusedWhile([
    { server: "rust-analyzer", progress: { title: "Indexing", message: null, percentage: 37 } },
  ]);
  expect(shown).toEqual([expect.stringContaining("언어 서버가 아직 준비 중이다")]);
  expect(host.lsp!.called("call:lsp.status.get")).toEqual([{ projectId: "p1" }]);
});

it("says it cannot be renamed when prepare answers null and that server shows no progress", async () => {
  // 다른 서버의 진행은 이 서버가 준비 중이라는 뜻이 아니다.
  const shown = await renameRefusedWhile([
    { server: "rust-analyzer", progress: null },
    { server: "taplo", progress: { title: "Indexing", message: null, percentage: 5 } },
  ]);
  expect(shown).toEqual([expect.stringContaining("이 자리는 이름을 바꿀 수 없다")]);
});

it("stays quiet without a server for the document (an untrusted project starts none — M4-D8)", async () => {
  renameHandlers();
  await mount();
  await open(MAIN, 1, 5);
  await act(async () => view().focus());
  const before = toastTexts();
  await shiftF6();
  await optionEnter();
  expect(host.lsp!.called("call:lsp.rename.prepare")).toEqual([]);
  expect(host.lsp!.called("call:lsp.codeAction.list")).toEqual([]);
  expect(document.querySelector(".cm-lsp-rename-input")).toBeNull();
  // 이 행의 알림(오류 · 안내)이 새로 뜨지 않았다.
  expect(toastTexts().filter((t) => !before.includes(t))).toEqual([]);
});

// ---- 리뷰 수정 회차(M4-12) ------------------------------------------------------------------------------------------------

/** 목록을 준 서버 — 문서 능력 칸의 서버(rust-analyzer)와 다르게 두어 resolve · 명령이 목록의 서버로 가는지 본다(M4). */
const LIST_SERVER = "list-server";

/** `⌥↩` → 「Fix it」(명령만) → 명령이 도는 동안. `finish` 로 명령을 끝낸다. */
async function runFixIt() {
  const run = {
    flowId: undefined as string | undefined,
    finish: undefined as (() => void) | undefined,
  };
  host.lsp!.handle("lsp.codeAction.list", async () => ({
    server: LIST_SERVER,
    staleVersion: false,
    truncated: false,
    actions: [
      {
        title: "Fix it",
        kind: "quickfix",
        isPreferred: true,
        disabled: null,
        raw: { title: "Fix it", data: 1 },
      },
    ],
  }));
  host.lsp!.handle("lsp.codeAction.resolve", async () => ({
    server: LIST_SERVER,
    edit: null,
    command: { title: "Fix it", command: "fix.it" },
  }));
  host.lsp!.handle("lsp.command.execute", (params) => {
    run.flowId = (params as { flowId: string }).flowId;
    return new Promise((resolve) => {
      run.finish = () => resolve({ server: LIST_SERVER, result: null });
    });
  });
  host.lsp!.handle("lsp.applyEdit.answer", async () => null);
  await openMain();
  await optionEnter();
  await act(async () => document.querySelector<HTMLButtonElement>(".pw-lsp-code-action")!.click());
  await until(() => run.flowId !== undefined);
  return run;
}

const libEdit = (newText = "// fixed\n") => ({
  kind: "text",
  path: LIB,
  version: null,
  foreign: false,
  edits: [{ range: range(0, 0, 0, 0), newText }],
});
const mainEdit = {
  kind: "text",
  path: MAIN,
  version: null,
  foreign: false,
  edits: [{ range: range(0, 0, 0, 0), newText: "// main\n" }],
};
const answers = () => host.lsp!.called("call:lsp.applyEdit.answer");

it("sends resolve and the command to the server that listed the action (M4)", async () => {
  const run = await runFixIt();
  expect(host.lsp!.called("call:lsp.codeAction.resolve")).toEqual([
    expect.objectContaining({ server: LIST_SERVER }),
  ]);
  expect(host.lsp!.called("call:lsp.command.execute")).toEqual([
    expect.objectContaining({ server: LIST_SERVER, flowId: run.flowId }),
  ]);
  await act(async () => run.finish!());
});

it("measures a server edit from before the command was sent — a file changed meanwhile stops it (M1)", async () => {
  const run = await runFixIt();
  // 명령을 보낸 뒤 · 서버가 편집을 계산하는 동안 LIB 가 바뀌었다.
  await emit("fs.changed", {
    root: ROOT,
    worktree: null,
    paths: [LIB],
    kinds: ["modified"],
    rescan: false,
  });
  await emit("lsp.applyEdit", {
    flowId: run.flowId,
    requestId: 1,
    server: LIST_SERVER,
    label: null,
    edit: { changes: [libEdit()] },
  });
  await until(() => answers().length > 0);
  expect(answers()).toEqual([expect.objectContaining({ requestId: 1, applied: false })]);
  expect(host.disk.text(LIB)).toBe(LIB_TEXT);
  await act(async () => run.finish!());
});

it("does not write a server edit whose command ended while the preview was open (M2)", async () => {
  const run = await runFixIt();
  await emit("lsp.applyEdit", {
    flowId: run.flowId,
    requestId: 1,
    server: LIST_SERVER,
    label: null,
    edit: { changes: [mainEdit, libEdit()] },
  });
  await until(() => document.querySelector("[data-testid='lsp-edit-preview']") !== null);
  // 미리보기를 보는 사이 명령이 끝났다 — Host 는 서버에 `applied: false` 를 이미 줬다.
  await act(async () => run.finish!());
  await settle();
  await act(async () => button("적용")!.click());
  await until(() => document.body.textContent!.includes("명령이 끝나 편집을 적용하지 않았다"));
  expect(host.disk.text(LIB)).toBe(LIB_TEXT);
  expect(host.disk.text(MAIN)).toBe(MAIN_TEXT);
  expect(view().state.doc.toString()).toBe(MAIN_TEXT);
});

it("answers applied: false when part of a server edit could not be written (L3)", async () => {
  const run = await runFixIt();
  host.disk.beforeWrite = (path) => {
    if (path !== LIB) return;
    host.disk.beforeWrite = undefined;
    host.disk.edit(LIB, "someone else\n");
  };
  await emit("lsp.applyEdit", {
    flowId: run.flowId,
    requestId: 1,
    server: LIST_SERVER,
    label: null,
    edit: { changes: [mainEdit, libEdit()] },
  });
  await until(() => document.querySelector("[data-testid='lsp-edit-preview']") !== null);
  await act(async () => button("적용")!.click());
  await until(() => answers().length > 0);
  expect(answers()).toEqual([
    expect.objectContaining({
      applied: false,
      failureReason: expect.stringContaining(LIB),
    }),
  ]);
  // 쓴 것(MAIN)은 남는다 — 알린 `failureHandling: abort`.
  expect(host.disk.text(MAIN)).toBe(`// main\n${MAIN_TEXT}`);
  expect(host.disk.text(LIB)).toBe("someone else\n");
  await act(async () => run.finish!());
});

it("stops a rename that touches a file open in another window and says so (H1)", async () => {
  renameHandlers();
  host.lsp!.handle("lsp.rename", async (params) => ({
    server: "rust-analyzer",
    staleVersion: false,
    edit: {
      changes: [
        {
          kind: "text",
          path: MAIN,
          version: (params as { version: number }).version,
          foreign: false,
          edits: [{ range: range(0, 3, 0, 6), newText: "neo" }],
        },
        { ...libEdit("neo"), foreign: true },
      ],
    },
  }));
  await openMain();
  await shiftF6();
  const input = document.querySelector<HTMLInputElement>(".cm-lsp-rename-input")!;
  input.value = "neo";
  await keydown({ key: "Enter", code: "Enter" }, input);
  await until(() => document.body.textContent!.includes("다른 창에서 연 파일이 있다"));
  expect(document.body.textContent).toContain("lib.rs");
  expect(document.querySelector("[data-testid='lsp-edit-preview']")).toBeNull();
  expect(host.disk.text(LIB)).toBe(LIB_TEXT);
  expect(host.disk.text(MAIN)).toBe(MAIN_TEXT);
  expect(view().state.doc.toString()).toBe(MAIN_TEXT);
});

it("keeps ⌥↩ in the rename input from renaming or opening code actions (L9)", async () => {
  renameHandlers();
  host.lsp!.handle("lsp.codeAction.list", async () => ({
    server: "rust-analyzer",
    staleVersion: false,
    truncated: false,
    actions: [],
  }));
  await openMain();
  await shiftF6();
  const input = document.querySelector<HTMLInputElement>(".cm-lsp-rename-input")!;
  input.value = "neo";
  await keydown({ key: "Enter", code: "Enter", altKey: true }, input);
  expect(host.lsp!.called("call:lsp.rename")).toEqual([]);
  expect(host.lsp!.called("call:lsp.codeAction.list")).toEqual([]);
  expect(document.querySelector(".cm-lsp-rename-input")).not.toBeNull();
});

it("moves the focus with ↓ through every file of the preview (M5)", async () => {
  const more = ["/work/demo/src/a.rs", "/work/demo/src/b.rs"];
  host.disk = fakeDisk(ROOT, {
    [MAIN]: MAIN_TEXT,
    [LIB]: LIB_TEXT,
    [more[0]!]: "use crate::old;\n",
    [more[1]!]: "use crate::old;\n",
  });
  renameHandlers();
  host.lsp!.handle("lsp.rename", async () => ({
    server: "rust-analyzer",
    staleVersion: false,
    edit: {
      changes: [LIB, ...more].map((path) => ({
        kind: "text",
        path,
        version: null,
        foreign: false,
        edits: [{ range: range(0, 0, 0, 0), newText: "// x\n" }],
      })),
    },
  }));
  await openMain();
  await shiftF6();
  const input = document.querySelector<HTMLInputElement>(".cm-lsp-rename-input")!;
  input.value = "neo";
  await keydown({ key: "Enter", code: "Enter" }, input);
  await until(() => document.querySelector("[data-testid='lsp-edit-preview']") !== null);
  const options = () => [...document.querySelectorAll<HTMLElement>("[role='option']")];
  expect(options()).toHaveLength(3);
  await act(async () => options()[0]!.focus());
  for (let i = 0; i < 2; i++) await keydown({ key: "ArrowDown", code: "ArrowDown" });
  expect(options()[2]!.getAttribute("aria-selected")).toBe("true");
  expect(document.activeElement).toBe(options()[2]);
  await keydown({ key: "Home", code: "Home" });
  expect(document.activeElement).toBe(options()[0]);
  await keydown({ key: "End", code: "End" });
  expect(document.activeElement).toBe(options()[2]);
  await act(async () => button("취소")!.click());
  expect(host.disk.text(LIB)).toBe(LIB_TEXT);
});
