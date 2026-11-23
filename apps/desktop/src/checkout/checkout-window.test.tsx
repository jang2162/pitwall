// @vitest-environment happy-dom
// 묶음 창의 체크아웃별 연결(18-m7-plan M7-11 완료 기준 — 웹뷰 시험, StrictMode 는 test-support/strict-mode.ts):
// ① 활성 체크아웃을 바꿔도 다른 머신의 탭은 남고 그 머신 연결로 읽고 쓴다 ② 원격 탭의 저장은 그 머신 연결로 간다(활성 체크아웃 ·
// 창의 연결로 가지 않는다) ④ 머신 표지가 글로 있다(타이틀바 · 상태바 · 탭 머리). 가짜 Host 는 연결 대상마다 따로다 — 이 머신
// (`?project=p1`)과 다른 머신 `MINI` 의 멤버(`r1`). 두 머신에 **같은 경로**의 파일이 있다 — 섞이면 내용 · 저장이 바뀐다.

import { EditorView } from "@codemirror/view";
import type { DockviewApi } from "dockview-react";
import { act as reactAct } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, vi } from "vitest";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../App";
import { openTab } from "../editor/editor-tabs";
import { resetWindowFileTree } from "../file-tree/project-view";
import type { KeymapNative } from "../keymap/native";
import { activeWorktree, activeWorktreeStore } from "../open-with/active-worktree";
import { projectOpenInbox } from "../project-open";
import { act, endTest, it } from "../test-support/ended-test";
import { createThemeStore } from "../theme/theme-store";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";
import { checkoutKey, qualify } from "./checkout";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const MINI = "SHA256:mini/fingerprint";
const FILE = "/work/demo/src/main.rs";

const fake = vi.hoisted(() => ({
  /** 머신 → (경로 → 내용). */
  files: new Map<string, Map<string, string>>(),
  /** 머신 → 받은 `fs.*` 호출. */
  calls: new Map<string, { method: string; params: Record<string, unknown> }[]>(),
  /** 연 연결의 대상. */
  targets: [] as unknown[],
  /** 열린 `remote.connect` 스트림 수. */
  linkStreams: 0,
  /** 열린 `mcp.approvals` 스트림: 스트림 id → (머신, 들은 프로젝트). */
  approvals: new Map<number, { machine: string; projectId: unknown }>(),
}));

