// @vitest-environment happy-dom
// 웰컴 창(App kind="welcome")을 가짜 Host · 가짜 네이티브(native.ts)에 붙여 S1 → S2 → S3 흐름을 키 · 클릭으로 돌린다.
// 앱(main.tsx)처럼 StrictMode 로 렌더한다 — effect 를 붙였다 떼고 다시 붙여도 흐름이 깨지지 않아야 한다.
// 프로젝트 창 열기 · 보류 요청은 M0-10 이 만드는 Tauri 명령이라 여기서는 네이티브 어댑터를 가짜로 바꿔 부른 인자만 본다.

import type {
  ConfigView,
  CreateProjectParams,
  HostInfo,
  KeymapFile,
  OpenWithList,
  OpenWithOutcome,
  OpenWithRunParams,
  Project,
  Resolution,
  RootCandidate,
  StorageStatus,
} from "@pitwall/rpc-client";
import { act, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../App";
import type { KeymapNative } from "../keymap/native";
import { createThemeStore } from "../theme/theme-store";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";
import type { DragDropEvent, FindProjectRequest, WelcomeNative } from "./native";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const host = vi.hoisted(() => ({
  projects: [] as Project[],
  storage: { kind: "ok", path: "/d/state.db" } as StorageStatus,
  candidates: [] as RootCandidate[],
  resolve: (_path: string): Resolution | null => null,
  /** 쓰기(만들기 · 지우기)를 STORAGE_UNAVAILABLE 로 거절하고 저장소를 이 상태로 바꾼다. */
  failWrites: undefined as StorageStatus | undefined,
  calls: [] as { method: string; params: unknown }[],
  emitHostEvent: (_topic: string) => {},
  /** `i18n.get` 이 고른 언어. 시험은 기본으로 한국어에 고정한다(브라우저 언어에 기대지 않는다). */
  locale: "ko",
  openWithList: { tools: [], default: { kind: "unset" } } as OpenWithList,
  openWithRun: (_params: OpenWithRunParams): OpenWithOutcome => ({
    kind: "noDefault",
    configured: null,
  }),
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
      server.onMessage((text) => {
        const m = JSON.parse(text) as { id?: number; method: string; params: never };
        const reply = (result: unknown) => send({ id: m.id, result });
        const fail = (message: string) => send({ id: m.id, error: { code: -32602, message } });
        host.calls.push({ method: m.method, params: m.params });
        if (host.failWrites && (m.method === "project.delete" || m.method === "project.create")) {
          host.storage = host.failWrites;
          return send({
            id: m.id,
            error: { code: -32003, message: "storage unavailable", data: "locked" },
          });
        }
        switch (m.method) {
          case "i18n.get":
            return reply(i18nState(host.locale));
          case "i18n.setLanguage":
            host.locale = (m.params as { language: string }).language;
            return reply(null);
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
          case "events.subscribe": {
            const id = next++;
            subs.set(id, m.params);
            return reply({ subscriptionId: id });
          }
          case "events.unsubscribe":
            return reply(null);
          case "project.list":
            return reply([...host.projects].sort((a, b) => a.name.localeCompare(b.name)));
          case "project.storage":
            return reply(host.storage);
          case "openWith.list":
            return reply(host.openWithList);
          case "openWith.run":
            return reply(host.openWithRun(m.params as OpenWithRunParams));
          case "project.candidates":
            return reply(host.candidates);
          case "project.resolve":
            return reply(host.resolve((m.params as { path: string }).path));
          case "project.delete": {
            const { id } = m.params as { id: string };
            host.projects = host.projects.filter((p) => p.id !== id);
            return reply(null);
          }
          case "project.create": {
            const p = m.params as CreateProjectParams;
            if (host.projects.some((x) => x.roots[0] === p.root))
              return fail("already a project root");
            const created = project(`n${host.projects.length}`, p.name ?? "?", p.root);
            host.projects.push(created);
            return reply(created);
          }
        }
      });
      host.emitHostEvent = (topic) => {
        for (const [subscriptionId, s] of subs) {
          if (s.projectId !== undefined || !s.topics.includes(topic)) continue;
          send({
            method: "events.event",
            params: { subscriptionId, projectId: null, topic, payload: {} },
          });
        }
      };
      return { transport: client, close: async () => {} };
    },
  };
});

function project(id: string, name: string, root: string, extra: Partial<Project> = {}): Project {
  return {
    id,
    name,
    host: "local",
    roots: [root],
    favorite: false,
    tags: [],
    configMode: "repository",
    slug: name,
    createdAt: 1,
    openedAt: null,
    ...extra,
  };
}

