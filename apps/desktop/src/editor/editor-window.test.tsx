// @vitest-environment happy-dom
// 프로젝트 창(App kind="project", `?project=p1`)을 가짜 Host 에 붙여 에디터 탭을 본다(12-m1-plan M1-09 완료 기준): 열기 요청이 탭과
// 커서 자리가 되고, 같은 파일은 그 탭을 다시 쓰고, ⌘⌥E 가 활성 탭의 파일 · 줄 · 열을 openWith.run 에 넘기고, ⌘\ 로 나눈 두 그룹이
// 스크롤을 따로 갖고, 탭 단축키(⌘W · ⌘⌥W · ⌃Tab)가 돈다.

import { EditorView } from "@codemirror/view";
import type {
  ConfigView,
  FsRead,
  FsStat,
  HostInfo,
  KeymapFile,
  OpenWithOutcome,
  Project,
  ProjectTrust,
} from "@pitwall/rpc-client";
import { act, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../App";
import type { KeymapNative } from "../keymap/native";
import { LAYOUT_STORAGE_KEY } from "../layout/project-layout";
import { activeWorktreeStore } from "../open-with/active-worktree";
import { projectOpenInbox } from "../project-open";
import { createThemeStore } from "../theme/theme-store";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";
import { EDITOR_PANEL } from "./editor-tabs";
import { cursorOf } from "./editor-workspace";

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
  calls: [] as { method: string; params: Record<string, unknown> }[],
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
          send({ id: m.id, error: { code: -32004, message, data: null } });
        if (m.method.startsWith("fs.") || m.method.startsWith("openWith."))
          host.calls.push({ method: m.method, params: m.params });
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
          case "project.claimTrustPrompt":
            return reply(false);
          case "fs.stat":
            if (!host.files.has(real)) return fail(`not found: ${path}`);
            return reply({
              path: real,
              kind: host.links.has(path) ? "symlink" : "file",
              target: host.links.has(path) ? "file" : null,
              size: size(),
              mtimeMs: 1,
              readonly: false,
              sizeClass: sizeClass(),
            } satisfies FsStat);
          case "fs.read":
            return reply({
              path: real,
              size: size(),
              mtimeMs: 1,
              offset: 0,
              eof: true,
              version: "v1",
              content: { kind: "text", text: host.files.get(real)! },
              judgment: {
                binary: false,
                encoding: "UTF-8",
                bom: null,
                lineEnding: "lf",
                longLines: false,
                sizeClass: sizeClass(),
                editable: sizeClass() === "editable",
                highlight: sizeClass() === "editable",
              },
            } satisfies FsRead);
          case "openWith.run":
            return reply({
              kind: "launched",
              tool: "vscode",
              label: "Visual Studio Code",
              argv: ["/usr/local/bin/code"],
              pid: 1,
            } satisfies OpenWithOutcome);
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

const native: KeymapNative = {
  syncMenu: async () => {},
  onMenuAction: () => () => {},
  openSettings: async () => {},
};

const LIB = "/work/demo/src/lib.rs";
const MAIN = "/work/demo/src/main.rs";
const README = "/work/demo/README.txt";

/** `n` 줄짜리 텍스트. 줄마다 그 줄 번호가 든다. */
function lines(n: number): string {
  return Array.from({ length: n }, (_, i) => `line ${i + 1} — some text`).join("\n");
}

let root: Root | undefined;

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
  host.calls = [];
  activeWorktreeStore.setState({ top: undefined });
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
  const store = createThemeStore({
    system: { current: () => "light", subscribe: () => () => {} },
    storage: { read: () => undefined, write: () => {} },
    theme: parseTheme(defaultTheme),
    apply: () => {},
  });
  root = createRoot(element);
  await act(async () =>
    root!.render(
      <StrictMode>
        <ThemeStoreProvider store={store}>
          <App kind="project" native={native} />
        </ThemeStoreProvider>
      </StrictMode>,
    ),
  );
  await settle();
}

/** 창이 받은 열기 요청(`pitwall <파일>:<줄>:<열>`). */
async function openRequest(file: string, line?: number, col?: number) {
  await act(async () => projectOpenInbox.push({ file, line, col }));
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

it("passes the active worktree to file reads and to openWith.run", async () => {
  await mount();
  await act(async () => activeWorktreeStore.setState({ top: "/work/demo-wt" }));
  await openRequest(LIB, 2, 1);
  expect(host.calls.find((c) => c.method === "fs.stat")!.params).toMatchObject({
    projectId: "p1",
    worktree: "/work/demo-wt",
    path: LIB,
  });
  await press("KeyE", { metaKey: true, altKey: true }, "´");
  expect(runCalls()[0]).toMatchObject({ worktree: "/work/demo-wt", file: LIB, line: 2 });
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

it("does not bring editor tabs back from the saved layout", async () => {
  await mount();
  await openRequest(LIB);
  // 저장본에는 탭이 든다(dockview 의 toJSON 그대로). 읽을 때 뺀다 — 탭 복원은 프로젝트 · 워크트리별 상태(M1-25)가 한다.
  await vi.waitFor(
    () => expect(localStorage.getItem(LAYOUT_STORAGE_KEY)).toContain(`"${EDITOR_PANEL}"`),
    { timeout: 2000 },
  );
  act(() => root?.unmount());
  root = undefined;
  await mount();
  expect(editorTabs()).toEqual([]);
  expect(document.querySelector("[data-testid=editor-empty]")).not.toBeNull();
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
