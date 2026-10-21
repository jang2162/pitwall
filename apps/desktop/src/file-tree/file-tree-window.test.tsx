// @vitest-environment happy-dom
// 프로젝트 창(App kind="project", `?project=p1`)의 파일 트리를 가짜 Host(메모리 디스크)에 붙여 본다(12-m1-plan M1-14 완료 기준):
// 루트를 프로젝트에서 읽고, 클릭이 미리보기 탭을 열고, 외부에서 만든 파일이 `fs.changed` 로 펼친 폴더에 나타나고, ignore 토글이
// `target/` 을 숨기고 보이고, ⌥F1 「프로젝트 뷰」 가 활성 탭의 파일을 트리에서 골라 화면에 보이고, 활성 워크트리가 바뀌면 그
// 워크트리 안의 루트로 다시 뿌리내린다.

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
import { activeWorktreeStore, rememberActiveWorktree } from "../open-with/active-worktree";
import { projectOpenInbox } from "../project-open";
import { createThemeStore } from "../theme/theme-store";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";
import { resetWindowFileTree } from "./project-view";
import { fakeTreeLayout } from "./testing";
import type { FsChanged } from "./tree-model";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const host = vi.hoisted(() => ({
  /** 폴더 경로 → 이름 → 종류. 파일 내용은 경로. */
  dirs: new Map<string, Map<string, { kind: "file" | "dir"; ignored?: boolean }>>(),
  calls: [] as { method: string; params: Record<string, unknown> }[],
  emit: (_topic: string, _payload: unknown) => {},
}));

vi.mock("../tauri-transport", async () => {
  const { createMemoryTransportPair: pair } = await import("@pitwall/rpc-client");
  const { DEFAULT_KEYMAP_TOML } = await import("../keymap/default-keymap");
  const { i18nState } = await import("../i18n/testing");
  return {
    connectHost: async () => {
      const [client, server] = pair();
      const subs = new Map<number, { projectId?: string; topics: string[] }>();
      let next = 1;
      const send = (m: object) => server.send(JSON.stringify({ jsonrpc: "2.0", ...m }));
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
        if (m.method.startsWith("fs.") || m.method === "project.resolve")
          host.calls.push({ method: m.method, params: m.params });
        const path = (m.params as { path?: string } | undefined)?.path ?? "";
        const slash = path.lastIndexOf("/");
        const parent = host.dirs.get(path.slice(0, slash));
        const self = parent?.get(path.slice(slash + 1));
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
            const dir = host.dirs.get(path);
            if (!dir) return fail(`not a folder: ${path}`);
            const entries: FsEntry[] = [...dir.entries()]
              .sort(([a], [b]) => a.localeCompare(b))
              .map(([name, e]) => ({
                name,
                kind: e.kind,
                target: null,
                outside: false,
                size: 0,
                mtimeMs: 1,
                ignored: e.ignored ?? false,
              }));
            return reply({ path, entries });
          }
          case "fs.stat":
            if (!self) return fail(`not found: ${path}`);
            return reply({
              path,
              kind: self.kind,
              target: null,
              size: path.length,
              mtimeMs: 1,
              readonly: false,
              sizeClass: self.kind === "file" ? "editable" : null,
            } satisfies FsStat);
          case "fs.read":
            return reply({
              path,
              size: path.length,
              mtimeMs: 1,
              offset: 0,
              eof: true,
              version: "v1",
              content: { kind: "text", text: path },
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
          case "events.subscribe": {
            const id = next++;
            subs.set(id, m.params as { projectId?: string; topics: string[] });
            return reply({ subscriptionId: id });
          }
          case "events.unsubscribe":
            return reply(null);
          default:
            return send({ id: m.id, error: { code: -32601, message: `no ${m.method}` } });
        }
      });
      host.emit = (topic, payload) => {
        for (const [subscriptionId, s] of subs) {
          if (s.projectId !== "p1" || !s.topics.includes(topic)) continue;
          send({
            method: "events.event",
            params: { subscriptionId, projectId: "p1", topic, payload },
          });
        }
      };
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

/** `paths` 의 파일(끝이 `/` 면 폴더)을 디스크에 만든다. 조상 폴더도 만든다. */
function disk(paths: string[], ignored: string[] = []) {
  for (const p of paths) {
    const parts = p.replace(/\/$/, "").split("/");
    for (let i = 2; i <= parts.length; i++) {
      const parent = parts.slice(0, i - 1).join("/");
      const name = parts[i - 1]!;
      const isLeafFile = i === parts.length && !p.endsWith("/");
      if (!host.dirs.has(parent)) host.dirs.set(parent, new Map());
      const path = `${parent}/${name}`;
      host.dirs.get(parent)!.set(name, {
        kind: isLeafFile ? "file" : "dir",
        ignored: ignored.some((i) => path === i || path.startsWith(`${i}/`)),
      });
      if (!isLeafFile && !host.dirs.has(path)) host.dirs.set(path, new Map());
    }
  }
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
  host.dirs = new Map();
  host.calls = [];
  disk(
    [
      "/work/demo/Cargo.toml",
      "/work/demo/src/lib.rs",
      "/work/demo/src/deep/a/b/leaf.rs",
      "/work/demo/target/debug/app",
      "/work/wt/demo/only-in-worktree.txt",
    ],
    ["/work/demo/target"],
  );
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
}

const tree = () => document.querySelector<HTMLElement>("[role=tree]")!;
const row = (path: string) => document.querySelector<HTMLElement>(`[data-path="${path}"]`);
const rowNames = () =>
  [...document.querySelectorAll<HTMLElement>("[role=treeitem]")].map((r) =>
    r.getAttribute("data-path"),
  );

async function click(el: HTMLElement, type: "click" | "dblclick" = "click") {
  await act(async () => {
    el.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true }));
  });
  await settle();
}