let menuListener: ((action: string) => void) | undefined;
const keymapNative = {
  syncMenu: async () => {},
  onMenuAction: (listener: (action: string) => void) => {
    menuListener = listener;
    return () => {
      if (menuListener === listener) menuListener = undefined;
    };
  },
  openSettings: vi.fn<KeymapNative["openSettings"]>(async () => {}),
} satisfies KeymapNative;

let pending: FindProjectRequest | null = null;
let findListener: (() => void) | undefined;
let dropListener: ((e: DragDropEvent) => void) | undefined;
let pickedFolder: string | null = null;
let pathKinds: Record<string, "file" | "dir" | "missing"> = {};
const native = {
  openProjectWindow: vi.fn<WelcomeNative["openProjectWindow"]>(async () => {}),
  takePendingFindProject: vi.fn<WelcomeNative["takePendingFindProject"]>(async () => {
    const taken = pending;
    pending = null;
    return taken;
  }),
  onFindProjectRequested: vi.fn<WelcomeNative["onFindProjectRequested"]>((l) => {
    findListener = l;
    return () => (findListener = undefined);
  }),
  pickFolder: vi.fn<WelcomeNative["pickFolder"]>(async () => pickedFolder),
  reveal: vi.fn<WelcomeNative["reveal"]>(async () => {}),
  onDragDrop: vi.fn<WelcomeNative["onDragDrop"]>((l) => {
    dropListener = l;
    return () => (dropListener = undefined);
  }),
  pathKind: vi.fn<WelcomeNative["pathKind"]>(async (path) => pathKinds[path] ?? "dir"),
} satisfies WelcomeNative;

let root: Root | undefined;
const log = { info: vi.fn(), warn: vi.fn(), debug: vi.fn() };

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
  );
  host.projects = [
    project("a", "web-console", "/w/web-console", { openedAt: 20 }),
    project("b", "billing-api", "/w/billing-api", { favorite: true, openedAt: 10, tags: ["work"] }),
    project("c", "infra", "/w/infra"),
  ];
  host.storage = { kind: "ok", path: "/d/state.db" };
  host.candidates = [];
  host.resolve = () => null;
  host.calls = [];
  pending = null;
  pickedFolder = null;
  pathKinds = {};
  host.failWrites = undefined;
  host.locale = "ko";
  host.openWithList = { tools: [], default: { kind: "unset" } };
  host.openWithRun = () => ({ kind: "noDefault", configured: null });
  for (const fn of Object.values(native)) fn.mockClear();
  keymapNative.openSettings.mockClear();
  log.info.mockClear();
  log.warn.mockClear();
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
          <App kind="welcome" native={keymapNative} welcomeNative={native} log={log} />
        </ThemeStoreProvider>
      </StrictMode>,
    ),
  );
  await settle();
}

async function press(code: string, mods: Partial<KeyboardEventInit> = {}, key = code) {
  let prevented = false;
  await act(async () => {
    const event = new KeyboardEvent("keydown", {
      code,
      key,
      bubbles: true,
      cancelable: true,
      ...mods,
    });
    (document.activeElement ?? document.body).dispatchEvent(event);
    prevented = event.defaultPrevented;
    document.body.dispatchEvent(new KeyboardEvent("keyup", { code, key, bubbles: true, ...mods }));
  });
  await settle();
  return prevented;
}

async function type(input: HTMLInputElement, value: string) {
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await settle();
}

/**
 * 한글 입력기가 실제로 보내는 순서를 흉내 낸다: compositionstart → 조합 중 keydown(isComposing, keyCode 229) → input →
 * compositionend. 값은 조합이 끝난 글자까지 넣는다.
 */
async function compose(input: HTMLInputElement, value: string) {
  await act(async () => {
    input.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
    input.dispatchEvent(
      new KeyboardEvent("keydown", {
        code: "KeyR",
        key: "Process",
        keyCode: 229,
        isComposing: true,
        bubbles: true,
        cancelable: true,
      }),
    );
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, value);
    input.dispatchEvent(new InputEvent("input", { bubbles: true, isComposing: true }));
    input.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: value }));
  });
  await settle();
}

const rows = () => [...document.querySelectorAll<HTMLElement>("[cmdk-root] [cmdk-item]")];
const rowNames = () => rows().map((r) => r.querySelector("span:nth-of-type(1)")?.textContent);
const selectedRow = () => document.querySelector<HTMLElement>('[cmdk-item][data-selected="true"]');
const searchInput = () => document.querySelector<HTMLInputElement>("[cmdk-input]")!;
const sheet = () =>
  document.querySelector<HTMLElement>('[data-keymap-context="createProjectSheet"]');
