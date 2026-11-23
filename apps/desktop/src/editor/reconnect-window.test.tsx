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
  /** 틀 상한을 넘는 메시지로 닫힌 연결. */
  dropped: [] as Conn[],
  /** 오간 메시지 중 가장 큰 것(다른 머신, UTF-8 바이트). */
  largest: 0,
  /** 조각 쓰기 모음(crates/pw-fs/src/chunks.rs 의 규칙 — 연결 · 프로젝트 · 경로에 묶인다). */
  uploads: new Map<
    string,
    { conn: Conn; projectId: unknown; path: string; text: boolean; pieces: string[]; bytes: number }
  >(),
  /** 이 수만큼 `fs.writeChunk` 를 받은 뒤 그 연결의 링크를 끊는다(조각을 보내는 중에 끊김). */
  dropAfterChunks: undefined as number | undefined,
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
      // 링크 틀(pw-link `mux.rs` `send_msg`): 다른 머신의 메시지가 틀 상한을 넘으면 그 채널 — 이 멤버 연결 — 을 닫는다.
      const overFrame = (text: string) => {
        if (machine === "local") return false;
        const bytes = new TextEncoder().encode(text).length;
        fake.largest = Math.max(fake.largest, bytes);
        return bytes > 64 * 1024 * 1024;
      };
      const send = (m: object) => {
        if (conn.closed) return;
        const text = JSON.stringify({ jsonrpc: "2.0", ...m });
        if (overFrame(text)) return void dropChannel();
        server.send(text);
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
        if (overFrame(text)) return void dropChannel();
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
        // 시각 · 길이로 정한다(내용을 이어 붙이면 큰 글의 시험이 응답마다 내용을 한 벌 더 싣는다). 시험의 쓰기는 늘 시각을 바꾼다.
        const version = (p: string) => `${files.get(p)!.mtime}:${files.get(p)!.text.length}`;
        const size = (p: string) => fake.sizes.get(machine)?.get(p) ?? files.get(p)!.text.length;
        const fail = (code: number, message: string, data: unknown = null) =>
          send({ id: m.id, error: { code, message, data } });
        // 크기 등급 · 판정은 실제 Host 처럼 크기로 정한다(crates/pw-fs/src/judge.rs — 5 MiB 까지 편집, 50 MiB 까지 읽기 전용).
        const MIB = 1024 * 1024;
        const sizeClass = (n: number) =>
          n <= 5 * MIB ? "editable" : n <= 50 * MIB ? "readOnly" : "tooLarge";
        const judgment = (n: number) => ({
          binary: false,
          encoding: "UTF-8",
          lossy: false,
          bom: null,
          lineEnding: "lf",
          longLines: false,
          sizeClass: sizeClass(n),
          editable: sizeClass(n) === "editable",
          highlight: false,
        });
        /** `JSON.stringify` 한 UTF-8 바이트(UTF-16 한 단위 — 대리 단위는 반쪽마다 2). */
        const cost = (c: number) =>
          c === 0x22 || c === 0x5c
            ? 2
            : c < 0x20
              ? [8, 9, 10, 12, 13].includes(c)
                ? 2
                : 6
              : c < 0x80
                ? 1
                : c < 0x800 || (c >= 0xd800 && c <= 0xdfff)
                  ? 2
                  : 3;

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
              sizeClass: sizeClass(size(path)),
            });
          case "fs.read": {
            if (!files.has(path)) break;
            // 범위 읽기는 그 범위만 준다(실제 Host 처럼 — 8 KiB 앞부분 읽기가 큰 파일을 통째로 싣지 않는다).
            const ranged = params.offset !== undefined || params.length !== undefined;
            const from = Number(params.offset ?? 0);
            const whole = files.get(path)!.text;
            const text = ranged
              ? whole.slice(
                  from,
                  params.length === undefined ? undefined : from + Number(params.length),
                )
              : whole;
            return reply({
              path,
              size: size(path),
              mtimeMs: files.get(path)!.mtime,
              offset: from,
              eof: from + text.length >= whole.length,
              version: ranged ? null : version(path),
              content: params.bytes ? { kind: "base64", data: btoa(text) } : { kind: "text", text },
              judgment: judgment(size(path)),
            });
          }
          // 조각 읽기(crates/pw-fs/src/chunks.rs): 조각마다 지금 파일로 자르고 `expectedVersion` 이 다르면 `CONFLICT`. 내용 자리는
          // 이 가짜의 단위(UTF-16)다 — 받는 쪽은 `next` 를 돌려줄 뿐이다.
          case "fs.readChunk": {
            if (!files.has(path)) break;
            const max = Number(params.maxBytes);
            if (!(max >= 16 && max <= 16 * MIB)) return fail(-32602, "maxBytes out of range");
            if (size(path) > 50 * MIB) return fail(-32602, "larger than 50 MiB");
            if (params.expectedVersion != null && params.expectedVersion !== version(path))
              return fail(-32004, "conflict", { path, current: version(path) });
            const text = files.get(path)!.text;
            const offset = Number(params.offset ?? 0);
            let used = 2;
            let end = offset;
            while (end < text.length) {
              const c = cost(text.charCodeAt(end));
              if (used + c > max) break;
              used += c;
              end++;
            }
            // 대리 쌍을 가르지 않는다.
            if (end < text.length && /[\ud800-\udbff]/.test(text[end - 1] ?? "")) end--;
            return reply({
              path,
              size: size(path),
              mtimeMs: files.get(path)!.mtime,
              version: version(path),
              judgment: judgment(size(path)),
              offset,
              next: end,
              total: text.length,
              eof: end === text.length,
              content: { kind: "text", text: text.slice(offset, end) },
            });
          }
          // 조각 쓰기: 모음은 연 연결 · 프로젝트 · 경로에 묶이고(남의 연결에는 없는 것과 같다), 커밋에서만 파일이 바뀐다.
          case "fs.writeChunk": {
            const content = params.content as { kind: string; text?: string; data?: string };
            const piece = content.text ?? content.data ?? "";
            const text = content.kind === "text";
            // 모은 바이트는 실제 Host 처럼 UTF-8(텍스트) · 푼 바이트(base64)로 센다 — 조각의 `offset` 이 그것과 같아야 한다.
            const pieceBytes = text ? new TextEncoder().encode(piece).length : atob(piece).length;
            const offset = Number(params.offset ?? 0);
            const id = params.upload as string | null | undefined;
            if (id == null) {
              if (offset !== 0) return fail(-32602, "the chunk must start at byte 0");
              const mine = [...fake.uploads.values()].filter((u) => u.conn === conn).length;
              if (fake.uploads.size >= 4 || mine >= 2)
                return fail(-32603, "internal error", "chunked write is busy: too many uploads");
              const upload = `u${++fake.clock}`;
              fake.uploads.set(upload, {
                conn,
                projectId: params.projectId,
                path,
                text,
                pieces: [piece],
                bytes: pieceBytes,
              });
              reply({ upload, received: pieceBytes });
            } else {
              const u = fake.uploads.get(id);
              if (!u || u.conn !== conn) return fail(-32005, "no such upload", { upload: id });
              if (u.projectId !== params.projectId || u.path !== path || u.text !== text)
                return fail(-32602, "the upload was opened for another file");
              if (offset !== u.bytes)
                return fail(-32602, `the chunk must start at byte ${u.bytes}`);
              if (u.bytes + pieceBytes > 50 * MIB) {
                fake.uploads.delete(id);
                return fail(-32602, "a chunked write is at most 50 MiB");
              }
              u.pieces.push(piece);
              u.bytes += pieceBytes;
              reply({ upload: id, received: u.bytes });
            }
            if (fake.dropAfterChunks !== undefined && --fake.dropAfterChunks <= 0) {
              fake.dropAfterChunks = undefined;
              dropChannel();
            }
            return;
          }
          case "fs.writeCommit": {
            const id = String(params.upload);
            const u = fake.uploads.get(id);
            if (!u || u.conn !== conn) return fail(-32005, "no such upload", { upload: id });
            if (u.projectId !== params.projectId || u.path !== path)
              return fail(-32602, "the upload was opened for another file");
            fake.uploads.delete(id);
            if (params.size != null && params.size !== u.bytes)
              return fail(-32602, "the upload holds another size");
            if (u.bytes + (params.bom ? 3 : 0) > 50 * MIB)
              return fail(-32602, "a chunked write is at most 50 MiB");
            const current = files.has(path) ? version(path) : null;
            if ((params.expectedVersion ?? null) !== current)
              return fail(-32004, "conflict", { path, current });
            const text = u.pieces.join("");
            files.set(path, { text, mtime: ++fake.clock });
            return reply({ path, size: text.length, mtimeMs: fake.clock, version: version(path) });
          }
          case "fs.writeAbort": {
            const u = fake.uploads.get(String(params.upload));
            if (u?.conn === conn) fake.uploads.delete(String(params.upload));
            return reply(null);
          }
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
      function dropChannel() {
        // 닫힌 연결의 모음은 Host 가 버린다(`Ctx::connection_closed`).
        for (const [id, u] of fake.uploads) if (u.conn === conn) fake.uploads.delete(id);
        conn.closed = true;
        fake.dropped.push(conn);
        for (const end of streams.values()) end();
        client.close();
      }
      return {
        transport: client,
        close: async () => {
          for (const [id, u] of fake.uploads) if (u.conn === conn) fake.uploads.delete(id);
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
  fake.dropped = [];
  fake.largest = 0;
  fake.uploads = new Map();
  fake.dropAfterChunks = undefined;
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

// 링크 틀(editor/link-limits.ts): 틀을 넘는 메시지는 그 머신의 채널을 닫는다. 다른 머신 문서는 조각으로 읽고 쓴다(M7-13b —
// editor/link-chunks.ts, crates/pw-fs/src/chunks.rs). 상한은 이 머신과 같은 50 MiB 다.
const MIB = 1024 * 1024;
/** 제어 문자 63 개 + 줄바꿈 줄로 채운 `bytes` 바이트의 글 — JSON 에서 거의 6배(`\u0001`). 8 KiB 뒤에 NUL 하나(바이너리가 아니다). */
function worstText(bytes: number): string {
  const text = `${"\u0001".repeat(63)}\n`.repeat(bytes / 64);
  return `${text.slice(0, 10_000)}\u0000${text.slice(10_001)}`;
}
const callsOf = (machine: string, method: string) =>
  fake.calls.get(machine)!.filter((c) => c.method === method);
async function replace(view: EditorView, text: string) {
  await act(async () =>
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: text },
      userEvent: "input.type",
    }),
  );
  await settle();
}

