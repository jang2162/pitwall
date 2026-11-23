// @vitest-environment happy-dom
// 실행 · 터미널의 대상 체크아웃(18-m7-plan M7-14 — 웹뷰 시험, StrictMode 는 test-support/strict-mode.ts). 묶음 창(`?project=p1`)이
// 이 머신(p1)과 다른 머신 `MINI`(멤버 r1)를 안다. 가짜 Host 는 연결 대상마다 따로이고 **실제 범위 규칙대로** 받는다(M7-11 리뷰 B1):
// `remote.*` 는 Host 범위 연결만, `terminal.*` · `run.*` · `fs.*` 는 프로젝트(멤버) 연결만, 그 연결의 프로젝트 것만. 다른 머신의
// 셸은 그 머신(Relay)의 것이라 연결이 닫혀도 산다(신뢰한 프로젝트 — M7-09 ②), 미신뢰면 연결과 함께 끝나고 떼기 · 붙기를 거절한다.
// 두 머신의 셸 번호 · 실행 번호 · 경로가 같다 — 섞이면 엉뚱한 머신에 입력 · 중지 · 열기가 간다(공격자 입장의 시험: 사람이 엉뚱한
// 머신에 쓰거나 실행하는 길).

import type { DockviewApi } from "dockview-react";
import type { RemoteStatus, RunConfigList, RunConfirmRequired } from "@pitwall/rpc-client";
import { ActionRegistry } from "@pitwall/keymap";
import { act as reactAct } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, vi } from "vitest";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../App";
import { checkoutKey } from "../checkout/checkout";
import { resetWindowFileTree } from "../file-tree/project-view";
import type { KeymapNative } from "../keymap/native";
import { activeWorktreeStore } from "../open-with/active-worktree";
import { projectOpenInbox } from "../project-open";
import { terminalTarget } from "../terminal/terminal-tool-window";
import { windowTerminals } from "../terminal/terminal-tabs";
import { act, endTest, it } from "../test-support/ended-test";
import { createThemeStore } from "../theme/theme-store";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";
import { runTarget } from "./run-picker";
import { resetWindowRunConfigs } from "./run-configs";
import { windowRuns } from "./run-tabs";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

// 창 하나를 통째로 그리고 두 머신을 오가는 긴 시나리오다 — 여러 워크트리가 함께 빌드하는 부하에서 기본 5초를 넘은 적이 있다.
vi.setConfig({ testTimeout: 20_000 });

const MINI = "SHA256:mini/fingerprint";

interface Shell {
  id: number;
  project: string;
  worktree: string | null;
  log: string;
  /** 붙은 스트림(연결 번호 · 스트림 번호). */
  stream?: { conn: number; streamId: number };
  exited?: boolean;
}

const fake = vi.hoisted(() => ({
  /** 머신 → 셸. 두 머신 모두 1번부터 센다. */
  shells: new Map<string, Map<number, Shell>>(),
  nextShell: new Map<string, number>(),
  /** 머신 → 받은 호출(메서드 · 인자). */
  calls: new Map<string, { method: string; params: Record<string, unknown> }[]>(),
  /** 머신 → 그 머신 프로젝트의 신뢰. */
  trusted: new Map<string, boolean>(),
  /** 연결 번호 → 보내기 · 머신. */
  conns: new Map<number, { machine: string; send(m: object): void; closed: boolean }>(),
  /** 열린 `remote.connect` 스트림. */
  links: [] as { send(m: object): void; streamId: number }[],
  /** 머신 → 실행. */
  runs: new Map<string, Record<string, unknown>[]>(),
  /** 머신 → 실행 출력(Host 의 버퍼)과 붙은 스트림. */
  runLogs: new Map<string, { log: string; stream?: { conn: number; streamId: number } }>(),
  /** `run.start` 가 처음에 첫 실행 확인(7001)을 낼 것인가. */
  confirm: undefined as unknown,
  /** 그 머신이 떼어 둘 수 있는 수(넘으면 conflict). */
  limit: 16,
  /** 지금 링크 상태(새 `remote.connect` 스트림의 첫 항목). */
  status: undefined as unknown,
  files: new Map<string, Set<string>>(),
}));

vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({ onDragDropEvent: async () => () => {} }),
}));