const calls = (method: string) => host.calls.filter((c) => c.method === method);
const button = (text: string) =>
  [...document.querySelectorAll<HTMLButtonElement>("button")].find((b) =>
    b.textContent?.includes(text),
  );

it("lists favorites first, then recent, then the rest, and opens the selected row with Enter", async () => {
  await mount();
  expect(rowNames()).toEqual(["billing-api", "web-console", "infra"]);
  const first = rows()[0]!;
  expect(first.textContent).toContain("/w/billing-api");
  expect(first.textContent).toContain("work");
  expect(first.querySelector('[aria-label="즐겨찾기"]')).not.toBeNull();

  await press("ArrowDown");
  expect(selectedRow()?.textContent).toContain("web-console");
  await press("Enter");
  expect(native.openProjectWindow).toHaveBeenCalledWith({ projectId: "a" });
});

it("selects on click and opens on double click", async () => {
  await mount();
  const infra = rows()[2]!;
  await act(async () => {
    infra.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
    infra.click();
  });
  await settle();
  expect(selectedRow()?.textContent).toContain("infra");
  expect(native.openProjectWindow).not.toHaveBeenCalled();
  await act(async () => infra.dispatchEvent(new MouseEvent("dblclick", { bubbles: true })));
  expect(native.openProjectWindow).toHaveBeenCalledWith({ projectId: "c" });
});

it("removes the selected project with ⌘⌫ only while the search box is empty", async () => {
  await mount();
  await type(searchInput(), "infra");
  expect(rowNames()).toEqual(["infra"]);
  // 검색어가 있으면 ⌘⌫ 는 입력 칸의 것(검색어 지우기)이다.
  expect(await press("Backspace", { metaKey: true })).toBe(false);
  expect(calls("project.delete")).toEqual([]);

  await type(searchInput(), "");
  expect(selectedRow()?.textContent).toContain("billing-api");
  expect(await press("Backspace", { metaKey: true })).toBe(true);
  expect(calls("project.delete")).toEqual([{ method: "project.delete", params: { id: "b" } }]);
  expect(rowNames()).toEqual(["web-console", "infra"]);
});

it("filters by a half-composed hangul query and keeps shortcuts working in the search box", async () => {
  host.projects.push(project("k", "결제-서버", "/w/pay"));
  await mount();
  await type(searchInput(), "결ㅈ");
  expect(rowNames()).toEqual(["결제-서버"]);
  // 조합을 확정하는 Enter 는 열지 않는다.
  await press("Enter", { isComposing: true }, "Enter");
  expect(native.openProjectWindow).not.toHaveBeenCalled();
  // 한글 입력 상태의 ⌘O(key 가 'ㅐ')는 code 로 맞아 폴더 열기가 돈다.
  expect(await press("KeyO", { metaKey: true }, "ㅐ")).toBe(true);
  expect(native.pickFolder).toHaveBeenCalledTimes(1);
  await press("Enter");
  expect(native.openProjectWindow).toHaveBeenCalledWith({ projectId: "k" });
});

it("takes a pending find request on mount and walks S2 → S3 → the project window", async () => {
  pending = { path: "/w/app/wt/src/App.kt", line: 12, col: 3 };
  host.candidates = [
    { path: "/w/app", worktreeMain: true, vcs: false, buildFiles: [] },
    { path: "/w/app/wt/src", worktreeMain: false, vcs: false, buildFiles: ["build.gradle.kts"] },
  ];
  host.resolve = (path) =>
    path === "/w/app/wt/src/App.kt" && host.projects.some((p) => p.roots[0] === "/w/app")
      ? {
          project: host.projects.find((p) => p.roots[0] === "/w/app")!,
          root: "/w/app",
          path,
          worktree: { top: "/w/app/wt", root: "/w/app/wt" },
        }
      : null;
  await mount();
  // StrictMode 는 쿼리를 두 번 부를 수 있다 — 같은 경로로만 불렸는지 본다.
  expect(new Set(calls("project.candidates").map((c) => JSON.stringify(c.params)))).toEqual(
    new Set([JSON.stringify({ path: "/w/app/wt/src/App.kt" })]),
  );
  const dialog = document.querySelector("[cmdk-dialog]")!;
  expect(dialog.textContent).toContain("/w/app/wt/src/App.kt 를 열 프로젝트");
  const items = [...dialog.querySelectorAll<HTMLElement>("[cmdk-item]")];
  expect(items.map((i) => i.textContent)).toEqual([
    "/w/app워크트리의 메인 저장소 (추천)",
    "/w/app/wt/srcbuild.gradle.kts",
    "폴더 직접 선택…",
    "프로젝트 없이 파일만 열기단일 파일 뷰어 창은 아직 없다",
  ]);
  expect(items[3]!.getAttribute("aria-disabled")).toBe("true");

  await press("Enter");
  expect(document.querySelector("[cmdk-dialog]")).toBeNull();
  expect(sheet()?.textContent).toContain("새 프로젝트: app");
  expect(sheet()?.textContent).toContain("감지된 항목이 없다");
  await act(async () =>
    sheet()!.querySelector<HTMLInputElement>('input[value="personal"]')!.click(),
  );

  expect(await press("Enter", { metaKey: true })).toBe(true);
  expect(calls("project.create").map((c) => c.params)).toEqual([
    { root: "/w/app", name: "app", configMode: "personal" },
  ]);
  expect(sheet()).toBeNull();
  expect(native.openProjectWindow).toHaveBeenCalledWith({
    projectId: "n3",
    worktree: "/w/app/wt",
    file: "/w/app/wt/src/App.kt",
    line: 12,
    col: 3,
  });
});

