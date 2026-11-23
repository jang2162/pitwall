// @vitest-environment happy-dom
// 끊긴 머신의 탭과 다시 붙을 때 맞추기(18-m7-plan M7-13 완료 기준 — 웹뷰 시험, StrictMode 는 test-support/strict-mode.ts):
// ① 끊김 동안 문서가 남고 편집이 막힌다 ② 다시 붙어 `version` 이 다르면 미저장 문서에 배너, 미저장이 없으면 새 내용 ③ 끊긴 동안의
// 저장 시도는 막힌다 — 그리고 ④ 정말 끊겼다 다시 붙으면 그 머신의 감시(`fs.watch`)를 새 연결로 다시 연다.
//
// 가짜 Host 는 연결 대상마다 따로이고 **실제 범위 규칙**대로 거절한다(M7-11 리뷰 B1): `remote.*` 는 Host 범위 연결만, 프로젝트 ·
// 멤버 연결은 자기 프로젝트(`p1` · 그 머신의 `r1`)의 `projectId` 만 받는다(`events.subscribe` 포함). 두 머신에 **같은 경로**의
// 파일이 있다 — 끊긴 머신의 저장이 다른 머신으로 새거나, 한 머신의 변경이 다른 머신의 문서에 닿으면 내용이 바뀐다.

import { EditorView } from "@codemirror/view";
import type { DockviewApi } from "dockview-react";
import { act as reactAct } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, vi } from "vitest";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../App";
import { checkoutKey, qualify } from "../checkout/checkout";
import { openTab } from "./editor-tabs";
import { resetWindowFileTree } from "../file-tree/project-view";
import type { KeymapNative } from "../keymap/native";
import { activeWorktreeStore } from "../open-with/active-worktree";
import { projectOpenInbox } from "../project-open";
import { act, endTest, it } from "../test-support/ended-test";
import { createThemeStore } from "../theme/theme-store";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const MINI = "SHA256:mini/fingerprint";
const FILE = "/work/demo/src/main.rs";
const OTHER = "/work/demo/src/other.rs";

interface Conn {
  machine: string;
  /** 연결의 범위가 받는 프로젝트(Host 범위면 없음). */
  project: string | undefined;
  closed: boolean;
  /** 구독 id → 토픽 · 프로젝트. */
  subscriptions: Map<number, { projectId: string; topics: string[] }>;
  emit(subscriptionId: number, payload: unknown): void;
}

const fake = vi.hoisted(() => ({
  /** 머신 → (경로 → 내용 · 시각). */
  files: new Map<string, Map<string, { text: string; mtime: number }>>(),
  calls: new Map<string, { method: string; params: Record<string, unknown> }[]>(),
  /** 연 연결(순서대로). */
  conns: [] as Conn[],
  /** 열린 `remote.connect` 스트림에 상태를 보낸다. */
  links: new Set<(status: unknown) => void>(),
  /** 머신 → 열린 `fs.watch` 스트림(연결 · 워크트리). */
  watches: [] as { conn: Conn; projectId: unknown; worktree: unknown; open: boolean }[],
  clock: 10,
  /** 머신 → (경로 → 보고할 크기). 없으면 내용의 길이(큰 파일을 내용 없이 흉내 낸다). */
  sizes: new Map<string, Map<string, number>>(),
}));