vi.mock("../tauri-transport", async () => {
  const { createMemoryTransportPair: pair } = await import("@pitwall/rpc-client");
  const { DEFAULT_KEYMAP_TOML } = await import("../keymap/default-keymap");
  const { i18nState } = await import("../i18n/testing");
  const MINI_ID = "SHA256:mini/fingerprint";
  let connNo = 0;
  const b64 = (text: string) => btoa(String.fromCharCode(...new TextEncoder().encode(text)));
  return {
    connectHost: async (target: Record<string, unknown>) => {
      const machine = "member" in target ? (target.member as { host: string }).host : "local";
      // 연결의 범위: Host 범위(`hostId`) · 프로젝트(창 · 멤버). 프로젝트 연결의 프로젝트는 그 머신의 것.
      const hostScope = "hostId" in target;
      const project =
        "member" in target
          ? (target.member as { project: string }).project
          : "projectId" in target
            ? String(target.projectId)
            : undefined;
      const [client, server] = pair();
      const conn = ++connNo;
      let next = 1;
      const send = (m: object) => {
        if (!fake.conns.get(conn)?.closed) server.send(JSON.stringify({ jsonrpc: "2.0", ...m }));
      };
      fake.conns.set(conn, { machine, send, closed: false });
      const shells = () => fake.shells.get(machine)!;
      const remote = machine !== "local";
      const chunk = (streamId: number, data: object) =>
        send({ method: "stream.chunk", params: { streamId, data } });
      server.onClose?.(() => {
        fake.conns.get(conn)!.closed = true;
        // 연결이 닫히면: 다른 머신(Relay)의 신뢰한 프로젝트 셸은 떼어져 남고, 미신뢰 · 이 머신의 셸은 끝난다.
        for (const shell of shells().values()) {
          if (shell.stream?.conn !== conn) continue;
          shell.stream = undefined;
          if (!remote || !fake.trusted.get(machine)) shells().delete(shell.id);
        }
      });
      server.onMessage((text) => {
        const m = JSON.parse(text) as {
          id?: number;
          method: string;
          params?: Record<string, unknown>;
        };
        const params = m.params ?? {};
        const reply = (result: unknown) => send({ id: m.id, result });
        const fail = (code: number, message: string, data: unknown = null) =>
          send({ id: m.id, error: { code, message, data } });
        if (m.method !== "stream.credit")
          fake.calls.get(machine)!.push({ method: m.method, params });
        if (m.method.startsWith("remote.") && !hostScope)
          return fail(-32001, "remote.* takes only a host connection");
        if (/^(terminal|run|fs)\./.test(m.method) && hostScope)
          return fail(-32001, `${m.method} takes a project window connection`);
        const info = (s: Shell) => ({
          terminalId: s.id,
          worktree: s.worktree,
          cwd: "/work/demo",
          shell: ["/bin/zsh", "-l"],
          pid: 100 + s.id,
          attached: s.stream !== undefined,
          exited: s.exited === true,
          code: null,
        });
        const mine = (id: unknown) => {
          const s = shells().get(Number(id));
          return s && s.project === project ? s : undefined;
        };
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
            return reply({ projectId: project, ownerId: project, state: "trusted" });
          case "project.state.get":
            return reply({ state: null, problem: null });
          case "project.state.set":
            return reply(null);
          case "task.list":
            return reply([]);
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
            const streamId = next++;
            const link = { streamId, send: (data: object) => chunk(streamId, data) };
            fake.links.push(link);
            reply({ streamId });
            return link.send(fake.status as object);
          }
          case "stream.cancel": {
            const { streamId } = params as { streamId: number };
            fake.links = fake.links.filter((l) => !(l.streamId === streamId));
            // 셸 스트림을 끊으면: 이 머신은 셸을 끝내고, 다른 머신은 떼어 둔다(신뢰한 프로젝트).
            for (const shell of shells().values())
              if (shell.stream?.conn === conn && shell.stream.streamId === streamId) {
                shell.stream = undefined;
                if (!remote || !fake.trusted.get(machine)) shells().delete(shell.id);
              }
            return;
          }
          case "stream.credit":
            return;
          case "events.subscribe":
            return reply({ subscriptionId: next++ });
          case "events.unsubscribe":
            return reply(null);
          case "lang.detect":
            return reply({ files: [] });
          case "fs.stat": {
            const path = String(params.path);
            if (!fake.files.get(machine)?.has(path)) break;
            return reply({
              path,
              kind: "file",
              target: null,
              size: 1,
              mtimeMs: 1,
              readonly: false,
              sizeClass: "editable",
            });
          }
          case "terminal.create": {
            if (params.projectId !== project) return fail(-32001, "another project");
            const id = fake.nextShell.get(machine)!;
            fake.nextShell.set(machine, id + 1);
            const streamId = next++;
            shells().set(id, {
              id,
              project: project!,
              worktree: (params.worktree as string | null) ?? null,
              log: "",
              stream: { conn, streamId },
            });
            reply({ streamId });
            return chunk(streamId, {
              kind: "started",
              terminalId: id,
              pid: 100 + id,
              shell: ["/bin/zsh", "-l"],
              cwd: "/work/demo",
            });
          }
          case "terminal.input":
          case "terminal.resize":
            if (!mine(params.terminalId)) return fail(-32602, "terminal is not running");
            return reply(null);
          case "terminal.close": {
            const s = mine(params.terminalId);
            if (s) {
              if (s.stream) chunk(s.stream.streamId, { kind: "exited", code: null });
              shells().delete(s.id);
            }
            return reply(null);
          }
          case "terminal.list":
            if (!remote) return reply({ terminals: [] });
            return reply({
              terminals: [...shells().values()].filter((s) => s.project === project).map(info),
            });
          case "terminal.detach": {
            if (!remote) return fail(-32602, "a local terminal is not detached");
            if (!fake.trusted.get(machine))
              return fail(-32001, "untrusted project", { reason: "untrusted" });
            const s = mine(params.terminalId);
            if (!s) return fail(-32602, "terminal is not running");
            const detached = [...shells().values()].filter((x) => !x.stream).length + 1;
            if (detached > fake.limit && !params.endOldest)
              return fail(-32004, "limit", { wouldEnd: [], otherProjects: 1, limit: fake.limit });
            s.stream = undefined;
            return reply({ ended: [] });
          }
          case "terminal.attach": {
            if (!remote) return fail(-32602, "terminal is not running");
            if (!fake.trusted.get(machine))
              return fail(-32001, "untrusted project", { reason: "untrusted" });
            const s = mine(params.terminalId);
            if (!s) return fail(-32602, "terminal is not running");
            const since = Number(params.since ?? 0);
            const streamId = next++;
            s.stream = { conn, streamId };
            reply({ streamId });
            chunk(streamId, { kind: "attached", terminal: info(s), from: since, dropped: 0 });
            if (s.log.length > since)
              chunk(streamId, { kind: "output", offset: since, data: b64(s.log.slice(since)) });
            return;
          }
          case "run.config.list":
            return reply(LIST_OF(machine));
          case "run.list":
            return reply({ runs: fake.runs.get(machine)!.filter((r) => r.projectId === project) });
          case "run.start": {
            if (params.projectId !== project) return fail(-32001, "another project");
            if (fake.confirm && params.confirmed !== (fake.confirm as { hash: string }).hash)
              return fail(7001, "not seen", fake.confirm);
            const run = RUN(machine, project!, (params.worktree as string | null) ?? null);
            fake.runs.get(machine)!.push(run);
            return reply(run);
          }
          case "run.attach": {
            const run = fake.runs.get(machine)!.find((r) => r.runId === params.runId);
            if (!run || run.projectId !== project) return fail(-32602, "no such run");
            const streamId = next++;
            reply({ streamId });
            const since = Number(params.since ?? 0);
            const out = fake.runLogs.get(machine) ?? { log: "" };
            fake.runLogs.set(machine, out);
            out.stream = { conn, streamId };
            chunk(streamId, { kind: "attached", run, from: since, dropped: 0 });
            if (out.log.length > since)
              chunk(streamId, { kind: "output", offset: since, data: b64(out.log.slice(since)) });
            return;
          }
          case "run.restart": {
            const run = fake.runs.get(machine)!.find((r) => r.runId === params.runId);
            if (!run || run.projectId !== project) return fail(-32602, "no such run");
            if (fake.confirm && params.confirmed !== (fake.confirm as { hash: string }).hash)
              return fail(7001, "not seen", fake.confirm);
            return reply({ ...run, runId: "run-2", restartOf: run.runId });
          }
          case "run.stop": {
            const run = fake.runs.get(machine)!.find((r) => r.runId === params.runId);
            if (!run || run.projectId !== project) return fail(-32602, "no such run");
            run.state = "exited";
            run.exit = { code: 0, signal: null };
            return reply(run);
          }
        }
        if (m.id !== undefined) fail(-32601, `no ${m.method} on ${machine}`);
      });
      return { transport: client, close: async () => client.close() };
    },
  };
});