it("opens S2 when a find request arrives later, and goes back to it when the folder dialog is cancelled", async () => {
  await mount();
  expect(native.takePendingFindProject).toHaveBeenCalled();
  expect(document.querySelector("[cmdk-dialog]")).toBeNull();

  pending = { path: "/w/x/file.txt" };
  await act(async () => findListener!());
  await settle();
  const pick = [...document.querySelectorAll<HTMLElement>("[cmdk-dialog] [cmdk-item]")].find(
    (i) => i.textContent === "폴더 직접 선택…",
  )!;
  await act(async () => pick.click());
  await settle();
  expect(native.pickFolder).toHaveBeenCalledWith("/w/x");
  expect(document.querySelector("[cmdk-dialog]")).not.toBeNull();

  await press("Escape", {}, "Escape");
  expect(document.querySelector("[cmdk-dialog]")).toBeNull();
  expect(sheet()).toBeNull();
});

it("keeps window shortcuts away from the list while the sheet is open", async () => {
  await mount();
  expect(await press("KeyN", { metaKey: true })).toBe(true);
  expect(sheet()).not.toBeNull();
  // 시트가 열려 있으면 ⌘⌫ · ⌘O 는 뒤의 목록 · 창 액션으로 가지 않는다.
  expect(await press("Backspace", { metaKey: true })).toBe(false);
  expect(await press("KeyO", { metaKey: true })).toBe(false);
  expect(calls("project.delete")).toEqual([]);
  expect(native.pickFolder).not.toHaveBeenCalled();
  // 루트가 없으면 만들 수 없다 — ⌘↩ 는 키를 가져가지 않는다.
  expect(await press("Enter", { metaKey: true })).toBe(false);

  pickedFolder = "/w/new-thing";
  await act(async () => button("선택…")!.click());
  await settle();
  expect(sheet()!.querySelector<HTMLInputElement>("input")!.value).toBe("new-thing");
  await act(async () => button("취소")!.click());
  await settle();
  expect(sheet()).toBeNull();
  expect(calls("project.create")).toEqual([]);
  // 시트가 닫히면 창 단축키가 돌아온다.
  expect(await press("Backspace", { metaKey: true })).toBe(true);
  expect(calls("project.delete")).toHaveLength(1);
});

it("opens a dropped folder that is already a project, and offers S3 for a new one", async () => {
  host.resolve = (path) =>
    path === "/w/infra"
      ? { project: host.projects[2]!, root: "/w/infra", path, worktree: null }
      : path === "/w/infra/sub"
        ? { project: host.projects[2]!, root: "/w/infra", path, worktree: null }
        : null;
  await mount();
  await act(async () => dropListener!({ kind: "enter" }));
  expect(document.body.textContent).toContain("폴더를 놓으면 새 프로젝트를 만든다");
  await act(async () => dropListener!({ kind: "drop", paths: ["/w/infra"] }));
  await settle();
  expect(document.body.textContent).not.toContain("폴더를 놓으면");
  expect(native.openProjectWindow).toHaveBeenCalledWith({ projectId: "c" });
  expect(sheet()).toBeNull();

  await act(async () => dropListener!({ kind: "drop", paths: ["/w/infra/sub"] }));
  await settle();
  expect(sheet()?.textContent).toContain("새 프로젝트: sub");
  expect(sheet()?.textContent).toContain("/w/infra/sub");
});