vi.mock("../tauri-transport", async () => {
  const { createMemoryTransportPair: pair } = await import("@pitwall/rpc-client");
  const { DEFAULT_KEYMAP_TOML } = await import("../keymap/default-keymap");
  const { i18nState } = await import("../i18n/testing");
  const MINI_ID = "SHA256:mini/fingerprint";
  return {
    connectHost: async (target: Record<string, unknown>) => {
      const member =
        "member" in target ? (target.member as { host: string; project: string }) : undefined;
      const machine = member?.host ?? "local";
      const hostScope = "hostId" in target;
      const project = member?.project ?? (target.projectId as string | undefined);
      const [client, server] = pair();
      let next = 1;
      const streams = new Map<number, () => void>();
      const send = (m: object) => {
        if (!conn.closed) server.send(JSON.stringify({ jsonrpc: "2.0", ...m }));
      };
      const conn: Conn = {
        machine,
        project,
        closed: false,
        subscriptions: new Map(),
        emit: (subscriptionId, payload) =>
          send({
            method: "events.event",
            params: { subscriptionId, projectId: project, topic: "fs.changed", payload },
          }),
      };
      fake.conns.push(conn);
      server.onMessage((text) => {
        const m = JSON.parse(text) as { id?: number; method: string; params: never };
        const reply = (result: unknown) => send({ id: m.id, result });
        const deny = (message: string) =>
          send({ id: m.id, error: { code: -32001, message, data: null } });
        const params = (m.params ?? {}) as Record<string, unknown>;
        const files = fake.files.get(machine)!;
        if (m.method.startsWith("fs.")) fake.calls.get(machine)!.push({ method: m.method, params });
        // 범위 규칙(pw-remote `host_scope`, 프로젝트 범위 연결의 `projectId` 검사).
        if (m.method.startsWith("remote.") && !hostScope)
          return deny("remote.* takes only a host connection");
        if ("projectId" in params && !hostScope && params.projectId !== project)
          return deny(`this connection is scoped to ${project}`);
        const path = String(params.path ?? "");
        const version = (p: string) => `${files.get(p)!.mtime}:${files.get(p)!.text}`;
        const size = (p: string) => fake.sizes.get(machine)?.get(p) ?? files.get(p)!.text.length;
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
            const push = (status: unknown) =>
              send({ method: "stream.chunk", params: { streamId, data: status } });
            fake.links.add(push);
            streams.set(streamId, () => fake.links.delete(push));
            reply({ streamId });
            return push({ state: "connected", machine: MINI_ID });
          }
          case "fs.watch": {
            const streamId = next++;
            const watch = {
              conn,
              projectId: params.projectId,
              worktree: params.worktree,
              open: true,
            };
            fake.watches.push(watch);
            streams.set(streamId, () => (watch.open = false));
            reply({ streamId });
            return send({
              method: "stream.chunk",
              params: { streamId, data: { roots: [{ path: "/work/demo", mode: "native" }] } },
            });
          }
          case "stream.cancel": {
            const { streamId } = m.params as { streamId: number };
            streams.get(streamId)?.();
            streams.delete(streamId);
            return;
          }
          case "fs.list":
            return reply({ path, entries: [] });
          case "fs.stat":
            if (!files.has(path)) break;
            return reply({
              path,
              kind: "file",
              target: null,
              size: size(path),
              mtimeMs: files.get(path)!.mtime,
              readonly: false,
              sizeClass: "editable",
            });
          case "fs.read":
            if (!files.has(path)) break;
            return reply({
              path,
              size: size(path),
              mtimeMs: files.get(path)!.mtime,
              offset: 0,
              eof: true,
              version: params.length === undefined ? version(path) : null,
              content: params.bytes
                ? { kind: "base64", data: btoa(files.get(path)!.text) }
                : { kind: "text", text: files.get(path)!.text },
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
            files.set(path, { text: content.text, mtime: ++fake.clock });
            return reply({
              path,
              size: content.text.length,
              mtimeMs: fake.clock,
              version: version(path),
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
          case "events.subscribe": {
            const id = next++;
            conn.subscriptions.set(id, {
              projectId: params.projectId as string,
              topics: params.topics as string[],
            });
            return reply({ subscriptionId: id });
          }
          case "events.unsubscribe":
            conn.subscriptions.delete((params as { subscriptionId: number }).subscriptionId);
            return reply(null);
        }
        if (m.id !== undefined)
          send({ id: m.id, error: { code: -32601, message: `no ${m.method} on ${machine}` } });
      });
      return {
        transport: client,
        close: async () => {
          conn.closed = true;
          for (const end of streams.values()) end();
          client.close();
        },
      };
    },
  };
});

let menuListener: ((action: string) => void) | undefined;
let closeGuard: (() => Promise<boolean>) | undefined;
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
  guardClose: (guard: () => Promise<boolean>) => {
    closeGuard = guard;
    return () => {
      if (closeGuard === guard) closeGuard = undefined;
    };
  },
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
    ["local", new Map([[FILE, { text: "local text", mtime: 1 }]])],
    [
      MINI,
      new Map([
        [FILE, { text: "remote text", mtime: 1 }],
        [OTHER, { text: "remote other", mtime: 1 }],
      ]),
    ],
  ]);
  fake.calls = new Map([
    ["local", []],
    [MINI, []],
  ]);
  fake.conns = [];
  fake.links = new Set();
  fake.watches = [];
  fake.sizes = new Map([
    ["local", new Map()],
    [MINI, new Map()],
  ]);
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