vi.mock("../tauri-transport", async () => {
  const { createMemoryTransportPair: pair } = await import("@pitwall/rpc-client");
  const { DEFAULT_KEYMAP_TOML } = await import("../keymap/default-keymap");
  const { i18nState } = await import("../i18n/testing");
  const MINI_ID = "SHA256:mini/fingerprint";
  return {
    connectHost: async (target: Record<string, unknown>) => {
      fake.targets.push(target);
      const machine = "member" in target ? (target.member as { host: string }).host : "local";
      // Host 의 범위 규칙: `remote.*` 는 Host 범위 연결만 받는다(pw-remote `host_scope`) — 프로젝트 · 멤버 대상 연결은 거절.
      const hostScope = "hostId" in target;
      const [client, server] = pair();
      let next = 1;
      const streams = new Set<number>();
      const send = (m: object) => server.send(JSON.stringify({ jsonrpc: "2.0", ...m }));
      server.onMessage((text) => {
        const m = JSON.parse(text) as { id?: number; method: string; params: never };
        const reply = (result: unknown) => send({ id: m.id, result });
        const params = (m.params ?? {}) as Record<string, unknown>;
        const files = fake.files.get(machine)!;
        if (m.method.startsWith("fs.")) fake.calls.get(machine)!.push({ method: m.method, params });
        if (m.method.startsWith("remote.") && !hostScope)
          return send({
            id: m.id,
            error: { code: -32001, message: "remote.* takes only a host connection", data: null },
          });
        const path = String(params.path ?? "");
        const version = (p: string) => `v:${files.get(p)}`;
        switch (m.method) {
          case "i18n.get":
            return reply(i18nState("ko"));
          case "config.get":
            return reply({ snapshot: { files: [], notices: [] }, error: null });
          case "keymap.get":
            return reply({
              id: "pitwall-default",
              extension: "dev.pitwall.keymap-default",
              kind: "base",
              content: DEFAULT_KEYMAP_TOML,
            });
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
            });
          case "project.resolve":
            return reply(null);
          case "project.trust":
            return reply({ projectId: "p1", ownerId: "p1", state: "trusted" });
          case "project.state.get":
            return reply({ state: null, problem: null });
          case "project.state.set":
            return reply(null);
          case "bundle.list":
            if (machine !== "local") break;
            return reply([
              {
                id: "b-1",
                name: "demo",
                favorite: false,
                tags: [],
                openedAt: null,
                implicit: false,
                members: [
                  { machine: "local", projectId: "p1", lastTarget: null },
                  { machine: MINI_ID, projectId: "r1", lastTarget: null },
                ],
              },
            ]);
          case "remote.hosts.list":
            if (machine !== "local") break;
            return reply({
              hosts: [
                {
                  name: "mini",
                  host: "mini.lan",
                  user: null,
                  port: null,
                  source: "sshConfig",
                  machine: MINI_ID,
                  status: { state: "disconnected", byUser: false, error: null },
                  lastLink: null,
                },
              ],
              warnings: [],
            });
          case "remote.connect": {
            if (machine !== "local") break;
            const streamId = next++;
            streams.add(streamId);
            fake.linkStreams++;
            reply({ streamId });
            return send({
              method: "stream.chunk",
              params: { streamId, data: { state: "connected", machine: MINI_ID } },
            });
          }
          case "mcp.approvals": {
            // 실제 Host 처럼 그 창 연결의 프로젝트만 듣게 한다(`scope_may_write`) — 멤버 연결은 그 머신의 프로젝트 id 다.
            const project = "member" in target ? "r1" : "p1";
            if (params.projectId !== project)
              return send({
                id: m.id,
                error: { code: -32001, message: "not this project's window", data: null },
              });
            const streamId = next++;
            fake.approvals.set(streamId, { machine, projectId: params.projectId });
            return reply({ streamId });
          }
          case "stream.cancel": {
            const { streamId } = m.params as { streamId: number };
            if (streams.delete(streamId)) fake.linkStreams--;
            fake.approvals.delete(streamId);
            return;
          }
          case "fs.list": {
            const prefix = path.endsWith("/") ? path : `${path}/`;
            const names = new Map<string, "file" | "dir">();
            for (const f of files.keys()) {
              if (!f.startsWith(prefix)) continue;
              const rest = f.slice(prefix.length).split("/");
              names.set(rest[0]!, rest.length > 1 ? "dir" : "file");
            }
            if (names.size === 0) break;
            return reply({
              path,
              entries: [...names].map(([name, kind]) => ({
                name,
                kind,
                target: null,
                outside: false,
                size: 0,
                mtimeMs: 1,
                ignored: false,
              })),
            });
          }
          case "fs.stat":
            if (!files.has(path)) break;
            return reply({
              path,
              kind: "file",
              target: null,
              size: files.get(path)!.length,
              mtimeMs: 1,
              readonly: false,
              sizeClass: "editable",
            });
          case "fs.read":
            if (!files.has(path)) break;
            return reply({
              path,
              size: files.get(path)!.length,
              mtimeMs: 1,
              offset: 0,
              eof: true,
              version: version(path),
              content: { kind: "text", text: files.get(path)! },
              judgment: {
                binary: false,
                encoding: "UTF-8",
                lossy: false,
                bom: null,
                lineEnding: "lf",
                longLines: false,
                sizeClass: "editable",
                editable: true,
                highlight: false,
              },
            });
          case "fs.write": {
            const content = params.content as { text: string };
            if (params.expectedVersion !== version(path))
              return send({ id: m.id, error: { code: -32004, message: "conflict", data: null } });
            files.set(path, content.text);
            return reply({ path, size: content.text.length, mtimeMs: 2, version: version(path) });
          }
          // 조각 읽기(crates/pw-fs/src/chunks.rs 의 규칙 — 다른 머신 문서의 통째 읽기는 이것으로 온다, M7-13b).
          case "fs.readChunk": {
            if (!files.has(path)) break;
            const max = Number(params.maxBytes);
            if (!(max >= 16 && max <= 16 * 1024 * 1024))
              return send({ id: m.id, error: { code: -32602, message: "maxBytes", data: null } });
            if (params.expectedVersion != null && params.expectedVersion !== version(path))
              return send({
                id: m.id,
                error: { code: -32004, message: "conflict", data: { current: version(path) } },
              });
            const whole = files.get(path)!;
            const offset = Number(params.offset ?? 0);
            let end = whole.length;
            while (
              end > offset &&
              new TextEncoder().encode(JSON.stringify(whole.slice(offset, end))).length > max
            )
              end = offset + Math.floor((end - offset) / 2);
            return reply({
              path,
              size: whole.length,
              mtimeMs: 1,
              version: version(path),
              judgment: {
                binary: false,
                encoding: "UTF-8",
                lossy: false,
                bom: null,
                lineEnding: "lf",
                longLines: false,
                sizeClass: "editable",
                editable: true,
                highlight: false,
              },
              offset,
              next: end,
              total: whole.length,
              eof: end === whole.length,
              content: { kind: "text", text: whole.slice(offset, end) },
            });
          }
          case "lang.detect":
            return reply({
              files: (params.files as unknown[]).map(() => ({
                language: null,
                source: "extension",
                vendored: false,
                generated: false,
                documentation: false,
              })),
            });
          case "events.subscribe":
            return reply({ subscriptionId: next++ });
          case "events.unsubscribe":
            return reply(null);
        }
        if (m.id !== undefined)
          send({ id: m.id, error: { code: -32601, message: `no ${m.method} on ${machine}` } });
      });
      return { transport: client, close: async () => {} };
    },
  };
});