it("shows the create error in the sheet and keeps it open", async () => {
  await mount();
  pickedFolder = "/w/infra";
  host.resolve = () => null;
  await press("KeyO", { metaKey: true });
  expect(sheet()).not.toBeNull();
  await press("Enter", { metaKey: true });
  expect(sheet()?.querySelector('[role="alert"]')?.textContent).toContain("already a project root");
  expect(native.openProjectWindow).not.toHaveBeenCalled();
});

it("reveals and removes from the context menu, and keeps window shortcuts away while it is open", async () => {
  await mount();
  const menuItems = () => [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')];
  const openMenu = async (row: HTMLElement) => {
    await act(async () =>
      row.dispatchEvent(
        new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 5, clientY: 5 }),
      ),
    );
    await settle();
  };
  await openMenu(rows()[2]!);
  expect(selectedRow()?.textContent).toContain("infra");
  // 메뉴가 열린 동안 창 단축키는 꺼져 있으므로 메뉴에 단축키를 적지 않는다.
  expect(menuItems().map((i) => i.textContent)).toEqual([
    "열기",
    "Finder 에서 보기",
    "다음에서 열기…",
    "목록에서 제거",
  ]);
  expect(await press("KeyN", { metaKey: true })).toBe(false);
  expect(sheet()).toBeNull();
  expect(await press("Backspace", { metaKey: true })).toBe(false);
  expect(calls("project.delete")).toEqual([]);

  await act(async () => menuItems()[1]!.click());
  await settle();
  expect(native.reveal).toHaveBeenCalledWith("/w/infra");

  // 실패 문구도 메뉴와 같은 이름(macOS 면 Finder)으로 말한다.
  native.reveal.mockRejectedValueOnce(new Error("no such folder"));
  await openMenu(rows()[2]!);
  await act(async () => menuItems()[1]!.click());
  await settle();
  expect(document.querySelector('[role="alert"]')?.textContent).toBe(
    "Finder 에서 보기 실패: no such folder",
  );

  await openMenu(rows()[2]!);
  await act(async () => menuItems()[3]!.click());
  await settle();
  expect(calls("project.delete").map((c) => c.params)).toEqual([{ id: "c" }]);
  expect(rowNames()).toEqual(["billing-api", "web-console"]);
});

it("shows a non-ok registry as a banner and disables writes when it is read-only", async () => {
  host.storage = { kind: "readOnly", path: "/d/state.db", reason: "newer schema" };
  await mount();
  expect(document.querySelector('[role="status"]')?.textContent).toContain("newer schema");
  expect(button("새 프로젝트")!.disabled).toBe(true);
  expect(await press("KeyN", { metaKey: true })).toBe(false);
  expect(await press("Backspace", { metaKey: true })).toBe(false);
});

it("refetches the list when the registry publishes project.changed", async () => {
  await mount();
  host.projects.push(project("d", "docs", "/w/docs"));
  expect(rowNames()).not.toContain("docs");
  await act(async () => host.emitHostEvent("project.changed"));
  await settle();
  expect(rowNames()).toContain("docs");
});

/** S2 를 띄운다: 보류 요청 → 알림. */
async function openFind(request: FindProjectRequest) {
  pending = request;
  await act(async () => findListener!());
  await settle();
}

const findDialog = () => document.querySelector("[cmdk-dialog]");
const findItem = (text: string) =>
  [...document.querySelectorAll<HTMLElement>("[cmdk-dialog] [cmdk-item]")].find((i) =>
    i.textContent?.startsWith(text),
  )!;

it("does not pass a folder as the file to open (a worktree directory from `pitwall <dir>`)", async () => {
  host.candidates = [{ path: "/w/app", worktreeMain: true, vcs: false, buildFiles: [] }];
  host.resolve = (path) => {
    const app = host.projects.find((p) => p.roots[0] === "/w/app");
    return app && path === "/w/app/.worktrees/x"
      ? { project: app, root: "/w/app", path, worktree: { top: path, root: path } }
      : null;
  };
  await mount();
  await openFind({ path: "/w/app/.worktrees/x", line: 4 });
  await press("Enter");
  await press("Enter", { metaKey: true });
  expect(native.openProjectWindow).toHaveBeenCalledWith({
    projectId: "n3",
    worktree: "/w/app/.worktrees/x",
  });
});

