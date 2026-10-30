// @vitest-environment happy-dom
// 레이아웃 프리셋(⌃⌥1 · ⌃⌥2, project-layout.ts 의 `applyPreset`)을 프로젝트 창(App kind="project")에서 본다: 한글 입력 상태의 키로
// 적용되고, 열린 에디터 탭 · 고정 탭 · 저장 안 한 편집을 잃지 않고, 창을 다시 열면 그 배치가 프로젝트 상태(`project.state.*`)에서
// 돌아온다.

import { EditorView } from "@codemirror/view";
import type {
  ConfigView,
  FsRead,
  FsStat,
  HostInfo,
  KeymapFile,
  LangDetectResult,
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
import { activeWorktreeStore } from "../open-with/active-worktree";
import { projectOpenInbox } from "../project-open";
import { createThemeStore } from "../theme/theme-store";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const host = vi.hoisted(() => ({
  files: new Map<string, string>(),
  /** 프로젝트 id → 저장된 프로젝트 상태(`$DATA/projects/<id>/state.json`). */
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
              version: `1:${host.files.get(path)}`,
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

let closeGuard: (() => Promise<boolean>) | undefined;
const native = {
  syncMenu: async () => {},
  onMenuAction: () => () => {},
  openSettings: async () => {},
  closeWindow: async () => {},
  guardClose: (shouldClose: () => Promise<boolean>) => {
    closeGuard = shouldClose;
    return () => {
      if (closeGuard === shouldClose) closeGuard = undefined;
    };
  },
} satisfies KeymapNative;

const LIB = "/work/demo/src/lib.rs";
const MAIN = "/work/demo/src/main.rs";

let root: Root | undefined;
let dock: DockviewApi | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
  );
  vi.spyOn(console, "info").mockImplementation(() => {});
  window.history.replaceState(null, "", "/?window=project&project=p1");
  host.files = new Map([
    [LIB, "fn lib() {}\n"],
    [MAIN, "fn main() {}\n"],
  ]);
  host.states = new Map();
  activeWorktreeStore.setState({ top: undefined });
  localStorage.clear();
});

afterEach(() => {
  act(() => root?.unmount());
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
      <StrictMode>
        <ThemeStoreProvider store={store}>
          <App kind="project" native={native} onDockApi={(api) => (dock = api)} />
        </ThemeStoreProvider>
      </StrictMode>,
    ),
  );
  await settle();
}

/** 창을 닫았다 다시 연다(닫기 확인이 프로젝트 상태를 마저 쓴다 — 앱 종료와 같은 길). */
async function restart() {
  expect(await closeGuard!()).toBe(true);
  act(() => root!.unmount());
  root = undefined;
  document.body.innerHTML = "";
  await mount();
}

async function openRequest(file: string) {
  await act(async () => projectOpenInbox.push({ file }));
  await settle();
}

/**
 * 한글 입력 상태의 ⌃⌥ 숫자. macOS 의 입력기는 이 조합을 「입력기가 처리한 키」(`key` 「Process」, keyCode 229)로 보낼 수 있다 —
 * 키맵은 `code` 로 맞춘다(M2-13 의 ⌃⇧B 와 같다).
 */
async function pressPreset(digit: 1 | 2 | 3) {
  const init = {
    code: `Digit${digit}`,
    key: "Process",
    keyCode: 229,
    ctrlKey: true,
    altKey: true,
    isComposing: false,
  };
  await act(async () => {
    (document.activeElement ?? document.body).dispatchEvent(
      new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }),
    );
    document.body.dispatchEvent(new KeyboardEvent("keyup", { bubbles: true, ...init }));
  });
  await settle();
}

function edge(dockName: "left" | "right" | "bottom") {
  const api = dock!;
  const groupApi = api.getEdgeGroup(dockName)!;
  return {
    active: api.groups.find((g) => g.id === groupApi.id)!.activePanel?.id,
    collapsed: groupApi.isCollapsed(),
  };
}

const layoutOf = () => ({ left: edge("left"), bottom: edge("bottom"), right: edge("right") });

const GIT_REVIEW = {
  left: { active: "commit", collapsed: false },
  bottom: { active: "git", collapsed: false },
  right: { active: "notifications", collapsed: true },
};
const DEFAULT = {
  left: { active: "project", collapsed: false },
  bottom: { active: "run", collapsed: false },
  right: { active: "notifications", collapsed: false },
};

const editorTabs = () =>
  [...document.querySelectorAll<HTMLElement>("[data-editor-tab-header]")].map((e) => e.title);

/** lib.rs 의 뷰(내용으로 찾는다). 그 탭이 보이는 탭이어야 한다. */
function libView(): EditorView {
  const view = [...document.querySelectorAll<HTMLElement>(".cm-editor")]
    .map((e) => EditorView.findFromDOM(e)!)
    .find((v) => v.state.doc.toString().includes("fn lib"));
  if (!view) throw new Error("lib.rs 의 뷰가 없다");
  return view;
}

const dirtyTabs = () =>
  [...document.querySelectorAll<HTMLElement>("[data-editor-tab-header][data-dirty]")].map(
    (e) => e.title,
  );

it("applies Git review with ⌃⌥2 under a Korean IME, keeps the tabs and unsaved edits, and restores it after a restart", async () => {
  await mount();
  expect(layoutOf()).toEqual(DEFAULT);
  await openRequest(LIB);
  // lib.rs 를 고치고(저장하지 않음) 고정한다.
  const lib = dock!.panels.find((p) => (p.params as { path?: string }).path === LIB)!;
  await act(async () => {
    libView().dispatch({ changes: { from: 0, insert: "x " } });
    lib.api.updateParameters({ ...(lib.params as object), pinned: true });
  });
  await openRequest(MAIN);
  const editorPanels = dock!.panels.filter((p) => p.api.component === lib.api.component);
  expect(dirtyTabs()).toEqual([LIB]);

  await pressPreset(2);
  expect(layoutOf()).toEqual(GIT_REVIEW);
  expect(editorTabs()).toEqual([LIB, MAIN]);
  // 활성 탭 · 고정 · 미저장 편집이 그대로이고, 탭을 다시 만들지 않았다(다시 만들면 뷰 · 툴 윈도우 내용이 새로 그려진다).
  expect(dock!.activePanel?.params).toMatchObject({ path: MAIN });
  expect(dock!.getPanel(lib.id)?.params).toMatchObject({ pinned: true });
  expect(dirtyTabs()).toEqual([LIB]);
  await act(async () => dock!.getPanel(lib.id)!.api.setActive());
  await settle();
  expect(libView().state.doc.toString()).toBe("x fn lib() {}\n");
  expect(dock!.panels.filter((p) => p.api.component === lib.api.component)).toEqual(editorPanels);

  // ⌃⌥3(DB)은 M3 — 등록하지 않았다.
  await pressPreset(3);
  expect(layoutOf()).toEqual(GIT_REVIEW);

  // 닫기 확인의 미저장 질문은 이 시험이 보는 것이 아니다 — 편집을 버리고 닫는다.
  await act(async () => libView().dispatch({ changes: { from: 0, to: 2, insert: "" } }));
  await settle();
  expect(dirtyTabs()).toEqual([]);
  await restart();
  expect(layoutOf()).toEqual(GIT_REVIEW);

  await pressPreset(1);
  expect(layoutOf()).toEqual(DEFAULT);
  await restart();
  expect(layoutOf()).toEqual(DEFAULT);
});