it("opens a 30 MiB worst-case remote file in chunks without any message over the link frame", async () => {
  await mount();
  const text = worstText(30 * MIB);
  fake.files.get(MINI)!.set("/work/demo/ctl.log", { text, mtime: 1 });
  await act(
    async () =>
      void openTab(dock!, { path: qualify(MINI, "/work/demo/ctl.log"), worktree: REMOTE_MAIN }),
  );
  // 통째로는 ~180 MiB 다 — 16 MiB 조각 여럿이 모두 같은 `version` 으로 온다.
  await vi.waitFor(() => expect(document.body.textContent).toContain("읽기 전용으로 연다"));
  const chunks = callsOf(MINI, "fs.readChunk").filter(
    (c) => c.params.path === "/work/demo/ctl.log",
  );
  expect(chunks.length).toBeGreaterThan(10);
  expect(chunks.slice(1).every((c) => c.params.expectedVersion === "1:" + text.length)).toBe(true);
  expect(callsOf(MINI, "fs.read").filter((c) => c.params.length === undefined)).toHaveLength(0);
  expect(document.body.textContent).not.toContain("열지 않는다");
  expect(fake.largest).toBeLessThanOrEqual(16 * MIB + 64 * 1024);
  expect(fake.dropped).toHaveLength(0);
  expect(member().closed).toBe(false);
});