const REMOTE_MAIN = checkoutKey({ machine: MINI });

const views = () =>
  [...document.querySelectorAll<HTMLElement>(".cm-editor")].map((e) => EditorView.findFromDOM(e)!);
const viewWith = (text: string) => views().find((v) => v.state.doc.toString().includes(text));
const writes = (machine: string) =>
  fake.calls
    .get(machine)!
    .filter((c) => c.method === "fs.write")
    .map((c) => c.params);
const reads = (machine: string) =>
  fake.calls
    .get(machine)!
    .filter((c) => c.method === "fs.read")
    .map((c) => c.params.path);
const offlineNotice = () => document.querySelector<HTMLElement>("[data-offline-notice]");
const banner = () => document.querySelector<HTMLElement>("[data-testid=disk-banner]");
const dialog = () => document.querySelector<HTMLElement>("[data-testid=unsaved-dialog]");
const memberConns = () => fake.conns.filter((c) => c.machine === MINI);
const liveMember = () => memberConns().find((c) => !c.closed);

/** 링크 상태를 그 머신을 쓰는 모든 `remote.connect` 스트림에 보낸다. */
async function link(status: object) {
  await act(async () => {
    for (const push of fake.links) push(status);
  });
  await settle();
}
const RECONNECTING = {
  state: "reconnecting",
  attempt: 1,
  retryInMs: 200,
  error: { kind: "io", message: "link lost" },
};

async function push(open: Parameters<typeof projectOpenInbox.push>[0]) {
  await act(async () => projectOpenInbox.push(open));
  await settle();
}

async function type(view: EditorView, text: string) {
  await act(async () =>
    view.dispatch({ changes: { from: 0, insert: text }, userEvent: "input.type" }),
  );
  await settle();
}

async function activate(title: string, machine: boolean) {
  const panel = dock!.panels.find(
    (p) =>
      p.title === title &&
      (machine
        ? String(p.params?.path).startsWith("pitwall-remote://")
        : !String(p.params?.path).startsWith("pitwall-remote://")),
  );
  await act(async () => panel?.api.setActive());
  await settle();
}

async function saveActive() {
  await act(async () => menuListener?.("editor.save"));
  await settle();
}

/** 그 머신의 `fs.changed` 를 그 연결의 구독마다 보낸다(감시가 낸 것처럼 — 그 머신의 경로). */
async function changed(conn: Conn, paths: string[]) {
  await act(async () => {
    for (const [id, sub] of conn.subscriptions)
      if (sub.topics.includes("fs.changed"))
        conn.emit(id, {
          root: "/work/demo",
          worktree: null,
          paths,
          kinds: paths.map(() => "modified"),
          rescan: false,
        });
  });
  await settle();
}

/** 이 머신의 탭과 다른 머신의 같은 경로 탭을 연다. 활성 체크아웃은 이 머신. */
async function openBoth() {
  await mount();
  await push({ file: FILE });
  await act(async () => void openTab(dock!, { path: qualify(MINI, FILE), worktree: REMOTE_MAIN }));
  const found = await vi.waitFor(() => {
    const view = viewWith("remote text");
    expect(view).toBeDefined();
    return view!;
  });
  expect(found).toBeDefined();
  expect((await viewOf("main.rs", false)).state.doc.toString()).toBe("local text");
  return viewOf("main.rs", true);
}

/** 그 탭을 앞으로 하고 그 뷰(한 그룹에는 앞 탭의 뷰만 그려진다). */
async function viewOf(title: string, machine: boolean): Promise<EditorView> {
  await activate(title, machine);
  return vi.waitFor(() => {
    const [view] = views();
    expect(view).toBeDefined();
    return view!;
  });
}