let menuListener: ((action: string) => void) | undefined;
const native = {
  syncMenu: async () => {},
  onMenuAction: (listener: (action: string) => void) => {
    menuListener = listener;
    return () => {
      if (menuListener === listener) menuListener = undefined;
    };
  },
  openSettings: async () => {},
  closeWindow: async () => {},
} satisfies KeymapNative;

let root: Root | undefined;
let dock: DockviewApi | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
  );
  vi.spyOn(console, "info").mockImplementation(() => {});
  vi.spyOn(console, "warn").mockImplementation(() => {});
  window.history.replaceState(null, "", "/?window=project&project=p1");
  fake.files = new Map([
    ["local", new Map([[FILE, "local text"]])],
    [MINI, new Map([[FILE, "remote text"]])],
  ]);
  fake.calls = new Map([
    ["local", []],
    [MINI, []],
  ]);
  fake.targets = [];
  fake.linkStreams = 0;
  fake.approvals = new Map();
  activeWorktreeStore.setState({ top: undefined });
  resetWindowFileTree();
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

const settle = () => act(async () => new Promise((r) => setTimeout(r, 30)));

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
}

const REMOTE_FILE = qualify(MINI, FILE);
const REMOTE_MAIN = checkoutKey({ machine: MINI });

const views = () =>
  [...document.querySelectorAll<HTMLElement>(".cm-editor")].map((e) => EditorView.findFromDOM(e)!);
const viewWith = (text: string) => views().find((v) => v.state.doc.toString().includes(text));
const headers = () => [...document.querySelectorAll<HTMLElement>("[data-editor-tab-header]")];
const remoteHeader = () =>
  headers().find((h) => h.title === FILE && h.querySelector(`[data-machine]`));
const writes = (machine: string) =>
  fake.calls
    .get(machine)!
    .filter((c) => c.method === "fs.write")
    .map((c) => c.params);

async function push(open: Parameters<typeof projectOpenInbox.push>[0]) {
  await act(async () => projectOpenInbox.push(open));
  await settle();
}

async function save(view: EditorView, text: string) {
  await act(async () =>
    view.dispatch({ changes: { from: 0, insert: text }, userEvent: "input.type" }),
  );
  // 메뉴의 저장(⌘S 와 같은 액션) — 활성 탭을 저장한다. 키 입력은 포커스를 옮겨 트리에서 온 시험과 다투지 않게 피한다.
  await act(async () => menuListener?.("editor.save"));
  await settle();
}