it("opens only the project window when the chosen folder does not hold the file", async () => {
  await mount();
  await openFind({ path: "/w/x/file.txt", line: 3 });
  pickedFolder = "/elsewhere/proj";
  await act(async () => findItem("폴더 직접 선택").click());
  await settle();
  await press("Enter", { metaKey: true });
  expect(calls("project.create").map((c) => c.params)).toEqual([
    { root: "/elsewhere/proj", name: "proj", configMode: "repository" },
  ]);
  expect(native.openProjectWindow).toHaveBeenCalledWith({ projectId: "n3" });
  expect(document.querySelector('[role="status"]')?.textContent).toContain(
    "/w/x/file.txt 를 담지 않아",
  );
});

it("keeps window shortcuts away from the list while S2 is open", async () => {
  await mount();
  await openFind({ path: "/w/x/file.txt" });
  expect(findDialog()).not.toBeNull();
  expect(await press("Backspace", { metaKey: true })).toBe(false);
  expect(await press("KeyO", { metaKey: true })).toBe(false);
  expect(calls("project.delete")).toEqual([]);
  expect(native.pickFolder).not.toHaveBeenCalled();
});

it("drops native menu actions that would act behind an open sheet, but still opens settings", async () => {
  await mount();
  await press("KeyN", { metaKey: true });
  expect(sheet()).not.toBeNull();
  await act(async () => menuListener!("action.find"));
  await settle();
  expect(findDialog()).toBeNull();
  await act(async () => menuListener!("settings.open"));
  await settle();
  expect(keymapNative.openSettings).toHaveBeenCalledTimes(1);
  expect(sheet()).not.toBeNull();

  // 시트를 닫으면 메뉴의 액션 찾기가 다시 돈다.
  await press("Escape", {}, "Escape");
  await act(async () => menuListener!("action.find"));
  await settle();
  expect(findDialog()).not.toBeNull();
});

it("refreshes the storage banner as soon as a write is refused as STORAGE_UNAVAILABLE", async () => {
  await mount();
  expect(document.querySelector('[role="status"]')).toBeNull();
  host.failWrites = { kind: "memory", path: "/d/state.db", reason: "database is locked" };
  expect(await press("Backspace", { metaKey: true })).toBe(true);
  expect(document.querySelector('[role="alert"]')?.textContent).toContain("storage unavailable");
  expect(document.querySelector('[role="status"]')?.textContent).toContain("database is locked");
  expect(button("새 프로젝트")!.disabled).toBe(true);
});

it("opens a dropped file in the project that holds it, and offers S2 for any other file", async () => {
  pathKinds = { "/w/infra/src/a.rs": "file", "/w/x/b.txt": "file" };
  host.resolve = (path) =>
    path.startsWith("/w/infra/")
      ? { project: host.projects[2]!, root: "/w/infra", path, worktree: null }
      : null;
  await mount();
  await act(async () => dropListener!({ kind: "drop", paths: ["/w/infra/src/a.rs"] }));
  await settle();
  expect(native.openProjectWindow).toHaveBeenCalledWith({
    projectId: "c",
    file: "/w/infra/src/a.rs",
  });
  expect(sheet()).toBeNull();

  await act(async () => dropListener!({ kind: "drop", paths: ["/w/x/b.txt"] }));
  await settle();
  expect(findDialog()?.textContent).toContain("/w/x/b.txt 를 열 프로젝트");
  expect(sheet()).toBeNull();
});

it("follows the real IME order: a composing Esc keeps S3 and S2 open, the Enter after composing does not open", async () => {
  host.projects.push(project("k", "결제-서버", "/w/pay"));
  await mount();
  await compose(searchInput(), "결제");
  expect(rowNames()).toEqual(["결제-서버"]);
  // WebKit 은 조합을 끝낸 Enter 를 isComposing false · keyCode 229 로 보낸다.
  await press("Enter", { keyCode: 229 }, "Enter");
  expect(native.openProjectWindow).not.toHaveBeenCalled();

  await press("KeyN", { metaKey: true });
  const name = sheet()!.querySelector<HTMLInputElement>("input")!;
  await compose(name, "새");
  await press("Escape", { isComposing: true, keyCode: 229 }, "Escape");
  expect(sheet()).not.toBeNull();
  // WebKit 은 조합을 끝내는 키를 isComposing 없이 keyCode 229 로 보내기도 한다.
  await press("Escape", { keyCode: 229 }, "Escape");
  expect(sheet()).not.toBeNull();
  await press("Escape", {}, "Escape");
  expect(sheet()).toBeNull();

  await openFind({ path: "/w/x/file.txt" });
  await compose(document.querySelector<HTMLInputElement>("[cmdk-dialog] [cmdk-input]")!, "폴");
  await press("Escape", { isComposing: true, keyCode: 229 }, "Escape");
  expect(findDialog()).not.toBeNull();
  await press("Escape", {}, "Escape");
  expect(findDialog()).toBeNull();
});