it("does not open a remote file over 50 MiB, just like this machine's", async () => {
  await mount();
  const BIG = 50 * MIB + 1;
  fake.files.get(MINI)!.set("/work/demo/big.log", { text: "big remote", mtime: 1 });
  fake.sizes.get(MINI)!.set("/work/demo/big.log", BIG);
  await act(
    async () =>
      void openTab(dock!, { path: qualify(MINI, "/work/demo/big.log"), worktree: REMOTE_MAIN }),
  );
  await vi.waitFor(() => expect(document.body.textContent).toContain("50 MiB 를 넘는 파일이라"));
  // 열지 않은 큰 파일의 안내다 — [앞부분 보기] · [hex] 는 범위 읽기라 링크 틀 안이다.
  expect(document.querySelector("[data-notice-action=head]")).not.toBeNull();
  expect(callsOf(MINI, "fs.readChunk")).toHaveLength(0);
  expect(viewWith("big remote")).toBeUndefined();
});

it("keeps a remote document when the file grew past 50 MiB and the read again is refused", async () => {
  await openBoth();
  const conn = liveMember()!;
  await vi.waitFor(() => expect(conn.subscriptions.size).toBeGreaterThan(0));
  fake.files.get(MINI)!.set(FILE, { text: "grown", mtime: ++fake.clock });
  fake.sizes.get(MINI)!.set(FILE, 50 * MIB + 1);
  await changed(conn, [FILE]);
  await settle();
  expect(viewWith("remote text")).toBeDefined();
  expect(fake.dropped).toHaveLength(0);
});