it("keeps a dropped machine's tab with its unsaved edits read-only, and saves it nowhere until it reconnects (① ③)", async () => {
  let remote = await openBoth();
  await type(remote, "mine ");
  expect(offlineNotice()).toBeNull();

  await link(RECONNECTING);
  // ① 탭 · 문서가 남는다 — 미저장 편집 그대로, 편집은 막힌다(읽기 전용), 안내에 머신 이름.
  remote = viewWith("remote text")!;
  expect(remote.state.doc.toString()).toBe("mine remote text");
  expect(remote.state.readOnly).toBe(true);
  expect(offlineNotice()?.textContent).toContain("mini");
  // 이 머신의 같은 경로 탭은 끊기지 않았다.
  const local = await viewOf("main.rs", false);
  expect(local.state.doc.toString()).toBe("local text");
  expect(local.state.readOnly).toBe(false);
  expect(offlineNotice()).toBeNull();

  // ③ 끊긴 동안의 저장은 막힌다 — 그 머신에도, 이 머신(창 · 활성 체크아웃의 연결)에도 쓰지 않는다.
  await viewOf("main.rs", true);
  await saveActive();
  expect(writes(MINI)).toHaveLength(0);
  expect(writes("local")).toHaveLength(0);
  expect(fake.files.get("local")!.get(FILE)!.text).toBe("local text");
  expect(document.body.textContent).toContain("이 파일의 머신과 연결이 끊겨 쓰지 않았다");

  // 다시 붙으면 편집 · 저장이 되고, 저장은 그 머신으로만 간다.
  await link({ state: "connected", machine: MINI });
  await vi.waitFor(() => expect(offlineNotice()).toBeNull());
  remote = viewWith("remote text")!;
  expect(remote.state.readOnly).toBe(false);
  await saveActive();
  expect(writes(MINI)).toHaveLength(1);
  expect(writes(MINI)[0]).toMatchObject({ projectId: "r1", path: FILE });
  expect(writes("local")).toHaveLength(0);
  expect(fake.files.get(MINI)!.get(FILE)!.text).toBe("mine remote text");
  expect(fake.files.get("local")!.get(FILE)!.text).toBe("local text");
});

it("asks as now when a dropped machine's unsaved tab closes, and Save keeps it open (③)", async () => {
  const remote = await openBoth();
  await type(remote, "mine ");
  await link(RECONNECTING);

  const header = [...document.querySelectorAll<HTMLElement>("[data-editor-tab-header]")].find(
    (h) => h.title === FILE && h.querySelector("[data-machine]"),
  )!;
  await act(async () => header.querySelector<HTMLElement>(".dv-default-tab-action")!.click());
  await settle();
  expect(dialog()).not.toBeNull();
  const button = (text: string) =>
    [...dialog()!.querySelectorAll<HTMLButtonElement>("button")].find(
      (b) => b.textContent === text,
    )!;
  await act(async () => button("저장").click());
  await settle();
  // 저장할 곳이 없다 — 닫지 않고 편집이 남는다.
  expect(writes(MINI)).toHaveLength(0);
  expect(writes("local")).toHaveLength(0);
  expect(viewWith("mine remote text")).toBeDefined();

  // 창을 닫을 때도 같은 확인이 뜬다.
  let answer: Promise<boolean> | undefined;
  await act(async () => {
    answer = closeGuard!();
  });
  await settle();
  expect(dialog()).not.toBeNull();
  await act(async () =>
    [...dialog()!.querySelectorAll<HTMLButtonElement>("button")]
      .find((b) => b.textContent === "취소")!
      .click(),
  );
  expect(await answer).toBe(false);
});