async function press(init: KeyboardEventInit) {
  await act(async () => {
    (document.activeElement ?? document.body).dispatchEvent(
      new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }),
    );
    document.body.dispatchEvent(new KeyboardEvent("keyup", { bubbles: true, ...init }));
  });
  await settle();
}

/** 가운데 그룹의 에디터 탭: 경로와 미리보기 여부. */
function editorTabs() {
  return [...document.querySelectorAll<HTMLElement>("[data-editor-tab-header]")].map((t) => ({
    text: t.textContent,
    preview: t.hasAttribute("data-preview"),
  }));
}

it("lists the project root and opens files in a preview tab on click, a pinned tab on double click", async () => {
  await mount();
  await vi.waitFor(() => expect(row("/work/demo/src")).not.toBeNull());
  expect(rowNames()).toEqual([
    "/work/demo",
    "/work/demo/src",
    "/work/demo/target",
    "/work/demo/Cargo.toml",
  ]);
  expect(row("/work/demo/target")!.hasAttribute("data-ignored")).toBe(true);

  await click(row("/work/demo/Cargo.toml")!);
  await vi.waitFor(() => expect(editorTabs()).toEqual([{ text: "Cargo.toml", preview: true }]));
  await click(row("/work/demo/Cargo.toml")!, "dblclick");
  await vi.waitFor(() => expect(editorTabs()).toEqual([{ text: "Cargo.toml", preview: false }]));
});

// 10-ui §5.2 「미리보기 탭의 문서에 미저장 편집이 생기면 보통 탭이 된다」 · §5.4 「편집 시작 → 고정 탭」(M1-13 의 문서 모델과 맞물림).
// 보통 탭이 되지 않으면 다음 미리보기가 그 자리를 닫아(`openTab`) 문서를 놓고, 미저장 편집이 묻지도 않고 사라진다.
it("keeps a preview tab that got an unsaved edit when the next file opens in preview", async () => {
  disk(["/work/demo/README.md"]);
  await mount();
  await vi.waitFor(() => expect(row("/work/demo/src")).not.toBeNull());
  await click(row("/work/demo/src")!, "dblclick");
  await vi.waitFor(() => expect(row("/work/demo/src/lib.rs")).not.toBeNull());
  const editor = () => EditorView.findFromDOM(document.querySelector<HTMLElement>(".cm-editor")!)!;

  await click(row("/work/demo/Cargo.toml")!);
  await vi.waitFor(() => expect(document.querySelector(".cm-editor")).not.toBeNull());
  await act(async () =>
    editor().dispatch({ changes: { from: 0, insert: "x" }, userEvent: "input.type" }),
  );
  await settle();
  expect(editorTabs()).toEqual([{ text: "●Cargo.toml", preview: false }]);

  await click(row("/work/demo/src/lib.rs")!);
  await vi.waitFor(() =>
    expect(editorTabs()).toEqual([
      { text: "●Cargo.toml", preview: false },
      { text: "lib.rs", preview: true },
    ]),
  );
  // 편집하지 않은 미리보기는 다음 미리보기가 대신한다.
  await click(row("/work/demo/README.md")!);
  await vi.waitFor(() =>
    expect(editorTabs()).toEqual([
      { text: "●Cargo.toml", preview: false },
      { text: "README.md", preview: true },
    ]),
  );
  await click(row("/work/demo/Cargo.toml")!);
  await vi.waitFor(() => expect(editor().state.doc.toString()).toBe("x/work/demo/Cargo.toml"));
  expect(editorTabs()).toEqual([
    { text: "●Cargo.toml", preview: false },
    { text: "README.md", preview: true },
  ]);
});