it("keeps another machine's tab on its own connection while the active checkout changes, and saves it there", async () => {
  await mount();
  await push({ file: FILE });
  // 활성 체크아웃은 이 머신 — 다른 머신의 파일을 그 체크아웃으로 연다(활성 체크아웃을 바꾸지 않는다).
  await act(async () => void openTab(dock!, { path: REMOTE_FILE, worktree: REMOTE_MAIN }));
  await vi.waitFor(() => expect(viewWith("remote text")).toBeDefined());
  const localHeader = () =>
    headers().find((h) => h.title === FILE && !h.querySelector("[data-machine]"));
  expect(localHeader()).toBeDefined();
  expect(fake.targets).toContainEqual({ member: { host: MINI, project: "r1" } });
  expect(fake.calls.get(MINI)!.find((c) => c.method === "fs.readChunk")?.params).toMatchObject({
    projectId: "r1",
    path: FILE,
    worktree: null,
  });
  // ④ 다른 머신의 탭 머리에 그 머신의 이름(별명)이 글로 있다.
  expect(remoteHeader()?.querySelector("[data-machine]")?.textContent).toBe("mini");

  // 활성 체크아웃을 이 머신의 다른 워크트리로 — 이 머신의 탭은 닫히고 다른 머신의 탭은 남는다.
  fake.files.get("local")!.set("/work/wt1/x.rs", "wt");
  await push({ worktree: "/work/wt1" });
  expect(localHeader()).toBeUndefined();
  expect(remoteHeader()).toBeDefined();
  await act(async () => dock!.panels.find((p) => p.title === "main.rs")?.api.setActive());
  await settle();
  const remote = viewWith("remote text");
  expect(remote).toBeDefined();

  // ② 남은 탭의 저장은 그 머신 연결로, 그 머신의 경로 · 프로젝트로 간다.
  await save(remote!, "edited ");
  expect(writes(MINI)).toHaveLength(1);
  expect(writes(MINI)[0]).toMatchObject({ projectId: "r1", path: FILE });
  expect(writes("local")).toHaveLength(0);
  expect(fake.files.get(MINI)!.get(FILE)).toBe("edited remote text");
  expect(fake.files.get("local")!.get(FILE)).toBe("local text");
});

it("shows the active machine as text in the title and status bars and unmarks its own tabs", async () => {
  await mount();
  expect(document.querySelector("header [data-machine]")).toBeNull();
  await push({ worktree: REMOTE_MAIN, file: REMOTE_FILE });
  await vi.waitFor(() => expect(viewWith("remote text")).toBeDefined());
  expect(document.querySelector("header [data-machine]")?.textContent).toBe("mini");
  expect(document.querySelector("[data-testid=status-machine]")?.textContent).toBe("mini");
  // 활성 체크아웃 머신의 탭에는 표지가 없다.
  expect(headers().some((h) => h.querySelector("[data-machine]"))).toBe(false);
  // 그 머신을 쓰는 동안 링크 스트림 하나(StrictMode 의 붙였다 떼기 뒤에도).
  expect(fake.linkStreams).toBe(1);

  await push({ mainCheckout: true });
  expect(document.querySelector("header [data-machine]")).toBeNull();
  expect(document.querySelector("[data-testid=status-machine]")?.textContent).toBe("");
  // 떠나는 체크아웃(그 머신)의 탭은 닫히고, 그 머신을 쓰는 것이 없으니 링크 스트림을 닫는다.
  expect(viewWith("remote text")).toBeUndefined();
  await vi.waitFor(() => expect(fake.linkStreams).toBe(0));
});

// M7-18: Desktop 의 승인 허브는 활성 체크아웃과 무관하게 창의 이 머신 연결 · 이 머신 프로젝트 id 로 듣는다 — 원격이 활성인 동안에도
// 이 Desktop 세션의 승인 요청(다른 머신을 겨눈 것 포함 — 묶음의 로컬 멤버 id 로 온다)을 받는다.
it("keeps hearing this Desktop's approvals on this machine's connection while another machine's checkout is active", async () => {
  await mount();
  const heard = () => [...fake.approvals.values()];
  await vi.waitFor(() => expect(heard()).toEqual([{ machine: "local", projectId: "p1" }]));
  await push({ worktree: REMOTE_MAIN, file: REMOTE_FILE });
  await vi.waitFor(() => expect(viewWith("remote text")).toBeDefined());
  expect(document.querySelector("[data-testid=status-machine]")?.textContent).toBe("mini");
  await settle();
  expect(heard()).toEqual([{ machine: "local", projectId: "p1" }]);
});