it("on reconnecting raises the banner over unsaved edits and takes new content otherwise (②)", async () => {
  await openBoth();
  await act(async () => void openTab(dock!, { path: qualify(MINI, OTHER), worktree: REMOTE_MAIN }));
  await vi.waitFor(() => expect(viewWith("remote other")).toBeDefined());
  await type(await viewOf("main.rs", true), "mine ");

  await link(RECONNECTING);
  // 끊긴 동안 그 머신에서 두 파일이 바뀌었다(감시 이벤트는 오지 않았다).
  fake.files.get(MINI)!.set(FILE, { text: "theirs", mtime: ++fake.clock });
  fake.files.get(MINI)!.set(OTHER, { text: "new other", mtime: ++fake.clock });
  // 이 머신의 같은 경로도 바뀌었다 — 그 머신의 맞추기가 이 머신 파일을 읽지 않는다.
  fake.calls.get("local")!.length = 0;

  await link({ state: "connected", machine: MINI });
  // 미저장이 있는 문서는 넣지 않고 배너, 없는 문서는 새 내용.
  await vi.waitFor(() => expect(banner()?.dataset.disk).toBe("changed"));
  expect(viewWith("mine remote text")).toBeDefined();
  expect(offlineNotice()).toBeNull();
  await vi.waitFor(async () =>
    expect((await viewOf("other.rs", true)).state.doc.toString()).toBe("new other"),
  );
  expect(reads("local")).toHaveLength(0);
  expect((await viewOf("main.rs", false)).state.doc.toString()).toBe("local text");
});

it("reopens the machine's file watch on the new connection after a real drop, and its changes reach only its documents (④)", async () => {
  await openBoth();
  const first = liveMember()!;
  await vi.waitFor(() =>
    expect(fake.watches.filter((w) => w.open && w.conn === first)).toHaveLength(1),
  );
  expect(fake.watches.find((w) => w.conn === first)).toMatchObject({
    projectId: "r1",
    worktree: null,
  });
  // 이 머신의 감시는 이 머신 프로젝트로(활성 체크아웃).
  expect(
    fake.watches.some((w) => w.open && w.conn.machine === "local" && w.projectId === "p1"),
  ).toBe(true);

  // 감시가 낸 그 머신의 변경은 그 머신의 문서에만 — 이 머신의 같은 경로 문서는 다시 읽지 않는다.
  fake.files.get(MINI)!.set(FILE, { text: "watched", mtime: ++fake.clock });
  fake.calls.get("local")!.length = 0;
  await changed(first, [FILE]);
  await vi.waitFor(() => expect(viewWith("watched")).toBeDefined());
  expect(reads("local")).toHaveLength(0);
  expect((await viewOf("main.rs", false)).state.doc.toString()).toBe("local text");
  await viewOf("main.rs", true);

  // 정말 끊김 → 멤버 연결이 닫히고(옛 연결의 감시도), 다시 붙으면 새 연결에 감시를 다시 연다.
  await link({ state: "disconnected", byUser: false, error: null });
  await vi.waitFor(() => expect(first.closed).toBe(true));
  expect(offlineNotice()).not.toBeNull();
  await link({ state: "connected", machine: MINI });
  const second = await vi.waitFor(() => {
    const now = liveMember();
    expect(now).toBeDefined();
    expect(now).not.toBe(first);
    return now!;
  });
  await vi.waitFor(() =>
    expect(fake.watches.filter((w) => w.open && w.conn === second)).toHaveLength(1),
  );
  await vi.waitFor(() => expect(offlineNotice()).toBeNull());
  // 이어지는 감시: 새 연결의 변경이 문서에 닿는다.
  fake.files.get(MINI)!.set(FILE, { text: "after", mtime: ++fake.clock });
  await changed(second, [FILE]);
  await vi.waitFor(() => expect(viewWith("after")).toBeDefined());
});

it("saves a reconnected tab only to its own machine's new connection after the active checkout moved", async () => {
  const remote = await openBoth();
  await type(remote, "mine ");
  const first = liveMember()!;
  await link({ state: "disconnected", byUser: false, error: null });
  await link({ state: "connected", machine: MINI });
  await vi.waitFor(() => expect(offlineNotice()).toBeNull());
  // 활성 체크아웃을 이 머신의 다른 워크트리로 — 이 머신의 탭은 닫히고 다른 머신의 탭은 남는다.
  fake.files.get("local")!.set("/work/wt1/x.rs", { text: "wt", mtime: 1 });
  await push({ worktree: "/work/wt1" });
  await viewOf("main.rs", true);
  expect(viewWith("mine remote text")).toBeDefined();
  await saveActive();
  // 옛(닫힌) 연결이 아니라 새 멤버 연결로 간다.
  expect(first.closed).toBe(true);
  expect(writes("local")).toHaveLength(0);
  expect(writes(MINI)).toHaveLength(1);
  expect(writes(MINI)[0]).toMatchObject({ projectId: "r1", path: FILE });
  expect(fake.files.get("local")!.get(FILE)!.text).toBe("local text");
});