// 두 머신이 같은 구성 id · 같은 실행 번호를 낸다 — 섞이면 엉뚱한 머신의 실행을 멈춘다.
function LIST_OF(machine: string): RunConfigList {
  return {
    configs: [
      {
        id: "dev",
        config: {
          name: machine === "local" ? "Dev (local)" : "Dev (mini)",
          type: "shell",
          command: "pnpm dev",
          cwd: null,
          env: {},
          envFile: [],
          profiles: {},
          debug: { adapter: "debugpy" },
        },
        layers: [{ origin: "shared", path: "/work/demo/.pitwall/run/dev.toml" }],
      },
    ],
    unusable: [],
    detected: [],
    detectProblems: [],
    taskLists: [],
    problem: null,
  };
}

function RUN(
  _machine: string,
  projectId: string,
  worktree: string | null,
): Record<string, unknown> {
  return {
    runId: "run-1",
    projectId,
    config: "dev",
    name: "Dev",
    kind: "process",
    launchedBy: { kind: "window", window: "main" },
    worktree,
    cwd: "/work/demo",
    profile: null,
    pid: 100,
    state: "running",
    exit: null,
    startedAt: 1,
    endedAt: null,
    restartOf: null,
    logStart: 0,
    logEnd: 0,
    ports: [{ port: 5173, allAddresses: false, addresses: ["127.0.0.1"] }],
  };
}

const CONFIRM: RunConfirmRequired = {
  hash: "sha256:" + "b".repeat(64),
  shown: {
    key: "dev",
    name: "Dev",
    kind: "shell",
    argv: ["/bin/zsh", "-l", "-c", "pnpm dev"],
    cwd: "app",
    profile: null,
    env: [],
    envFiles: [],
    definition: null,
    definitionSource: null,
    secretLooking: false,
    recordable: true,
  },
};

let closeGuard: (() => Promise<boolean>) | undefined;
const native = {
  syncMenu: async () => {},
  onMenuAction: () => () => {},
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
let registries = new Set<ActionRegistry>();
const registry = () => [...registries].at(-1)!;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
  );
  vi.spyOn(console, "info").mockImplementation(() => {});
  vi.spyOn(console, "warn").mockImplementation(() => {});
  window.history.replaceState(null, "", "/?window=project&project=p1");
  for (const machine of ["local", MINI]) {
    fake.shells.set(machine, new Map());
    fake.nextShell.set(machine, 1);
    fake.calls.set(machine, []);
    fake.runs.set(machine, []);
    fake.trusted.set(machine, true);
    fake.files.set(machine, new Set());
  }
  fake.runLogs = new Map();
  fake.conns = new Map();
  fake.links = [];
  fake.confirm = undefined;
  fake.limit = 16;
  fake.status = { state: "connected", machine: MINI };
  activeWorktreeStore.setState({ top: undefined });
  terminalTarget().setState({ chosen: undefined });
  runTarget().setState({ chosen: undefined });
  resetWindowFileTree();
  resetWindowRunConfigs();
  localStorage.clear();
  sessionStorage.clear();
  const register = ActionRegistry.prototype.register;
  const seen = new Set<ActionRegistry>();
  registries = seen;
  vi.spyOn(ActionRegistry.prototype, "register").mockImplementation(function (
    this: ActionRegistry,
    action,
  ) {
    seen.add(this);
    return register.call(this, action);
  });
});

