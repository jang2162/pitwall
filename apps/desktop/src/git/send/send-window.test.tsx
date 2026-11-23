// @vitest-environment happy-dom
// 커밋 보내기 · 가져오기 화면(18-m7-plan M7-17 완료 기준 — 웹뷰 시험, StrictMode 는 test-support/strict-mode.ts):
// ① 마지막 대상이 먼저 골라진다 ② 멈춤에 git 원문이 보이고 대상이 바뀌지 않았다고 적는다 ③ [되돌리기] 는 그사이 대상 ref 가 바뀌었으면
// 하지 않는다. 덧붙여 — 확인 창의 두 머신 이름이 실제 원본 · 대상과 같다, 쓰기는 대상 머신 연결로만 간다, 대상 연결에서도
// `git.prompts` 를 듣는다(M7-16 보고 5), 원격 활성 체크아웃으로 가져올 때 그 머신의 미저장 문서를 묻는다(메인 판정 2026-11-23).
//
// 가짜 Host 는 연결 대상마다 따로이고 실제 범위 규칙대로 거절한다(fake-send-host.ts — M7-11 리뷰 B1). 이 머신(`?project=p1`)과 다른
// 머신 `MINI` 의 멤버(`r1`)가 **같은 작업 트리 경로**(`/work/demo`)를 가진다 — 머신을 섞으면 경로가 같아 티가 나지 않는 자리를 잡는다.

import { EditorView } from "@codemirror/view";
import type { DockviewApi } from "dockview-react";
import { act as reactAct } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, vi } from "vitest";

import defaultTheme from "../../../../../extensions/theme-default/themes/default.json";
import { App } from "../../App";
import { checkoutKey, qualify } from "../../checkout/checkout";
import { resetWindowFileTree } from "../../file-tree/project-view";
import type { KeymapNative } from "../../keymap/native";
import { activeWorktreeStore } from "../../open-with/active-worktree";
import { projectOpenInbox } from "../../project-open";
import { act, endTest, it } from "../../test-support/ended-test";
import { createThemeStore } from "../../theme/theme-store";
import { ThemeStoreProvider } from "../../theme/theme-switcher";
import { parseTheme } from "../../theme/tokens";
import { commit, createGitHost, createRepo, type FakeGitHost } from "./fake-send-host";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const MINI = "SHA256:mini/fingerprint";
const FILE = "/work/demo/src/main.rs";

const fake = vi.hoisted(() => ({
  git: new Map<string, unknown>(),
  files: new Map<string, Map<string, string>>(),
  lastTarget: null as unknown,
  /** `bundle.setLastSendTarget` 의 파라미터. */
  setLast: [] as unknown[],
}));