// 링크 틀 상한(메인 판정 — editor/link-limits.ts): 틀을 넘는 메시지는 그 머신의 채널을 닫는다. 다른 머신 파일은 통째로 읽기 ·
// 쓰기 전에 크기를 보고 막는다 — 이 머신 파일은 그대로 연다.
const BIG = 30 * 1024 * 1024;
const wholeReads = (machine: string, path: string) =>
  fake.calls
    .get(machine)!
    .filter(
      (c) => c.method === "fs.read" && c.params.path === path && c.params.length === undefined,
    );

it("does not read a remote file too large for the link at once, and opens the same size from this machine", async () => {
  await mount();
  fake.files.get(MINI)!.set("/work/demo/big.log", { text: "big remote", mtime: 1 });
  fake.sizes.get(MINI)!.set("/work/demo/big.log", BIG);
  fake.files.get("local")!.set("/work/demo/big.log", { text: "big local", mtime: 1 });
  fake.sizes.get("local")!.set("/work/demo/big.log", BIG);
  await act(
    async () =>
      void openTab(dock!, { path: qualify(MINI, "/work/demo/big.log"), worktree: REMOTE_MAIN }),
  );
  await vi.waitFor(() => expect(document.body.textContent).toContain("링크로는 열지 않는다"));
  // 열지 않은 큰 파일의 안내다(읽기 실패가 아니다) — [앞부분 보기] · [hex] 는 범위 읽기라 링크 틀 안이다.
  expect(document.querySelector("[data-notice-action=head]")).not.toBeNull();
  expect(wholeReads(MINI, "/work/demo/big.log")).toHaveLength(0);
  expect(viewWith("big remote")).toBeUndefined();

  await push({ file: "/work/demo/big.log" });
  await vi.waitFor(() => expect(viewWith("big local")).toBeDefined());
});

it("does not read a remote document again once it grew past the link limit", async () => {
  await openBoth();
  const member = liveMember()!;
  await vi.waitFor(() => expect(member.subscriptions.size).toBeGreaterThan(0));
  fake.files.get(MINI)!.set(FILE, { text: "grown", mtime: ++fake.clock });
  fake.sizes.get(MINI)!.set(FILE, BIG);
  const before = wholeReads(MINI, FILE).length;
  await changed(member, [FILE]);
  await settle();
  expect(wholeReads(MINI, FILE)).toHaveLength(before);
  expect(viewWith("remote text")).toBeDefined();
});

it("refuses to send a remote save too large for the link and keeps the edit", async () => {
  const remote = await openBoth();
  // 따옴표는 JSON 에서 두 바이트다 — 25 MiB 의 따옴표는 보낼 때 50 MiB 로, 쓰기 상한(48 MiB)을 넘는다.
  await type(remote, '"'.repeat(25 * 1024 * 1024));
  await saveActive();
  expect(writes(MINI)).toHaveLength(0);
  expect(writes("local")).toHaveLength(0);
  expect(document.body.textContent).toContain("보내지 않았다");
  expect(member().closed).toBe(false);
}, 60_000);

it("reads a remote tab's image on that machine and none on this one", async () => {
  await mount();
  fake.files.get(MINI)!.set("/work/demo/logo.png", { text: "PNGDATA", mtime: 1 });
  await act(
    async () =>
      void openTab(dock!, { path: qualify(MINI, "/work/demo/logo.png"), worktree: REMOTE_MAIN }),
  );
  await vi.waitFor(() =>
    expect(
      fake.calls.get(MINI)!.some((c) => c.method === "fs.read" && c.params.bytes === true),
    ).toBe(true),
  );
  expect(
    fake.calls.get(MINI)!.find((c) => c.method === "fs.read" && c.params.bytes === true)!.params,
  ).toMatchObject({ projectId: "r1", path: "/work/demo/logo.png" });
  const localFs = fake.calls.get("local")!.filter((c) => c.params.path === "/work/demo/logo.png");
  expect(localFs).toHaveLength(0);
});

const member = () => liveMember()!;