it("logs what it asks the native side to do, and every failure", async () => {
  await mount();
  native.openProjectWindow.mockRejectedValueOnce(new Error("unknown project"));
  await press("Enter");
  expect(log.info).toHaveBeenCalledWith('open project window: {"projectId":"b"}', "welcome");
  expect(log.warn).toHaveBeenCalledWith("프로젝트 열기 failed: unknown project", "welcome");
});

/** 상태바의 테마 고르기: 묶음 이름과 「시스템」 버튼 이름. */
const footerThemeLabel = () => {
  const system = document.querySelector('footer [data-testid="theme-system"]');
  return `${system?.parentElement?.getAttribute("aria-label")} / ${system?.getAttribute("aria-label")}`;
};

it("draws the whole welcome window in English when the host picks en", async () => {
  host.locale = "en";
  await mount();
  expect(searchInput().placeholder).toBe("Search projects");
  expect(document.querySelector("nav")?.getAttribute("aria-label")).toBe("Welcome");
  expect(document.querySelector("nav")?.textContent).toBe("ProjectsRemote HostsExtensions");
  expect(button("New Project…")).toBeDefined();
  expect(button("Open Folder…")).toBeDefined();
  expect(rows()[0]!.querySelector('[aria-label="Favorite"]')).not.toBeNull();
  expect(footerThemeLabel()).toBe("Theme / System Theme");
  // 보이는 글자뿐 아니라 툴팁 · 화면 읽기 이름 · 자리 표시 글자에도 한국어가 남지 않는다.
  const hangul = () =>
    [
      document.body.textContent ?? "",
      ...[...document.querySelectorAll("[title], [aria-label], [placeholder]")].flatMap((e) =>
        ["title", "aria-label", "placeholder"].map((name) => e.getAttribute(name) ?? ""),
      ),
    ].filter((text) => /[가-힣]/.test(text));
  expect(hangul()).toEqual([]);

  await openFind({ path: "/w/x/file.txt" });
  expect(findDialog()?.textContent).toContain("Choose a project for /w/x/file.txt");
  expect(findDialog()?.textContent).toContain("Choose Folder…");
  await press("Escape", {}, "Escape");
  await press("KeyN", { metaKey: true });
  expect(sheet()?.textContent).toContain("New Project: ");
  expect(sheet()?.textContent).toContain("Nothing detected");
  expect(hangul()).toEqual([]);
});

/** 한글 조합 필터는 UI 언어와 무관하다 — 영어 화면에서도 조합 중인 검색어로 목록이 맞는다. */
it("filters by a composing hangul query in the English UI too", async () => {
  host.locale = "en";
  host.projects.push(project("k", "결제-서버", "/w/pay"));
  await mount();
  await compose(searchInput(), "결ㅈ");
  expect(rowNames()).toEqual(["결제-서버"]);
  await press("Enter", { keyCode: 229 }, "Enter");
  expect(native.openProjectWindow).not.toHaveBeenCalled();
});

/**
 * 액션 찾기의 언어 액션 → `i18n.setLanguage` → Host 범위 `config.changed` → `i18n.get` 다시 부르기. 창을 다시 띄우지 않고 바뀐다 —
 * 검색어 · 선택이 그대로 남는다.
 */
it("switches the language from Find Action without remounting the window", async () => {
  await mount();
  expect(button("새 프로젝트…")).toBeDefined();
  await type(searchInput(), "bill");

  await press("KeyA", { metaKey: true, shiftKey: true }, "A");
  const languageAction = [
    ...document.querySelectorAll<HTMLElement>("[cmdk-dialog] [cmdk-item]"),
  ].find((e) => e.textContent === "언어: English");
  expect(languageAction).toBeDefined();
  await act(async () => languageAction!.click());
  await settle();
  expect(calls("i18n.setLanguage").map((c) => c.params)).toEqual([{ language: "en" }]);

  await act(async () => host.emitHostEvent("config.changed"));
  await settle();
  expect(button("New Project…")).toBeDefined();
  expect(searchInput().placeholder).toBe("Search projects");
  expect(searchInput().value).toBe("bill");
  expect(rowNames()).toEqual(["billing-api"]);
  expect(footerThemeLabel()).toBe("Theme / System Theme");

  // 액션 이름도 바뀐다.
  await press("KeyA", { metaKey: true, shiftKey: true }, "A");
  const names = [...document.querySelectorAll("[cmdk-dialog] [cmdk-item]")].map(
    (e) => e.textContent,
  );
  expect(names).toContain("Language: 한국어");
  expect(names.some((n) => n?.startsWith("New Project"))).toBe(true);
});