// 완료 기준은 「외부에서 만든 파일이 500 ms 안에 펼친 폴더에 나타난다」. Host 의 감시가 이벤트를 내기까지 300 ms 안(M1-06
// 시험)이므로 창 쪽(이벤트 → 행)은 나머지 200 ms 안이어야 한다. rename 으로 들어온 경로(원자 쓰기로 새로 만든 파일 · 옮겨 온
// 파일)는 `modified` 로 온다.
it.each(["created", "modified"] as const)(
  "shows a file made outside the app (%s) in an expanded folder within 200 ms of the event",
  async (kind) => {
    await mount();
    await vi.waitFor(() => expect(row("/work/demo/src")).not.toBeNull());
    await click(row("/work/demo/src")!, "dblclick");
    await vi.waitFor(() => expect(row("/work/demo/src/lib.rs")).not.toBeNull());

    disk(["/work/demo/src/made-by-agent.rs"]);
    const started = performance.now();
    await act(async () =>
      host.emit("fs.changed", {
        root: "/work/demo",
        worktree: null,
        paths: ["/work/demo/src/made-by-agent.rs"],
        kinds: [kind],
        rescan: false,
      } satisfies FsChanged),
    );
    await vi.waitFor(() => expect(row("/work/demo/src/made-by-agent.rs")).not.toBeNull(), {
      timeout: 200,
      interval: 5,
    });
    expect(performance.now() - started).toBeLessThan(200);
    // 접힌 폴더는 다시 읽지 않는다.
    expect(
      host.calls.filter((c) => c.method === "fs.list").map((c) => c.params.path),
    ).not.toContain("/work/demo/target");
  },
);

it("hides and shows ignored target/ with the toolbar toggle", async () => {
  await mount();
  await vi.waitFor(() => expect(row("/work/demo/target")).not.toBeNull());
  const toggle = () =>
    document.querySelector<HTMLButtonElement>('[data-tool-window="project"] button[aria-pressed]')!;
  expect(toggle().getAttribute("aria-label")).toBe("무시된 파일 숨기기");
  await click(toggle());
  expect(row("/work/demo/target")).toBeNull();
  expect(toggle().getAttribute("aria-label")).toBe("무시된 파일 보이기");
  await click(toggle());
  expect(row("/work/demo/target")).not.toBeNull();
});

it("selects the active tab's file in the tree from the ⌥F1 popup and brings it into view", async () => {
  await mount();
  await vi.waitFor(() => expect(row("/work/demo/src")).not.toBeNull());
  const leaf = "/work/demo/src/deep/a/b/leaf.rs";
  await act(async () => projectOpenInbox.push({ file: leaf }));
  await settle();
  await vi.waitFor(() => expect(editorTabs().map((t) => t.text)).toEqual(["leaf.rs"]));
  expect(row(leaf)).toBeNull();

  await press({ code: "F1", key: "F1", altKey: true });
  const item = [...document.querySelectorAll<HTMLElement>("[cmdk-item]")][0]!;
  expect(item.textContent).toBe("프로젝트 뷰");
  await act(async () => item.click());
  await settle(50);

  await vi.waitFor(() => expect(row(leaf)?.getAttribute("aria-selected")).toBe("true"));
  expect(document.activeElement).toBe(tree());
  expect(tree().getAttribute("aria-activedescendant")).toBe(row(leaf)!.id);
});

it("disables 「프로젝트 뷰」 when no file is open", async () => {
  await mount();
  await vi.waitFor(() => expect(row("/work/demo/src")).not.toBeNull());
  await press({ code: "F1", key: "F1", altKey: true });
  const item = [...document.querySelectorAll<HTMLElement>("[cmdk-item]")][0]!;
  expect(item.textContent).toContain("프로젝트 뷰");
  expect(item.textContent).toContain("열린 파일이 없다");
  expect(item.getAttribute("aria-disabled")).toBe("true");
});

it("re-roots at the worktree's project root when the active worktree changes", async () => {
  await mount();
  await vi.waitFor(() => expect(row("/work/demo/src")).not.toBeNull());
  await act(async () => rememberActiveWorktree({ worktree: "/work/wt" }));
  await vi.waitFor(() => expect(row("/work/wt/demo/only-in-worktree.txt")).not.toBeNull());
  expect(row("/work/demo/src")).toBeNull();
  const list = host.calls.filter((c) => c.method === "fs.list").at(-1)!;
  expect(list.params).toEqual({ projectId: "p1", worktree: "/work/wt", path: "/work/wt/demo" });
});