afterEach(async () => {
  await endTest();
  await reactAct(async () => root?.unmount());
  await reactAct(async () => windowTerminals().disposeAll());
  await reactAct(async () => windowRuns().disposeAll());
  root = undefined;
  dock = undefined;
  closeGuard = undefined;
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
  await vi.waitFor(() => expect(byTestId("run-toolbar")).not.toBeNull());
}

const byTestId = (id: string) => document.querySelector<HTMLElement>(`[data-testid=${id}]`);
const calls = (machine: string, method: string) =>
  fake.calls.get(machine)!.filter((c) => c.method === method);
const b64 = (text: string) => btoa(String.fromCharCode(...new TextEncoder().encode(text)));

async function click(el: Element | null | undefined) {
  expect(el).toBeTruthy();
  await act(async () => (el as HTMLElement).click());
  await settle();
}

/** 링크 상태를 바꾼다(그 머신을 쓰는 창의 `remote.connect` 스트림 모두에). */
async function link(status: RemoteStatus) {
  fake.status = status;
  await act(async () => {
    for (const l of fake.links) l.send(status);
  });
  await settle();
}

/** 그 머신 셸의 출력 — 붙은 스트림에 보내고, 떼어져 있으면 그 머신의 버퍼에 쌓는다(Relay 의 링). */
async function shellOut(machine: string, id: number, text: string) {
  const shell = fake.shells.get(machine)!.get(id)!;
  const offset = shell.log.length;
  shell.log += text;
  if (shell.stream) {
    const conn = fake.conns.get(shell.stream.conn)!;
    await act(async () =>
      conn.send({
        method: "stream.chunk",
        params: {
          streamId: shell.stream!.streamId,
          data: {
            kind: "output",
            data: b64(text),
            offset: machine === "local" ? null : offset,
          },
        },
      }),
    );
  }
  await settle();
}

/** 그 머신 실행의 출력 — 붙은 콘솔 스트림에 보내고, 끊겼으면 Host 버퍼에만 쌓는다. */
async function runOut(machine: string, text: string) {
  const out = fake.runLogs.get(machine) ?? { log: "" };
  fake.runLogs.set(machine, out);
  const offset = out.log.length;
  out.log += text;
  const conn = out.stream && fake.conns.get(out.stream.conn);
  if (conn && !conn.closed)
    await act(async () =>
      conn.send({
        method: "stream.chunk",
        params: {
          streamId: out.stream!.streamId,
          data: { kind: "output", offset, data: b64(text) },
        },
      }),
    );
  await settle();
}

/** 고르기 단추로 대상 체크아웃을 고른다(`which` 는 `data-checkout` 의 값 — 활성 체크아웃 따라가기는 `active`). */
async function choose(testId: string, which: string) {
  await click(byTestId(testId));
  const menu = byTestId("checkout-target-menu");
  expect(menu).not.toBeNull();
  await click(menu!.querySelector(`[data-checkout="${which}"]`));
}

async function showTerminal() {
  await act(async () => {
    document.body.dispatchEvent(
      new KeyboardEvent("keydown", { code: "F12", key: "F12", altKey: true, bubbles: true }),
    );
  });
  await settle();
}

const terminalTabs = () => [
  ...document.querySelectorAll<HTMLElement>("[data-testid=terminal-tab]"),
];
const runTabs = () => [...document.querySelectorAll<HTMLElement>("[data-testid=run-tab]")];
const REMOTE_MAIN = checkoutKey({ machine: MINI })!;
const screenText = (id: number) => {
  const term = windowTerminals().instance(id)!.term;
  const lines: string[] = [];
  for (let y = 0; y < term.buffer.active.length; y++)
    lines.push(term.buffer.active.getLine(y)!.translateToString(true));
  return lines.join("\n");
};

/** 다른 머신 `MINI` 를 대상으로 셸을 연다. 돌려주는 것은 탭 id. */
async function openMiniShell(): Promise<number> {
  await showTerminal();
  await vi.waitFor(() => expect(byTestId("terminal-target")).not.toBeNull());
  await choose("terminal-target", REMOTE_MAIN);
  await click(byTestId("terminal-new-tab"));
  await vi.waitFor(() => expect(calls(MINI, "terminal.create")).toHaveLength(1));
  const id = windowTerminals().store.getState().activeId!;
  await vi.waitFor(() => expect(windowTerminals().instance(id)!.state.kind).toBe("running"));
  return id;
}

it("starts a terminal on the chosen checkout's machine and keeps it across an active checkout switch", async () => {
  await mount();
  const id = await openMiniShell();
  // 그 머신 연결 · 그 머신의 프로젝트로 — 이 머신에는 셸을 띄우지 않았다(⌥F12 가 연 첫 탭은 이 머신).
  expect(calls(MINI, "terminal.create")[0]!.params).toMatchObject({
    projectId: "r1",
    worktree: null,
  });
  expect(calls("local", "terminal.create")).toHaveLength(1);
  expect(calls("local", "terminal.create")[0]!.params).toMatchObject({ projectId: "p1" });
  // 탭 머리에 그 머신 이름이 글로 있다.
  const tab = terminalTabs().find((t) => t.dataset.machineTab === MINI)!;
  expect(tab.querySelector("[data-machine]")?.textContent).toBe("mini");
  expect(tab.textContent).not.toContain("로컬");

  // 입력은 그 머신 연결로, 그 머신의 셸 번호로 간다(두 머신 모두 1번이다).
  await act(async () => windowTerminals().instance(id)!.paste("ls\r"));
  await settle();
  await vi.waitFor(() => expect(calls(MINI, "terminal.input")).toHaveLength(1));
  expect(calls(MINI, "terminal.input")[0]!.params).toMatchObject({ terminalId: 1 });
  expect(calls("local", "terminal.input")).toHaveLength(0);

  // 활성 체크아웃을 그 머신으로, 다시 이 머신으로 바꿔도 터미널 탭은 남는다(탭 묶음에 들지 않는다).
  await act(async () => projectOpenInbox.push({ worktree: REMOTE_MAIN }));
  await settle();
  await act(async () => projectOpenInbox.push({ mainCheckout: true }));
  await settle();
  expect(windowTerminals().instance(id)).toBeDefined();
  expect(terminalTabs().some((t) => t.dataset.machineTab === MINI)).toBe(true);
  expect(windowTerminals().instance(id)!.state.kind).toBe("running");
});

it("keeps the remote shell stream while the link resumes and shows the tab read-only meanwhile", async () => {
  await mount();
  const id = await openMiniShell();
  await shellOut(MINI, 1, "before\r\n");
  await link({ state: "resuming", machine: MINI });
  // 읽기 전용 표시 — 글로. 입력은 버린다.
  await vi.waitFor(() => expect(byTestId("terminal-tab-read-only")?.textContent).toBe("읽기 전용"));
  await act(async () => windowTerminals().instance(id)!.paste("rm -rf x\r"));
  await settle();
  expect(calls(MINI, "terminal.input")).toHaveLength(0);
  // 이어 붙는 동안 그 머신 연결은 그대로라 스트림이 산다 — 출력이 그대로 온다(다시 붙기 · `terminal.attach` 없이).
  await shellOut(MINI, 1, "during\r\n");
  await link({ state: "connected", machine: MINI });
  await shellOut(MINI, 1, "after\r\n");
  expect(calls(MINI, "terminal.attach")).toHaveLength(0);
  expect(byTestId("terminal-tab-read-only")).toBeNull();
  const text = screenText(id);
  expect(text).toContain("before");
  expect(text).toContain("during");
  expect(text).toContain("after");
  await act(async () => windowTerminals().instance(id)!.paste("ok\r"));
  await vi.waitFor(() => expect(calls(MINI, "terminal.input")).toHaveLength(1));
});

it("marks a shell whose link really closed as disconnected and picks it up from where it was after reconnecting", async () => {
  await mount();
  const id = await openMiniShell();
  await shellOut(MINI, 1, "one\r\n");
  // 링크가 정말 닫혔다(이어 붙기 시한이 지남 — 새로 붙기를 기다림). 그 머신 연결이 닫혀 스트림이 끊긴다.
  await link({
    state: "reconnecting",
    attempt: 1,
    retryInMs: 1000,
    error: { stage: "handshake", kind: "link", message: "the link closed" },
  });
  await vi.waitFor(() => expect(windowTerminals().instance(id)!.state.kind).toBe("disconnected"));
  expect(byTestId("terminal-tab-disconnected")?.textContent).toBe("끊김");
  // 그동안 그 머신의 셸은 살아 있고 출력이 그 머신 버퍼에 쌓인다.
  await shellOut(MINI, 1, "two\r\n");
  await link({ state: "connected", machine: MINI });
  // 다시 붙으면 받은 끝부터 이어 받는다(같은 셸 번호 — 새 셸을 띄우지 않는다).
  await vi.waitFor(() => expect(calls(MINI, "terminal.attach")).toHaveLength(1));
  expect(calls(MINI, "terminal.attach")[0]!.params).toEqual({ terminalId: 1, since: 5 });
  expect(calls(MINI, "terminal.create")).toHaveLength(1);
  await vi.waitFor(() => expect(screenText(id)).toContain("two"));
  expect(screenText(id).match(/one/g)).toHaveLength(1);
  expect(windowTerminals().instance(id)!.state.kind).toBe("running");
  // 다시 연 셸 목록에는 이미 탭이 있는 셸이 다시 열리지 않는다.
  expect(terminalTabs().filter((t) => t.dataset.machineTab === MINI)).toHaveLength(1);
});

it("reopens the machine's detached shells per checkout once it connects, without taking the active tab", async () => {
  // 그 머신에 떼어진 셸 둘 — 한 워크트리 · 메인 체크아웃. 다른 프로젝트의 셸은 이 창의 연결에 보이지 않는다.
  fake.shells.get(MINI)!.set(4, { id: 4, project: "r1", worktree: "/srv/wt", log: "kept\r\n" });
  fake.shells.get(MINI)!.set(5, { id: 5, project: "r1", worktree: null, log: "" });
  fake.shells.get(MINI)!.set(6, { id: 6, project: "other", worktree: null, log: "" });
  await mount();
  await showTerminal();
  const localTab = windowTerminals().store.getState().activeId;
  await act(async () => projectOpenInbox.push({ worktree: REMOTE_MAIN }));
  await settle();
  await vi.waitFor(() =>
    expect(
      windowTerminals()
        .store.getState()
        .tabs.filter((t) => t.machine === MINI),
    ).toHaveLength(2),
  );
  const reopened = windowTerminals()
    .store.getState()
    .tabs.filter((t) => t.machine === MINI)
    .map((t) => windowTerminals().instance(t.id)!);
  expect(reopened.map((i) => [i.terminalId, i.worktree])).toEqual([
    [4, "/srv/wt"],
    [5, undefined],
  ]);
  expect(windowTerminals().store.getState().activeId).toBe(localTab);
  // 보이게 하면 처음부터(`since: 0`) 이어 받는다 — 새로 띄우지 않는다.
  const id = windowTerminals()
    .store.getState()
    .tabs.find((t) => t.machine === MINI)!.id;
  await act(async () => windowTerminals().activate(id));
  await settle();
  await vi.waitFor(() => expect(calls(MINI, "terminal.attach")).toHaveLength(1));
  expect(calls(MINI, "terminal.attach")[0]!.params).toEqual({ terminalId: 4, since: 0 });
  expect(calls(MINI, "terminal.create")).toHaveLength(0);
  await vi.waitFor(() => expect(screenText(id)).toContain("kept"));
});

it("asks End / Keep / Cancel when closing a remote shell's tab, Cancel focused, and keeps it on that machine", async () => {
  await mount();
  const id = await openMiniShell();
  const closeButton = () =>
    terminalTabs()
      .find((t) => t.dataset.machineTab === MINI)!
      .querySelector<HTMLElement>("button[aria-label$='닫기']");
  await click(closeButton());
  const dialog = await vi.waitFor(() => {
    const d = byTestId("remote-close-dialog");
    expect(d).not.toBeNull();
    return d!;
  });
  expect([...dialog.querySelectorAll("button")].map((b) => b.textContent)).toEqual([
    "끝내기",
    "두고 닫기",
    "취소",
  ]);
  expect(document.activeElement?.textContent).toBe("취소");
  expect(dialog.querySelector("[data-testid=target-machine]")?.textContent).toContain("mini");
  // [취소] — 아무것도 하지 않는다.
  await click(document.activeElement);
  expect(windowTerminals().instance(id)).toBeDefined();
  expect(calls(MINI, "terminal.detach")).toHaveLength(0);

  // [두고 닫기] — 상한에 걸리면 끝날 것을 묻고, 확인하면 `endOldest` 로 다시.
  fake.limit = 0;
  await click(closeButton());
  await click(byTestId("remote-close-dialog")!.querySelector("[data-answer=keep]"));
  const limit = await vi.waitFor(() => {
    const d = byTestId("keep-limit-dialog");
    expect(d).not.toBeNull();
    return d!;
  });
  expect(limit.textContent).toContain("다른 프로젝트 1개");
  expect(document.activeElement?.textContent).toBe("취소");
  await click(limit.querySelector("[data-answer=confirm]"));
  expect(calls(MINI, "terminal.detach").map((c) => c.params)).toEqual([
    { terminalId: 1, endOldest: false },
    { terminalId: 1, endOldest: true },
  ]);
  expect(calls(MINI, "terminal.close")).toHaveLength(0);
  expect(windowTerminals().instance(id)).toBeUndefined();
  // 셸은 그 머신에 남았다.
  expect(fake.shells.get(MINI)!.get(1)?.stream).toBeUndefined();
  expect(fake.shells.get(MINI)!.has(1)).toBe(true);
});

it("ends the remote shell on that machine with End, and refuses to keep an untrusted project's shell", async () => {
  await mount();
  const first = await openMiniShell();
  const closeMini = () =>
    terminalTabs()
      .find((t) => t.dataset.machineTab === MINI)!
      .querySelector<HTMLElement>("button[aria-label$='닫기']");
  await click(closeMini());
  await click(byTestId("remote-close-dialog")!.querySelector("[data-answer=end]"));
  expect(calls(MINI, "terminal.close").map((c) => c.params)).toEqual([{ terminalId: 1 }]);
  expect(fake.shells.get(MINI)!.has(1)).toBe(false);
  expect(windowTerminals().instance(first)).toBeUndefined();

  // 미신뢰 프로젝트의 셸은 연결 수명이다 — 그 머신이 떼기를 거절하면 알리고 탭은 그대로 둔다.
  fake.trusted.set(MINI, false);
  await click(byTestId("terminal-new-tab"));
  await vi.waitFor(() => expect(calls(MINI, "terminal.create")).toHaveLength(2));
  const second = windowTerminals().store.getState().activeId!;
  await click(closeMini());
  await click(byTestId("remote-close-dialog")!.querySelector("[data-answer=keep]"));
  expect(windowTerminals().instance(second)).toBeDefined();
  await vi.waitFor(() => expect(document.body.textContent).toContain("그 머신에 두지 못했다"));
});

it("asks before closing the window while shells and runs are left on another machine", async () => {
  await mount();
  await openMiniShell();
  expect(closeGuard).toBeDefined();
  let closing!: Promise<boolean>;
  await act(async () => {
    closing = closeGuard!();
  });
  await settle();
  const dialog = await vi.waitFor(() => {
    const d = byTestId("remote-close-dialog");
    expect(d).not.toBeNull();
    return d!;
  });
  expect(dialog.textContent).toContain("터미널 1개");
  expect(document.activeElement?.textContent).toBe("취소");
  await click(document.activeElement);
  expect(await closing).toBe(false);

  await act(async () => {
    closing = closeGuard!();
  });
  await settle();
  await click(byTestId("remote-close-dialog")!.querySelector("[data-answer=keep]"));
  expect(await closing).toBe(true);
  expect(calls(MINI, "terminal.detach")).toHaveLength(1);
});

it("opens a path the remote shell printed on that machine, not this machine's same path", async () => {
  const FILE = "/work/demo/src/main.rs";
  fake.files.get(MINI)!.add(FILE);
  fake.files.get("local")!.add(FILE);
  await mount();
  const id = await openMiniShell();
  await shellOut(MINI, 1, `${FILE}:3\r\n`);
  const term = windowTerminals().instance(id)!;
  const links = await new Promise<{ text: string; activate(e: MouseEvent, t: string): void }[]>(
    (r) => term.view.links.provideLinks(1, (l) => r((l ?? []) as never)),
  );
  expect(links.map((l) => l.text)).toEqual([`${FILE}:3`]);
  // 링크를 고를 때 있는지 본 것도 그 머신이다.
  expect(calls(MINI, "fs.stat").length).toBeGreaterThan(0);
  expect(calls("local", "fs.stat")).toHaveLength(0);
  await act(async () =>
    links[0]!.activate(new MouseEvent("click", { metaKey: true }), links[0]!.text),
  );
  await settle();
  // 여는 쪽도 그 머신의 파일을 본다(열기 입구가 그 머신의 `fs.stat`).
  expect(calls("local", "fs.stat")).toHaveLength(0);
  await vi.waitFor(() =>
    expect(
      dock!.panels.some((p) =>
        (p.params as { path?: string }).path?.includes(encodeURIComponent(MINI)),
      ),
    ).toBe(true),
  );
});

it("runs the chosen checkout's configuration on that machine, names it in the first-run confirmation, and keeps its tab", async () => {
  fake.confirm = CONFIRM;
  await mount();
  await choose("run-target", REMOTE_MAIN);
  // 구성 목록은 그 머신에서 읽는다.
  await vi.waitFor(() => expect(byTestId("run-picker")?.textContent).toContain("Dev (mini)"));
  expect(byTestId("run-target")?.querySelector("[data-machine]")?.textContent).toBe("mini");
  await click(byTestId("run-start"));
  const dialog = await vi.waitFor(() => {
    const d = byTestId("run-confirm-dialog");
    expect(d).not.toBeNull();
    return d!;
  });
  // 첫 실행 확인(7001)의 문장에 대상 머신 이름.
  expect(dialog.querySelector("[data-testid=target-machine]")?.textContent).toContain("mini");
  await click([...dialog.querySelectorAll("button")].find((b) => b.textContent === "실행"));
  // 그 머신 연결 · 그 머신 프로젝트로 — 이 머신에는 보내지 않았다.
  expect(calls(MINI, "run.start").map((c) => c.params)).toEqual([
    expect.objectContaining({ projectId: "r1", configId: "dev", confirmed: null }),
    expect.objectContaining({ projectId: "r1", configId: "dev", confirmed: CONFIRM.hash }),
  ]);
  expect(calls("local", "run.start")).toHaveLength(0);
  await vi.waitFor(() => expect(runTabs()).toHaveLength(1));
  expect(runTabs()[0]!.querySelector("[data-machine]")?.textContent).toBe("mini");
  // 디버그는 활성 체크아웃에서만 — 대상이 다른 머신이면 꺼진다.
  expect((byTestId("run-debug") as HTMLButtonElement).disabled).toBe(true);

  // 활성 체크아웃을 바꿔도 실행 탭은 남고, 중지는 그 머신으로 간다(이 머신에도 같은 번호의 실행이 있다).
  fake.runs.get("local")!.push(RUN("local", "p1", null));
  await act(async () => projectOpenInbox.push({ worktree: REMOTE_MAIN }));
  await settle();
  await act(async () => projectOpenInbox.push({ mainCheckout: true }));
  await settle();
  const remoteTab = windowRuns()
    .store.getState()
    .tabs.find((t) => t.machine === MINI)!;
  expect(remoteTab).toBeDefined();
  await act(async () => windowRuns().stop(remoteTab.key));
  await settle();
  expect(calls(MINI, "run.stop").map((c) => c.params)).toEqual([{ runId: "run-1" }]);
  expect(calls("local", "run.stop")).toHaveLength(0);
});

it("shows a remote run's port as that machine's port and never opens it on this computer", async () => {
  fake.runs.get(MINI)!.push(RUN(MINI, "r1", null));
  await mount();
  const opened: string[] = [];
  await act(async () => projectOpenInbox.push({ worktree: REMOTE_MAIN }));
  await settle();
  await vi.waitFor(() => expect(windowRuns().store.getState().tabs).toHaveLength(1));
  const chip = await vi.waitFor(() => {
    const c = document.querySelector<HTMLElement>("[data-testid=status-run-port]");
    expect(c).not.toBeNull();
    return c!;
  });
  expect(chip.dataset.machine).toBe(MINI);
  expect(chip.title).toContain("mini");
  expect(chip.tagName).toBe("SPAN");
  const forward = document.querySelector<HTMLButtonElement>("[data-testid=run-port-forward]")!;
  expect(forward.disabled).toBe(true);
  const spy = vi.spyOn(windowRuns(), "openUrl").mockImplementation((url) => void opened.push(url));
  await click(chip);
  expect(opened).toEqual([]);
  spy.mockRestore();
});

it("refuses the checkout actions without a checkout or with a machine this window does not know", async () => {
  await mount();
  await vi.waitFor(() => expect(registry().get("terminal.newTabIn")).toBeDefined());
  await vi.waitFor(() => expect(registry().get("run.chooseIn")).toBeDefined());
  // 인자 없이는 돌지 않는다 — 키 · 메뉴로 부를 수 없다.
  expect(registry().run("terminal.newTabIn")).toBe(false);
  expect(registry().run("run.chooseIn")).toBe(false);
  expect(registry().needsArgs("terminal.newTabIn")).toBe(true);
  expect(registry().needsArgs("run.chooseIn")).toBe(true);
  // 창이 모르는 머신은 열지 않는다(그 머신 연결을 잡지 않는다).
  await act(
    async () => void registry().run("terminal.newTabIn", { checkout: { machine: "SHA256:evil" } }),
  );
  await settle();
  expect(windowTerminals().store.getState().tabs).toHaveLength(0);
  // 그 체크아웃에서 열면 그 머신 연결로 띄운다.
  await act(
    async () =>
      void registry().run("terminal.newTabIn", {
        checkout: { machine: MINI, worktree: "/srv/wt" },
      }),
  );
  await settle();
  await vi.waitFor(() => expect(calls(MINI, "terminal.create")).toHaveLength(1));
  expect(calls(MINI, "terminal.create")[0]!.params).toMatchObject({
    projectId: "r1",
    worktree: "/srv/wt",
  });
  await act(async () => void registry().run("run.chooseIn", { checkout: { machine: MINI } }));
  await settle();
  expect(runTarget().getState().chosen).toEqual({ machine: MINI });
});

it("keeps a remote run's console through a resuming link and picks it up with `since` after the link really closed", async () => {
  fake.runs.get(MINI)!.push(RUN(MINI, "r1", null));
  await mount();
  await act(async () => projectOpenInbox.push({ worktree: REMOTE_MAIN }));
  await settle();
  await vi.waitFor(() => expect(windowRuns().store.getState().tabs).toHaveLength(1));
  await act(async () => void registry().run("toolWindow.run"));
  await settle();
  await vi.waitFor(() => expect(calls(MINI, "run.attach")).toHaveLength(1));
  const key = windowRuns().store.getState().tabs[0]!.key;
  const text = () => {
    const term = windowRuns().console(key)!.term;
    const lines: string[] = [];
    for (let y = 0; y < term.buffer.active.length; y++)
      lines.push(term.buffer.active.getLine(y)!.translateToString(true));
    return lines.join("\n");
  };
  await runOut(MINI, "alpha\r\n");
  // 이어 붙는 동안은 같은 스트림이 산다 — 다시 붙지 않는다.
  await link({ state: "resuming", machine: MINI });
  await runOut(MINI, "beta\r\n");
  await link({ state: "connected", machine: MINI });
  expect(calls(MINI, "run.attach")).toHaveLength(1);
  await vi.waitFor(() => expect(text()).toContain("beta"));
  // 링크가 정말 닫히면 스트림이 끊기고, 다시 붙으면 받은 끝부터 잇는다(실패 줄 없이).
  await link({
    state: "reconnecting",
    attempt: 1,
    retryInMs: 1000,
    error: { stage: "handshake", kind: "link", message: "the link closed" },
  });
  await runOut(MINI, "gamma\r\n");
  await link({ state: "connected", machine: MINI });
  await vi.waitFor(() => expect(calls(MINI, "run.attach")).toHaveLength(2));
  expect(calls(MINI, "run.attach")[1]!.params).toEqual({ runId: "run-1", since: 13 });
  await vi.waitFor(() => expect(text()).toContain("gamma"));
  expect(text().match(/alpha/g)).toHaveLength(1);
  expect(text()).not.toContain("[");
});

// 리뷰 B1: 디버그는 활성 체크아웃 연결로 열린다 — 대상이 다른 머신이면 ⌃D · 메뉴 · 액션 찾기(`debug.current`) · 🐞 모두 시작하지
// 않고 까닭을 알린다(그 머신의 구성 id 로 이 머신에서 디버그로 돌지 않게). 대상이 활성 체크아웃이면 시작한다(대조).
it("never starts a debug session on this machine for another machine's configuration", async () => {
  await mount();
  await choose("run-target", REMOTE_MAIN);
  await vi.waitFor(() => expect(byTestId("run-picker")?.textContent).toContain("Dev (mini)"));
  expect((byTestId("run-debug") as HTMLButtonElement).disabled).toBe(true);
  await click(byTestId("run-debug"));
  await act(async () => void registry().run("debug.current"));
  await settle();
  expect(calls("local", "debug.start")).toHaveLength(0);
  expect(calls(MINI, "debug.start")).toHaveLength(0);
  await vi.waitFor(() =>
    expect(document.body.textContent).toContain("디버그는 활성 체크아웃에서만"),
  );

  await choose("run-target", "active");
  await vi.waitFor(() => expect(byTestId("run-picker")?.textContent).toContain("Dev (local)"));
  expect((byTestId("run-debug") as HTMLButtonElement).disabled).toBe(false);
  await act(async () => void registry().run("debug.current"));
  await settle();
  await vi.waitFor(() => expect(calls("local", "debug.start")).toHaveLength(1));
});

// 리뷰 B2: 다시 실행의 첫 실행 확인(7001)은 그 탭의 실행이 도는 머신을 문장에 넣는다 — 실행 선택기의 대상(이 머신)이 아니라.
it("names the tab's machine in the first-run confirmation of a restart", async () => {
  fake.runs.get(MINI)!.push(RUN(MINI, "r1", null));
  await mount();
  await act(async () => projectOpenInbox.push({ worktree: REMOTE_MAIN }));
  await settle();
  await vi.waitFor(() => expect(windowRuns().store.getState().tabs).toHaveLength(1));
  await act(async () => projectOpenInbox.push({ mainCheckout: true }));
  await settle();
  const key = windowRuns().store.getState().tabs[0]!.key;
  fake.confirm = CONFIRM;
  let restarting!: Promise<void>;
  await act(async () => {
    restarting = windowRuns().restart(key);
  });
  await settle();
  const dialog = await vi.waitFor(() => {
    const d = byTestId("run-confirm-dialog");
    expect(d).not.toBeNull();
    return d!;
  });
  expect(dialog.querySelector("[data-testid=target-machine]")?.textContent).toContain("mini");
  await click([...dialog.querySelectorAll("button")].find((b) => b.textContent === "실행"));
  await act(async () => restarting);
  expect(calls(MINI, "run.restart").map((c) => c.params.confirmed)).toEqual([null, CONFIRM.hash]);
  expect(calls("local", "run.restart")).toHaveLength(0);
});

// 리뷰 F1: 링크가 이어 붙는 동안 그 머신의 실행 콘솔도 읽기 전용이다(입력을 버린다, 탭 머리에 글로).
it("makes a remote run console read-only while its link resumes", async () => {
  fake.runs.get(MINI)!.push(RUN(MINI, "r1", null));
  await mount();
  await act(async () => projectOpenInbox.push({ worktree: REMOTE_MAIN }));
  await settle();
  await vi.waitFor(() => expect(windowRuns().store.getState().tabs).toHaveLength(1));
  await act(async () => void registry().run("toolWindow.run"));
  await settle();
  await vi.waitFor(() => expect(calls(MINI, "run.attach")).toHaveLength(1));
  const key = windowRuns().store.getState().tabs[0]!.key;
  const term = windowRuns().console(key)!.term;
  await act(async () => term.input("x"));
  await settle();
  await vi.waitFor(() => expect(calls(MINI, "run.input")).toHaveLength(1));
  await link({ state: "resuming", machine: MINI });
  await vi.waitFor(() => expect(byTestId("run-tab-read-only")?.textContent).toBe("읽기 전용"));
  await act(async () => term.input("y"));
  await settle();
  expect(calls(MINI, "run.input")).toHaveLength(1);
  await link({ state: "connected", machine: MINI });
  expect(byTestId("run-tab-read-only")).toBeNull();
  await act(async () => term.input("z"));
  await vi.waitFor(() => expect(calls(MINI, "run.input")).toHaveLength(2));
});

// 리뷰 F3: 끊긴 셸에 [끝내기] 를 고르면 그 머신이 다시 붙을 때 끝낸다 — 다시 연결돼도 탭으로 되살아나지 않는다.
it("ends a disconnected remote shell once its machine reconnects instead of reopening it", async () => {
  await mount();
  const id = await openMiniShell();
  await link({
    state: "reconnecting",
    attempt: 1,
    retryInMs: 1000,
    error: { stage: "handshake", kind: "link", message: "the link closed" },
  });
  await vi.waitFor(() => expect(windowTerminals().instance(id)!.state.kind).toBe("disconnected"));
  // 탭을 닫으면 그 머신을 놓는다 — 다른 쓰는 것(활성 체크아웃)이 그 머신을 쥐게 둔다.
  await act(async () => projectOpenInbox.push({ worktree: REMOTE_MAIN }));
  await settle();
  await click(
    terminalTabs()
      .find((t) => t.dataset.machineTab === MINI)!
      .querySelector<HTMLElement>("button[aria-label$='닫기']"),
  );
  await click(byTestId("remote-close-dialog")!.querySelector("[data-answer=end]"));
  expect(windowTerminals().instance(id)).toBeUndefined();
  expect(fake.shells.get(MINI)!.has(1)).toBe(true);
  await link({ state: "connected", machine: MINI });
  await vi.waitFor(() =>
    expect(calls(MINI, "terminal.close").map((c) => c.params)).toEqual([{ terminalId: 1 }]),
  );
  await settle();
  expect(fake.shells.get(MINI)!.has(1)).toBe(false);
  expect(
    windowTerminals()
      .store.getState()
      .tabs.some((t) => t.machine === MINI),
  ).toBe(false);
});

// 리뷰 F4: 다른 머신에서 실행만 돌아도 창 닫기에 [두고 닫기] 가 있다 — 실행은 그 머신에서 그대로 돈다.
it("offers Keep and Close when only a remote run is left", async () => {
  fake.runs.get(MINI)!.push(RUN(MINI, "r1", null));
  await mount();
  await act(async () => projectOpenInbox.push({ worktree: REMOTE_MAIN }));
  await settle();
  await vi.waitFor(() => expect(windowRuns().store.getState().tabs).toHaveLength(1));
  let closing!: Promise<boolean>;
  await act(async () => {
    closing = closeGuard!();
  });
  await settle();
  const dialog = await vi.waitFor(() => {
    const d = byTestId("remote-close-dialog");
    expect(d).not.toBeNull();
    return d!;
  });
  expect(dialog.textContent).toContain("실행 1개");
  await click(dialog.querySelector("[data-answer=keep]"));
  expect(await closing).toBe(true);
  expect(calls(MINI, "run.stop")).toHaveLength(0);
});