describe("open with an external tool", () => {
  const tool = (id: string, label: string, problem: string | null = null) => ({
    id,
    label,
    extension: "dev.pitwall.external-tools",
    fromSettings: false,
    opensFiles: true,
    problem,
  });
  const popupItems = () => [...document.querySelectorAll<HTMLElement>("[cmdk-dialog] [cmdk-item]")];
  const popupText = () => document.querySelector("[cmdk-dialog]")?.textContent ?? "";
  const alertText = () => document.querySelector('[role="alert"]')?.textContent;

  beforeEach(() => {
    host.openWithList = {
      tools: [
        tool("zed", "Zed"),
        tool("idea", "IntelliJ IDEA", "`command` is empty"),
        tool("finder", "Finder"),
      ],
      default: { kind: "tool", id: "finder" },
    };
  });

  it("opens the popup from the context menu, marks the default and runs the picked tool", async () => {
    host.openWithRun = (params) => ({
      kind: "launched",
      tool: params.tool ?? "?",
      label: "Zed",
      argv: ["/usr/local/bin/zed", "/w/infra"],
      pid: 42,
    });
    await mount();
    await act(async () =>
      rows()[2]!.dispatchEvent(
        new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 5, clientY: 5 }),
      ),
    );
    await settle();
    const item = [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].find(
      (i) => i.textContent === "다음에서 열기…",
    );
    await act(async () => item!.click());
    await settle();
    expect(popupText()).toContain("다음에서 열기: infra");
    // 메뉴가 닫히며 돌려놓은 포커스가 팝업의 입력 칸을 덮지 않는다.
    expect(document.activeElement?.getAttribute("placeholder")).toBe("도구 이름");
    expect(calls("openWith.list").map((c) => c.params)).toContainEqual({
      projectId: "c",
      worktree: null,
    });
    expect(popupItems().map((i) => i.textContent)).toEqual([
      "Zed",
      "IntelliJ IDEA쓸 수 없음: `command` is empty",
      "Finder기본",
    ]);
    expect(popupItems()[1]!.getAttribute("data-disabled")).toBe("true");
    // 팝업이 열린 동안 창 단축키(⌘⌥E · ⌘N)는 돌지 않는다.
    expect(await press("KeyE", { metaKey: true, altKey: true })).toBe(false);
    expect(await press("KeyN", { metaKey: true })).toBe(false);
    expect(calls("openWith.run")).toEqual([]);
    // 한글 조합을 확정하는 Enter 는 고르지 않는다.
    await press("Enter", { isComposing: true, keyCode: 229 }, "Enter");
    await settle();
    expect(calls("openWith.run")).toEqual([]);
    await press("Enter");
    await settle();
    expect(document.querySelector("[cmdk-dialog]")).toBeNull();
    expect(calls("openWith.run").map((c) => c.params)).toEqual([
      { projectId: "c", tool: "zed", worktree: null },
    ]);
  });

  it("opens the selected project with the default tool and says why it failed", async () => {
    host.openWithRun = () => ({
      kind: "failed",
      tool: "zed",
      label: "Zed",
      argv: ["zed", "/w/billing-api"],
      problem: { kind: "notFound", program: "zed", loginShellPath: true },
    });
    await mount();
    expect(await press("KeyE", { metaKey: true, altKey: true }, "´")).toBe(true);
    await settle();
    expect(calls("openWith.run").map((c) => c.params)).toEqual([
      { projectId: "b", tool: null, worktree: null },
    ]);
    expect(alertText()).toBe(
      "Zed 로 열지 못했다: 「zed」 를 로그인 셸 PATH 에서 찾지 못했다 — 명령: zed /w/billing-api",
    );
  });

  it("asks for a tool when there is no default", async () => {
    host.openWithRun = () => ({ kind: "noDefault", configured: "explorer" });
    await mount();
    await press("KeyE", { metaKey: true, altKey: true });
    await settle();
    expect(popupText()).toContain("기본 도구 「explorer」 는 이 시스템에서 쓸 수 없다");
    await press("Escape");
    await settle();
    expect(document.querySelector("[cmdk-dialog]")).toBeNull();
    // ⌥F1 은 고른 프로젝트로 팝업을 연다.
    await press("F1", { altKey: true });
    await settle();
    expect(popupText()).toContain("다음에서 열기: billing-api");
  });
});