it("saves 33 MiB to the remote machine in chunks, and this machine's tab with one write", async () => {
  const remote = await openBoth();
  // 따옴표는 JSON 에서 두 바이트다 — 한 번의 `fs.write` 로는 66 MiB(틀을 넘는다). 조각으로 모아 한 번에 쓴다.
  // (제어 문자는 6배지만 happy-dom 의 CodeMirror 가 글자마다 표시를 그려 메모리가 넘친다 — 쓰기의 제어 문자 자르기는
  // link-chunks.test.ts 가 30 MiB 로, 읽기는 위 시험이 본다.)
  await replace(remote, '"'.repeat(33 * MIB));
  await saveActive();
  await vi.waitFor(() => expect(callsOf(MINI, "fs.writeCommit")).toHaveLength(1));
  expect(callsOf(MINI, "fs.writeChunk").length).toBeGreaterThan(4);
  expect(callsOf(MINI, "fs.writeChunk").every((c) => c.params.path === FILE)).toBe(true);
  expect(writes(MINI)).toHaveLength(0);
  expect(fake.files.get(MINI)!.get(FILE)!.text.length).toBe(33 * MIB);
  expect(fake.uploads.size).toBe(0);
  expect(fake.largest).toBeLessThanOrEqual(16 * MIB + 64 * 1024);
  expect(fake.dropped).toHaveLength(0);
  // 엉뚱한 머신에 아무것도 가지 않았다.
  expect(callsOf("local", "fs.writeChunk")).toHaveLength(0);
  expect(fake.files.get("local")!.get(FILE)!.text).toBe("local text");

  // 저장한 `version` 이 다음 저장의 `expectedVersion` 이다 — 두 번째 저장도 충돌 없이 그 머신에.
  await type(remote, "more ");
  await saveActive();
  await vi.waitFor(() => expect(callsOf(MINI, "fs.writeCommit")).toHaveLength(2));
  expect(fake.files.get(MINI)!.get(FILE)!.text.startsWith("more ")).toBe(true);
  expect(banner()).toBeNull();

  // 이 머신의 탭은 링크를 지나지 않는다 — 같은 크기도 `fs.write` 한 번.
  await replace(await viewOf("main.rs", false), '"'.repeat(33 * MIB));
  await saveActive();
  expect(writes("local")).toHaveLength(1);
  expect(fake.files.get("local")!.get(FILE)!.text.length).toBe(33 * MIB);
});

it("leaves the remote file untouched and keeps the edits when the link drops while chunks are sent", async () => {
  const remote = await openBoth();
  // 평문 20 MiB — 한 조각(16 MiB)보다 커서 조각으로 가지만 통째 `fs.write` 로도 틀(64 MiB) 안이다. 통째로 보냈다면 끊기기 전에
  // 그 머신의 파일이 바뀌었을 것이다(리뷰 F2). 첫 조각을 받은 뒤 링크가 끊긴다.
  await replace(remote, "a".repeat(20 * MIB));
  fake.dropAfterChunks = 1;
  await saveActive();
  // 끊기거나(조각) 통째 `fs.write` 가 닿을 때까지 — 어느 쪽이든 먼저 그 머신의 파일을 본다.
  await vi.waitFor(() => expect(fake.dropped.length + writes(MINI).length).toBeGreaterThan(0));
  await settle();
  // 커밋은 가지 않았고 모음은 버려졌다 — 그 머신의 파일은 그대로, 편집은 탭에 남는다.
  expect(fake.files.get(MINI)!.get(FILE)!.text).toBe("remote text");
  expect(fake.dropped).toHaveLength(1);
  expect(callsOf(MINI, "fs.writeCommit")).toHaveLength(0);
  expect(fake.uploads.size).toBe(0);
  expect(callsOf(MINI, "fs.writeChunk")).toHaveLength(1);
  expect(remote.state.doc.length).toBe(20 * MIB);
  expect(writes(MINI)).toHaveLength(0);
  expect(writes("local")).toHaveLength(0);
  expect(callsOf("local", "fs.writeChunk")).toHaveLength(0);
});

it("refuses to send more than 50 MiB to the remote machine", async () => {
  const remote = await openBoth();
  await replace(remote, "a".repeat(50 * MIB + 1));
  await saveActive();
  await vi.waitFor(() => expect(document.body.textContent).toContain("보내지 않았다"));
  expect(callsOf(MINI, "fs.writeChunk")).toHaveLength(0);
  expect(writes(MINI)).toHaveLength(0);
  expect(fake.files.get(MINI)!.get(FILE)!.text).toBe("remote text");
});

// 리뷰 R-a: 원격 Markdown 의 상대 이미지는 그 머신의 루트(머신을 붙인 것)로 풀어 그 머신에서 읽는다.
it("reads a remote Markdown document's relative image on that machine", async () => {
  await mount();
  fake.files
    .get(MINI)!
    .set("/work/demo/README.md", { text: "# hi\n\n![logo](logo.png)\n", mtime: 1 });
  fake.files.get(MINI)!.set("/work/demo/logo.png", { text: "PNGDATA", mtime: 1 });
  await act(
    async () =>
      void openTab(dock!, { path: qualify(MINI, "/work/demo/README.md"), worktree: REMOTE_MAIN }),
  );
  await vi.waitFor(() =>
    expect(
      fake.calls
        .get(MINI)!
        .some((c) => c.method === "fs.read" && c.params.path === "/work/demo/logo.png"),
    ).toBe(true),
  );
  expect(
    fake.calls.get("local")!.filter((c) => c.params.path === "/work/demo/logo.png"),
  ).toHaveLength(0);
});

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