it("lists the bundle's other machine in the branch popup and makes its checkout the active one", async () => {
  await mount();
  await act(async () => {
    document.body.dispatchEvent(
      new KeyboardEvent("keydown", {
        bubbles: true,
        cancelable: true,
        code: "KeyB",
        key: "B",
        ctrlKey: true,
        shiftKey: true,
      }),
    );
  });
  await settle();
  const item = await vi.waitFor(() => {
    const found = [...document.querySelectorAll<HTMLElement>("[cmdk-item]")].find((e) =>
      e.querySelector(`[data-machine="${MINI}"]`),
    );
    expect(found?.textContent).toContain("mini");
    return found!;
  });
  await act(async () => item.click());
  await settle();
  expect(activeWorktreeStore.getState().top).toBe(REMOTE_MAIN);
  expect(document.querySelector("header [data-machine]")?.textContent).toBe("mini");
});

// 리뷰 B2: 원격 체크아웃이 활성일 때 그 머신의 파일 트리에서 연 파일은 **그 머신의 파일**이다 — Host 가 돌려준 접두 없는 경로를
// 이 머신의 같은 경로로 읽지 않는다. 저장도 그 머신으로 가고 이 머신의 파일은 그대로다.
it("opens a file from the active remote checkout's file tree on that machine and saves it there", async () => {
  // happy-dom 은 배치를 하지 않는다 — 트리의 가상화가 읽는 보이는 높이만 준다(file-tree/testing.ts 의 일부). 파일을 연 뒤 되돌린다.
  const isTree = (el: HTMLElement) => el.getAttribute("role") === "tree";
  const layout = (["offsetHeight", "clientHeight"] as const).map((prop) =>
    vi.spyOn(HTMLElement.prototype, prop, "get").mockImplementation(function (this: HTMLElement) {
      return isTree(this) ? 600 : 0;
    }),
  );
  await mount();
  await push({ worktree: REMOTE_MAIN });
  const row = (path: string) => document.querySelector<HTMLElement>(`[data-path="${path}"]`);
  const click = async (el: HTMLElement, type: "click" | "dblclick") => {
    await act(async () => {
      el.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true }));
    });
    await settle();
  };
  await vi.waitFor(() => expect(row("/work/demo/src")).not.toBeNull());
  await click(row("/work/demo/src")!, "dblclick");
  await vi.waitFor(() => expect(row(FILE)).not.toBeNull());
  await click(row(FILE)!, "click");
  const view = await vi.waitFor(() => {
    const found = viewWith("remote text");
    expect(found).toBeDefined();
    return found!;
  });
  for (const spy of layout) spy.mockRestore();
  expect(viewWith("local text")).toBeUndefined();
  const localFileCalls = () =>
    fake.calls.get("local")!.filter((c) => ["fs.stat", "fs.read", "fs.write"].includes(c.method));
  expect(localFileCalls()).toHaveLength(0);
  await save(view, "edited ");
  expect(writes(MINI)).toHaveLength(1);
  expect(writes(MINI)[0]).toMatchObject({ projectId: "r1", path: FILE });
  expect(writes("local")).toHaveLength(0);
  expect(fake.files.get("local")!.get(FILE)).toBe("local text");

  // 탭을 바로 여는 자리(`openTab` — DB · git 문서 말고 파일 탭)도 같은 규칙이다(리뷰어의 탐침 P2): 그 머신의 다른 파일.
  fake.files.get(MINI)!.set("/work/demo/src/other.rs", "remote other");
  fake.files.get("local")!.set("/work/demo/src/other.rs", "local other");
  await act(
    async () =>
      void openTab(dock!, { path: "/work/demo/src/other.rs", worktree: activeWorktree() }),
  );
  await vi.waitFor(() => expect(viewWith("remote other")).toBeDefined());
  expect(viewWith("local other")).toBeUndefined();
  expect(localFileCalls()).toHaveLength(0);
});