vi.mock("../../tauri-transport", async () => {
  const { createMemoryTransportPair: pair } = await import("@pitwall/rpc-client");
  const { DEFAULT_KEYMAP_TOML } = await import("../../keymap/default-keymap");
  const { i18nState } = await import("../../i18n/testing");
  const MINI_ID = "SHA256:mini/fingerprint";
  return {
    connectHost: async (target: Record<string, unknown>) => {
      const member =
        "member" in target ? (target.member as { host: string; project: string }) : undefined;
      const machine = member ? member.host : "local";
      const scope =
        "hostId" in target
          ? ({ kind: "host" } as const)
          : ({
              kind: "project",
              projectId: member ? member.project : String(target.projectId),
            } as const);
      const [client, server] = pair();
      let next = 1;
      const send = (m: object) => server.send(JSON.stringify({ jsonrpc: "2.0", ...m }));
      const conn = { scope, send };
      const git = () => fake.git.get(machine) as FakeGitHost;
      server.onMessage((text) => {
        const m = JSON.parse(text) as { id?: number; method: string; params: never };
        const reply = (result: unknown) => send({ id: m.id, result });
        const params = (m.params ?? {}) as Record<string, unknown>;
        const files = fake.files.get(machine)!;
        // Host 의 범위 규칙: `remote.*` 는 Host 범위 연결만 받는다(pw-remote `host_scope`).
        if (m.method.startsWith("remote.") && scope.kind !== "host")
          return send({
            id: m.id,
            error: { code: -32001, message: "remote.* takes only a host connection", data: null },
          });
        if (m.method === "stream.cancel") {
          git().cancel(conn, (m.params as { streamId: number }).streamId);
          return;
        }
        if (m.method === "stream.credit") return;
        if (git().handle(conn, m.id, m.method, params, () => next++)) return;
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
                  { machine: "local", projectId: "p1", lastTarget: fake.lastTarget },
                  { machine: MINI_ID, projectId: "r1", lastTarget: null },
                ],
              },
            ]);
          case "bundle.setLastSendTarget":
            if (machine !== "local") break;
            fake.setLast.push(params);
            return reply(null);
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
            reply({ streamId });
            return send({
              method: "stream.chunk",
              params: { streamId, data: { state: "connected", machine: MINI_ID } },
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
      return {
        transport: client,
        close: async () => git().closed(conn),
      };
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
let local: FakeGitHost;
let mini: FakeGitHost;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
  );
  vi.spyOn(console, "info").mockImplementation(() => {});
  vi.spyOn(console, "warn").mockImplementation(() => {});
  window.history.replaceState(null, "", "/?window=project&project=p1");
  local = createGitHost(createRepo("local", "p1", "/work/demo"));
  mini = createGitHost(createRepo(MINI, "r1", "/work/demo"));
  local.repo.branches.set("feat", "c0");
  commit(local.repo, "feat", "c1");
  local.repo.head = "feat";
  fake.git = new Map<string, unknown>([
    ["local", local],
    [MINI, mini],
  ]);
  fake.files = new Map([
    ["local", new Map([[FILE, "local text"]])],
    [MINI, new Map([[FILE, "remote text"]])],
  ]);
  fake.lastTarget = null;
  fake.setLast = [];
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

async function push(open: Parameters<typeof projectOpenInbox.push>[0]) {
  await act(async () => projectOpenInbox.push(open));
  await settle();
}

async function runAction(id: string) {
  await act(async () => menuListener?.(id));
  await settle();
}

const items = () => [...document.querySelectorAll<HTMLElement>("[data-checkout]")];
const itemFor = (machine: string, worktree?: string) =>
  items().find((e) => e.dataset.checkout === `checkout:${machine}:${worktree ?? ""}`);
const confirm = () => document.querySelector<HTMLElement>("[data-testid=git-send-confirm]");
const toasts = () => [...document.querySelectorAll<HTMLElement>("[data-sonner-toast]")];
const toastWith = (text: string) => toasts().find((t) => t.textContent?.includes(text));
const sendCalls = (host: FakeGitHost) =>
  host.repo.calls.filter((c) => c.method.startsWith("git.send.") || c.method === "git.reset");

/** 고르기 → 확인 → 보내기. */
async function sendTo(machine: string, worktree?: string) {
  await runAction("git.send.open");
  const item = await vi.waitFor(() => {
    const found = itemFor(machine, worktree);
    expect(found).toBeDefined();
    return found!;
  });
  await act(async () => item.click());
  await settle();
  const go = await vi.waitFor(() => {
    const button = confirm()?.querySelector<HTMLButtonElement>("[data-send-go]");
    expect(button?.disabled).toBe(false);
    return button!;
  });
  await act(async () => go.click());
  await settle();
}

it("preselects the source checkout's last target in the picker", async () => {
  fake.lastTarget = { machine: MINI, worktree: "/work/demo-wt" };
  await mount();
  await runAction("git.send.open");
  const picked = await vi.waitFor(() => {
    const selected = items().find((e) => e.getAttribute("aria-selected") === "true");
    expect(selected).toBeDefined();
    return selected!;
  });
  expect(picked.dataset.checkout).toBe(`checkout:${MINI}:/work/demo-wt`);
  // 다른 머신의 메인 체크아웃도 고를 수 있다. 이 머신(원본)은 대상 목록에 없다.
  expect(itemFor(MINI)).toBeDefined();
  expect(itemFor("local")).toBeUndefined();
});

it("names both real machines in the confirmation and writes only through the target machine's connection", async () => {
  local.repo.dirty = ["notes.txt"];
  mini.repo.runs = [{ worktree: null, state: "running" }];
  await mount();
  await runAction("git.send.open");
  const item = await vi.waitFor(() => itemFor(MINI)!);
  await act(async () => item.click());
  await settle();
  const dialog = await vi.waitFor(() => {
    expect(confirm()?.querySelector("[data-send-go]")?.hasAttribute("disabled")).toBe(false);
    return confirm()!;
  });
  // 확인의 원본 · 대상 머신이 실제 그 머신이고, 이름도 그 머신의 별명이다.
  const from = dialog.querySelector<HTMLElement>("[data-send-side=원본]")!;
  const to = dialog.querySelector<HTMLElement>("[data-send-side=대상]")!;
  expect(from.dataset.machine).toBe("local");
  expect(from.textContent).toContain("이 컴퓨터");
  expect(to.dataset.machine).toBe(MINI);
  expect(to.textContent).toContain("mini");
  expect(dialog.querySelector("h2")?.textContent).toContain("feat");
  // 원본의 커밋 안 한 변경 · 대상에서 도는 실행을 알린다(막지 않는다).
  expect(dialog.querySelector("[data-send-note=uncommitted]")).not.toBeNull();
  expect(dialog.querySelector("[data-send-note=busy]")?.textContent).toContain("1");

  await act(async () => dialog.querySelector<HTMLButtonElement>("[data-send-go]")!.click());
  await vi.waitFor(() => expect(mini.repo.branches.get("feat")).toBe("c1"));
  expect(mini.repo.head).toBe("feat");
  // 원본(이 머신)에는 읽기(prepare)만, 받기 · 맞추기는 대상 머신의 그 프로젝트(`r1`)로만.
  expect(sendCalls(local).map((c) => c.method)).toEqual(["git.send.prepare"]);
  const writes = sendCalls(mini).filter((c) => c.method !== "git.send.theirs");
  const methods = writes.map((c) => c.method);
  expect(methods[0]).toBe("git.send.receive.begin");
  expect(methods.slice(-2)).toEqual(["git.send.receive.finish", "git.send.apply"]);
  expect(new Set(methods.slice(1, -2))).toEqual(new Set(["git.send.receive.chunk"]));
  expect(
    writes.filter((c) => "projectId" in c.params).every((c) => c.params.projectId === "r1"),
  ).toBe(true);
  // 마지막 대상을 기억한다(원본 = 이 머신).
  expect(fake.setLast).toEqual([
    { bundleId: "b-1", source: "local", target: { machine: MINI, worktree: null } },
  ]);
  const done = await vi.waitFor(() => {
    const found = toastWith("mini 에 만들고 체크아웃했다");
    expect(found).toBeDefined();
    return found!;
  });
  // 만든 브랜치는 움직인 ref 가 없다 — [되돌리기] 가 없다.
  expect(done.textContent).not.toContain("되돌리기");
});

it("stops on a diverged target, shows git's text and says the target is unchanged", async () => {
  commit(mini.repo, "feat", "x9");
  await mount();
  await sendTo(MINI);
  const toast = await vi.waitFor(() => {
    const found = toastWith("대상은 바뀌지 않았다");
    expect(found).toBeDefined();
    return found!;
  });
  expect(toast.textContent).toContain("fatal: Not possible to fast-forward, aborting.");
  expect(toast.textContent).toContain("refs/pitwall/incoming/local/feat");
  expect(mini.repo.branches.get("feat")).toBe("x9");
  // 멈춘 보내기에는 [되돌리기] 가 없다.
  expect(toast.textContent).not.toContain("되돌리기");
});

it("undoes a fast-forward on the target only while its ref has not moved since", async () => {
  // 대상에 feat 가 c0 에 있다 — 보내면 c0 → c1 로 올라간다(되돌릴 수 있는 이동).
  mini.repo.branches.set("feat", "c0");
  await mount();
  await sendTo(MINI);
  const undoButton = async () =>
    vi.waitFor(() => {
      const button = [
        ...document.querySelectorAll<HTMLButtonElement>("[data-sonner-toast] button"),
      ].find((b) => b.textContent === "되돌리기");
      expect(button).toBeDefined();
      return button!;
    });
  const button = await undoButton();
  expect(mini.repo.branches.get("feat")).toBe("c1");
  // 그사이 대상 머신에서 누가 커밋을 더했다.
  commit(mini.repo, "feat", "c7");
  await act(async () => button.click());
  await settle();
  await vi.waitFor(() =>
    expect(mini.repo.calls.filter((c) => c.method === "git.reset")).toHaveLength(1),
  );
  const reset = mini.repo.calls.find((c) => c.method === "git.reset")!;
  expect(reset.params).toMatchObject({
    projectId: "r1",
    commit: "c0",
    expected: "c1",
    expectedRef: "refs/heads/feat",
  });
  // Host 가 거절했다 — 대상은 그대로다.
  expect(mini.repo.branches.get("feat")).toBe("c7");
  expect(local.repo.calls.some((c) => c.method === "git.reset")).toBe(false);
});

it("undoes an untouched fast-forward on the target machine", async () => {
  mini.repo.branches.set("feat", "c0");
  await mount();
  await sendTo(MINI);
  const button = await vi.waitFor(() => {
    const found = [
      ...document.querySelectorAll<HTMLButtonElement>("[data-sonner-toast] button"),
    ].find((b) => b.textContent === "되돌리기");
    expect(found).toBeDefined();
    return found!;
  });
  await act(async () => button.click());
  await vi.waitFor(() => expect(mini.repo.branches.get("feat")).toBe("c0"));
  expect(local.repo.branches.get("feat")).toBe("c1");
});

it("asks about the remote checkout's unsaved document when receiving into it", async () => {
  // 활성 체크아웃은 그 머신의 메인 체크아웃이고, 그 머신의 파일을 고쳐 두었다(저장하지 않음).
  await mount();
  await push({ worktree: checkoutKey({ machine: MINI }), file: qualify(MINI, FILE) });
  const view = await vi.waitFor(() => {
    const found = [...document.querySelectorAll<HTMLElement>(".cm-editor")]
      .map((e) => EditorView.findFromDOM(e)!)
      .find((v) => v.state.doc.toString().includes("remote text"));
    expect(found).toBeDefined();
    return found!;
  });
  await act(async () =>
    view.dispatch({ changes: { from: 0, insert: "edited " }, userEvent: "input.type" }),
  );
  await settle();
  // 이 머신의 feat 를 가져온다 — 대상은 활성(원격) 체크아웃이다.
  await runAction("git.receive.open");
  const item = await vi.waitFor(() => itemFor("local")!);
  await act(async () => item.click());
  await settle();
  const go = await vi.waitFor(() => {
    const b = confirm()?.querySelector<HTMLButtonElement>("[data-send-go]");
    expect(b?.disabled).toBe(false);
    return b!;
  });
  expect(confirm()!.querySelector<HTMLElement>("[data-send-side=대상]")!.dataset.machine).toBe(
    MINI,
  );
  await act(async () => go.click());
  // 맞추기가 바꿀 `src/main.rs` 는 그 머신의 미저장 문서다 — 묻는다.
  await vi.waitFor(() =>
    expect(document.querySelector("[data-testid=unsaved-dialog]")).not.toBeNull(),
  );
  expect(mini.repo.branches.has("feat")).toBe(false);
  void dock;
});
