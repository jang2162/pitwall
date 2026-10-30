// @vitest-environment happy-dom
// 프로젝트 창(App kind="project")의 원격 · 위험 작업 다이얼로그(13-m2-plan.md M2-20 완료 기준)를 가짜 Host 에 붙여 본다:
// ⌘⇧K 의 푸시 다이얼로그가 보내기 전에 `git.push.plan`(대상 · 터미널과의 차이)과 보낼 커밋을 보이고, 보낼 때 그 계획의 remote ·
// remoteRef 를 `expectedTarget` 으로 그대로 싣는다(대상을 입력으로 고정하지 않는다); `-32004` 면 계획을 다시 보인다; 원격이 앞서
// 있어 git 이 거절하면 그 원문과 [풀…] 을 보이고 [풀…] 이 풀 다이얼로그를 연다; force 는 경고 다이얼로그 뒤에만, force-with-lease 의
// 기대값은 계획의 lease 다; git 이 거절할 대상이면 그 까닭과 후보를 보이고 후보를 고르면 그것으로 다시 계획한다; 결과 알림이 실제
// 대상 · refspec · 터미널과의 차이를 보인다; ⌘T 의 풀 다이얼로그는 upstream 이면 원격을 넘기지 않는다; askpass 다이얼로그에 넣은
// 값은 `git.prompt.answer` 로만 가고 콘솔 · 알림 · 화면에 남지 않는다; ⌃V 에 푸시 · 업데이트 · fetch 가 있다(브랜치 팝업의 [되돌리기] 는 branch-window.test.tsx).

import type {
  ConfigView,
  FsEntry,
  FsStat,
  GitPushPlanResult,
  GitRefs,
  HostInfo,
  KeymapFile,
  Project,
  ProjectTrust,
  Resolution,
} from "@pitwall/rpc-client";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import defaultTheme from "../../../../../extensions/theme-default/themes/default.json";
import { App } from "../../App";
import { resetWindowFileTree } from "../../file-tree/project-view";
import { fakeTreeLayout } from "../../file-tree/testing";
import type { KeymapNative } from "../../keymap/native";
import { activeWorktreeStore } from "../../open-with/active-worktree";
import { createThemeStore, type ThemeStore } from "../../theme/theme-store";
import { ThemeStoreProvider } from "../../theme/theme-switcher";
import { parseTheme } from "../../theme/tokens";
import { resetWindowBranch } from "../branch-state";
import { resetWindowGitStatus } from "../git-status";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const MAIN = "abc1234567890abc1234567890abc1234567890a";
const SEEN = "5ee0000000000000000000000000000000000000";
const SECRET = "pw-ui-secret-7d21aa";

type Answer = unknown | ((params: Record<string, unknown>) => unknown);

const host = vi.hoisted(() => ({
  refs: undefined as unknown as GitRefs,
  calls: [] as { method: string; params: Record<string, unknown> }[],
  answers: new Map<string, unknown>(),
  /** `git.prompts` 스트림에 질문을 보낸다. 연결마다 바뀐다. */
  prompt: (_event: unknown) => {},
}));

function plan(over: Partial<GitPushPlanResult> = {}): GitPushPlanResult {
  return {
    workTree: "/work/demo",
    branch: "main",
    remote: "origin",
    remoteRef: "refs/heads/main",
    refspec: "refs/heads/main:refs/heads/main",
    target: "pushDefault",
    terminal: null,
    lease: { trackingRef: "refs/remotes/origin/main", commit: SEEN },
    refusal: null,
    ...over,
  };
}

function refs(): GitRefs {
  return {
    workTree: "/work/demo",
    head: { kind: "branch", name: "main", commit: MAIN },
    branches: [
      {
        name: "main",
        refName: "refs/heads/main",
        commit: MAIN,
        head: true,
        upstream: {
          name: "origin/main",
          refName: "refs/remotes/origin/main",
          track: { ahead: 2, behind: 1 },
        },
      },
    ],
    remoteBranches: [
      { remote: "origin", name: "origin/main", refName: "refs/remotes/origin/main", commit: SEEN },
      {
        remote: "origin",
        name: "origin/topic",
        refName: "refs/remotes/origin/topic",
        commit: SEEN,
      },
    ],
    tags: [],
    unsupported: null,
    broken: [],
  };
}

vi.mock("../../tauri-transport", async () => {
  const { createMemoryTransportPair: pair } = await import("@pitwall/rpc-client");
  const { DEFAULT_KEYMAP_TOML } = await import("../../keymap/default-keymap");
  const { i18nState } = await import("../../i18n/testing");
  return {
    connectHost: async () => {
      const [client, server] = pair();
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
        const params = (m.params ?? {}) as Record<string, unknown>;
        if (m.method.startsWith("git.")) host.calls.push({ method: m.method, params });
        const answer = host.answers.get(m.method) as Answer;
        if (answer !== undefined) {
          const value =
            typeof answer === "function"
              ? (answer as (p: Record<string, unknown>) => unknown)(params)
              : answer;
          if (value && typeof value === "object" && "error" in value)
            return send({ id: m.id, error: (value as { error: unknown }).error });
          return reply(value);
        }
        const path = (params.path as string | undefined) ?? "";
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
          case "project.list":
            return reply([project]);
          case "project.resolve":
            return reply({
              project,
              root: "/work/demo",
              path,
              worktree: { top: path, root: path },
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
          case "project.state.get":
            return reply({ state: null, problem: null });
          case "project.state.set":
            return reply(null);
          case "fs.list":
            return reply({ path, entries: [] as FsEntry[] });
          case "fs.stat":
            return reply({
              path,
              kind: "dir",
              target: null,
              size: 0,
              mtimeMs: 1,
              readonly: false,
              sizeClass: null,
            } satisfies FsStat);
          case "git.watch": {
            const streamId = next++;
            send({ id: m.id, result: { streamId } });
            return send({
              method: "stream.chunk",
              params: { streamId, data: { workTree: "/work/demo" } },
            });
          }
          case "git.prompts": {
            const streamId = next++;
            send({ id: m.id, result: { streamId } });
            host.prompt = (event) =>
              send({ method: "stream.chunk", params: { streamId, data: event } });
            return;
          }
          case "stream.cancel":
            return;
          case "git.status":
            return reply({ workTree: "/work/demo", entries: [], unsupported: null });
          case "git.refs":
            return reply(host.refs);
          case "git.operation":
            return reply({ operation: null });
          case "git.worktrees":
            return reply({ workTree: "/work/demo", worktrees: [] });
          case "events.subscribe":
            return reply({ subscriptionId: next++ });
          case "events.unsubscribe":
            return reply(null);
          default:
            return m.id === undefined
              ? undefined
              : send({ id: m.id, error: { code: -32601, message: `no ${m.method}` } });
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
  closeWindow: async () => {},
};

let root: Root | undefined;
let theme: ThemeStore | undefined;
let consoleCalls: unknown[][] = [];

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
  );
  consoleCalls = [];
  for (const level of ["log", "info", "warn", "error", "debug"] as const)
    vi.spyOn(console, level).mockImplementation((...args: unknown[]) => {
      consoleCalls.push(args);
    });
  fakeTreeLayout(600);
  window.history.replaceState(null, "", "/?window=project&project=p1");
  host.refs = refs();
  host.calls = [];
  host.answers = new Map<string, unknown>([
    ["git.push.plan", plan()],
    [
      "git.push.commits",
      {
        workTree: "/work/demo",
        commits: [
          { hash: MAIN, subject: "second change", author: "Pitwall Test", authorTime: 2 },
          { hash: "b".repeat(40), subject: "first change", author: "Pitwall Test", authorTime: 1 },
        ],
        truncated: false,
      },
    ],
    [
      "git.push",
      {
        workTree: "/work/demo",
        branch: "main",
        remote: "origin",
        remoteRef: "refs/heads/main",
        refspec: "refs/heads/main:refs/heads/main",
        terminal: null,
      },
    ],
  ]);
  activeWorktreeStore.setState({ top: undefined });
  resetWindowFileTree();
  resetWindowGitStatus();
  resetWindowBranch();
  localStorage.clear();
});

afterEach(() => {
  act(() => root?.unmount());
  root = undefined;
  theme?.dispose();
  theme = undefined;
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

const settle = (ms = 20) => act(async () => new Promise((r) => setTimeout(r, ms)));

async function mount() {
  const element = document.createElement("div");
  element.style.width = "1280px";
  element.style.height = "800px";
  document.body.append(element);
  theme = createThemeStore({
    system: { current: () => "light", subscribe: () => () => {} },
    storage: { read: () => undefined, write: () => {} },
    theme: parseTheme(defaultTheme),
    apply: () => {},
  });
  root = createRoot(element);
  await act(async () =>
    root!.render(
      <ThemeStoreProvider store={theme!}>
        <App kind="project" native={native} />
      </ThemeStoreProvider>,
    ),
  );
  await vi.waitFor(() =>
    expect(document.querySelector("[data-testid=status-branch]")).not.toBeNull(),
  );
  await vi.waitFor(() => expect(calls("git.refs").length).toBeGreaterThan(0));
  await settle(50);
}

async function press(init: KeyboardEventInit, target?: Element | null) {
  await act(async () => {
    (target ?? document.activeElement ?? document.body).dispatchEvent(
      new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }),
    );
    document.body.dispatchEvent(new KeyboardEvent("keyup", { bubbles: true, ...init }));
  });
  await settle();
}

async function click(element: Element) {
  await act(async () => (element as HTMLElement).click());
  await settle();
}

const calls = (method: string) => host.calls.filter((c) => c.method === method);
const byTestId = (id: string) => document.querySelector<HTMLElement>(`[data-testid=${id}]`);
const button = (scope: Element, text: string) =>
  [...scope.querySelectorAll<HTMLButtonElement>("button")].find((b) => b.textContent === text);

/** ⌘⇧K — 한글 입력 상태여도 물리 키(KeyK)로 맞춘다. */
const pushKey = () => press({ code: "KeyK", key: "ㅏ", metaKey: true, shiftKey: true });

async function openPush() {
  await pushKey();
  await vi.waitFor(() => expect(byTestId("git-push-target")).not.toBeNull());
  await settle();
  return byTestId("git-push-dialog")!;
}

it("shows the plan and the commits before sending, and sends the plan's target as expectedTarget", async () => {
  host.answers.set(
    "git.push.plan",
    plan({
      terminal: {
        configured: [],
        pushDefault: "upstream",
        sends: ["refs/heads/main:refs/heads/trunk"],
        truncated: false,
        refusal: null,
      },
    }),
  );
  host.answers.set("git.push", {
    workTree: "/work/demo",
    branch: "main",
    remote: "origin",
    remoteRef: "refs/heads/main",
    refspec: "refs/heads/main:refs/heads/main",
    terminal: {
      configured: [],
      pushDefault: "upstream",
      sends: ["refs/heads/main:refs/heads/trunk"],
      truncated: false,
      refusal: null,
    },
  });
  await mount();
  const dialog = await openPush();
  expect(calls("git.push.plan")[0]!.params).toEqual({ projectId: "p1", worktree: null });
  expect(byTestId("git-push-target")!.textContent).toBe("main → origin : refs/heads/main");
  expect(byTestId("git-push-terminal")!.textContent).toContain("push.default = upstream");
  expect(byTestId("git-push-terminal")!.textContent).toContain("refs/heads/main:refs/heads/trunk");
  // 보낼 커밋은 원격에 있다고 아는 마지막 커밋(lease)부터.
  expect(calls("git.push.commits")[0]!.params).toEqual({
    projectId: "p1",
    worktree: null,
    branch: "main",
    remote: "origin",
    base: SEEN,
    limit: 100,
  });
  expect(byTestId("git-push-commits")!.textContent).toContain("second change");
  expect(dialog.textContent).toContain("보낼 커밋 2개");
  // upstream 이 있으면 「upstream 으로 정하기」 는 꺼진 채, force 는 꺼진 채다.
  const boxes = [...dialog.querySelectorAll<HTMLInputElement>("input[type=checkbox]")];
  expect(boxes.map((b) => b.checked)).toEqual([false, false]);
  expect(calls("git.push")).toHaveLength(0);

  await click(button(dialog, "푸시")!);
  await vi.waitFor(() => expect(calls("git.push")).toHaveLength(1));
  expect(calls("git.push")[0]!.params).toEqual({
    projectId: "p1",
    worktree: null,
    branch: "main",
    setUpstream: false,
    forceWithLease: null,
    expectedTarget: { remote: "origin", remoteRef: "refs/heads/main" },
  });
  await vi.waitFor(() => expect(byTestId("git-push-dialog")).toBeNull());
  // 결과 알림: 실제 대상 · 보낸 refspec · 터미널과의 차이.
  await vi.waitFor(() =>
    expect(document.body.textContent).toContain("main → origin : refs/heads/main 푸시함"),
  );
  expect(document.body.textContent).toContain("보낸 것: refs/heads/main:refs/heads/main");
  expect(document.body.textContent).toContain("터미널의 git push 는 다르게 한다");
});

it("sends the planned branch even when HEAD moved to another branch after the plan", async () => {
  await mount();
  const dialog = await openPush();
  // 터미널에서 다른 브랜치로 체크아웃했다 — 계획(main)은 그대로 보인다.
  host.refs = {
    ...refs(),
    head: { kind: "branch", name: "other", commit: SEEN },
    branches: [
      { ...refs().branches[0]!, head: false },
      { name: "other", refName: "refs/heads/other", commit: SEEN, head: true, upstream: null },
    ],
  };
  await click(button(dialog, "푸시")!);
  await vi.waitFor(() => expect(calls("git.push")).toHaveLength(1));
  // Host 는 그 브랜치를 지금 브랜치인 것처럼 셈해 계획한 대상으로 보낸다(crates/pw-git/src/remote/tests/dialog.rs).
  expect(calls("git.push")[0]!.params).toMatchObject({
    branch: "main",
    expectedTarget: { remote: "origin", remoteRef: "refs/heads/main" },
  });
});

it("shows the plan again when the target changed after it was shown (-32004)", async () => {
  let n = 0;
  host.answers.set("git.push.plan", () =>
    ++n === 1
      ? plan()
      : plan({ remoteRef: "refs/heads/trunk", refspec: "refs/heads/main:refs/heads/trunk" }),
  );
  host.answers.set("git.push", {
    error: {
      code: -32004,
      message: "the push target changed",
      data: { remote: "origin", remoteRef: "refs/heads/trunk" },
    },
  });
  await mount();
  const dialog = await openPush();
  await click(button(dialog, "푸시")!);
  await vi.waitFor(() => expect(calls("git.push.plan")).toHaveLength(2));
  await vi.waitFor(() =>
    expect(byTestId("git-push-target")!.textContent).toBe("main → origin : refs/heads/trunk"),
  );
  expect(byTestId("git-push-notice")!.textContent).toContain("보낼 곳이 바뀌었다");
  expect(calls("git.push")).toHaveLength(1);
  expect(byTestId("git-push-dialog")).not.toBeNull();
});

it("shows git's rejection when the remote is ahead and offers a pull", async () => {
  const rejection =
    " ! [rejected]        main -> main (fetch first)\nerror: failed to push some refs to 'file:///remote.git'";
  host.answers.set("git.push", {
    error: {
      code: 2004,
      message: "git push failed",
      data: { command: "push", exitCode: 1, stderr: rejection, stdout: "" },
    },
  });
  await mount();
  const dialog = await openPush();
  await click(button(dialog, "푸시")!);
  await vi.waitFor(() => expect(byTestId("git-push-rejected")).not.toBeNull());
  expect(byTestId("git-push-rejected")!.textContent).toContain(
    "! [rejected]        main -> main (fetch first)",
  );
  await click(button(byTestId("git-push-dialog")!, "풀…")!);
  await vi.waitFor(() => expect(byTestId("git-pull-dialog")).not.toBeNull());
  expect(byTestId("git-push-dialog")).toBeNull();
});

it("force pushes only after the warning dialog, with the plan's lease as the expected remote hash", async () => {
  await mount();
  const dialog = await openPush();
  const force = [...dialog.querySelectorAll<HTMLInputElement>("input[type=checkbox]")][1]!;
  await click(force);
  await click(button(dialog, "강제 푸시")!);
  const warning = await vi.waitFor(() => {
    const el = byTestId("git-confirm-dialog");
    if (!el) throw new Error("no warning");
    return el;
  });
  expect(warning.getAttribute("data-danger")).toBe("true");
  expect(warning.textContent).toContain("5ee00000");
  expect(calls("git.push")).toHaveLength(0);
  await click(button(warning, "취소")!);
  await settle(50);
  expect(calls("git.push")).toHaveLength(0);

  await click(button(byTestId("git-push-dialog")!, "강제 푸시")!);
  const again = await vi.waitFor(() => {
    const el = byTestId("git-confirm-dialog");
    if (!el) throw new Error("no warning");
    return el;
  });
  await click(button(again, "강제 푸시")!);
  await vi.waitFor(() => expect(calls("git.push")).toHaveLength(1));
  expect(calls("git.push")[0]!.params).toMatchObject({
    forceWithLease: SEEN,
    expectedTarget: { remote: "origin", remoteRef: "refs/heads/main" },
  });
});

it("cannot force when the remote has no such branch yet", async () => {
  host.answers.set(
    "git.push.plan",
    plan({ lease: { trackingRef: "refs/remotes/origin/main", commit: null } }),
  );
  await mount();
  const dialog = await openPush();
  const force = [...dialog.querySelectorAll<HTMLInputElement>("input[type=checkbox]")][1]!;
  expect(force.disabled).toBe(true);
});

it("explains a refusal, lists the candidates, and plans again with the one chosen", async () => {
  host.refs = {
    ...refs(),
    branches: refs().branches.map((b) => ({ ...b, upstream: null })),
  };
  host.answers.set("git.push.plan", (p: Record<string, unknown>) =>
    p.remoteRef
      ? plan({ remoteRef: p.remoteRef as string, target: "input", lease: null })
      : plan({
          remoteRef: null,
          refspec: null,
          target: null,
          lease: null,
          refusal: {
            reason: "noUpstream",
            message: "The current branch main has no upstream branch.",
            candidates: ["refs/heads/main"],
          },
        }),
  );
  await mount();
  await pushKey();
  const refusal = await vi.waitFor(() => {
    const el = byTestId("git-push-refusal");
    if (!el) throw new Error("no refusal");
    return el;
  });
  expect(refusal.textContent).toContain("이 브랜치에 upstream 이 없다.");
  expect(calls("git.push.commits")).toHaveLength(0);
  expect(button(byTestId("git-push-dialog")!, "푸시")!.disabled).toBe(true);
  await click(refusal.querySelector("[data-candidate='refs/heads/main']")!);
  await vi.waitFor(() =>
    expect(byTestId("git-push-target")!.textContent).toBe("main → origin : refs/heads/main"),
  );
  expect(calls("git.push.plan").at(-1)!.params).toEqual({
    projectId: "p1",
    worktree: null,
    remoteRef: "refs/heads/main",
  });
  // upstream 이 없으니 「upstream 으로 정하기」 가 켜져 있다.
  const dialog = byTestId("git-push-dialog")!;
  expect(dialog.querySelector<HTMLInputElement>("input[type=checkbox]")!.checked).toBe(true);
  await click(button(dialog, "푸시")!);
  await vi.waitFor(() => expect(calls("git.push")).toHaveLength(1));
  expect(calls("git.push")[0]!.params).toEqual({
    projectId: "p1",
    worktree: null,
    remoteRef: "refs/heads/main",
    branch: "main",
    setUpstream: true,
    forceWithLease: null,
    expectedTarget: { remote: "origin", remoteRef: "refs/heads/main" },
  });
});

it("opens the update dialog with ⌘T and pulls from the upstream without naming it", async () => {
  host.answers.set("git.pull", { workTree: "/work/demo", operation: null, stopped: false });
  await mount();
  await press({ code: "KeyT", key: "t", metaKey: true });
  const dialog = await vi.waitFor(() => {
    const el = byTestId("git-pull-dialog");
    if (!el) throw new Error("no pull dialog");
    return el;
  });
  expect(dialog.textContent).toContain("upstream: origin/main");
  await click(dialog.querySelector<HTMLInputElement>("input[value=rebase]")!);
  await click(button(dialog, "업데이트")!);
  await vi.waitFor(() => expect(calls("git.pull")).toHaveLength(1));
  expect(calls("git.pull")[0]!.params).toEqual({ projectId: "p1", worktree: null, mode: "rebase" });
});

it("pulls a chosen remote branch by remote and branch name", async () => {
  host.answers.set("git.pull", { workTree: "/work/demo", operation: null, stopped: true });
  await mount();
  await press({ code: "KeyT", key: "t", metaKey: true });
  const dialog = await vi.waitFor(() => {
    const el = byTestId("git-pull-dialog");
    if (!el) throw new Error("no pull dialog");
    return el;
  });
  const select = dialog.querySelector<HTMLSelectElement>("select")!;
  await act(async () => {
    select.value = "refs/remotes/origin/topic";
    select.dispatchEvent(new Event("change", { bubbles: true }));
  });
  await click(button(dialog, "업데이트")!);
  await vi.waitFor(() => expect(calls("git.pull")).toHaveLength(1));
  expect(calls("git.pull")[0]!.params).toEqual({
    projectId: "p1",
    worktree: null,
    mode: "merge",
    remote: "origin",
    branch: "topic",
  });
  await vi.waitFor(() => expect(document.body.textContent).toContain("충돌로 멈췄다"));
});

it("answers askpass through git.prompt.answer only — the value is not logged, notified or left on screen", async () => {
  await mount();
  await act(async () =>
    host.prompt({
      kind: "open",
      prompt: {
        promptId: 7,
        request: {
          kind: "askpass",
          text: "Password for 'https://x@example.invalid': ",
          hint: "secret",
        },
      },
    }),
  );
  const input = await vi.waitFor(() => {
    const el = document.querySelector<HTMLInputElement>("[data-testid=git-askpass-dialog] input");
    if (!el) throw new Error("no askpass dialog");
    return el;
  });
  expect(input.type).toBe("password");
  expect(input.autocomplete).toBe("off");
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, SECRET);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await click(document.querySelector("[data-testid=git-askpass-dialog] button[type=submit]")!);
  await vi.waitFor(() => expect(calls("git.prompt.answer")).toHaveLength(1));
  expect(calls("git.prompt.answer")[0]!.params).toEqual({
    projectId: "p1",
    promptId: 7,
    answer: { kind: "askpass", value: SECRET },
  });
  await settle(50);
  expect(byTestId("git-askpass-dialog")).toBeNull();
  expect(document.body.innerHTML).not.toContain(SECRET);
  expect(JSON.stringify(consoleCalls.map((args) => args.map(String)))).not.toContain(SECRET);
  // 다른 RPC 에도 실리지 않았다.
  for (const c of host.calls.filter((c) => c.method !== "git.prompt.answer"))
    expect(JSON.stringify(c.params)).not.toContain(SECRET);
});

it("focuses 아니오 first in an ssh yes/no askpass, so a stray Enter cannot accept an unknown host key", async () => {
  await mount();
  await act(async () =>
    host.prompt({
      kind: "open",
      prompt: {
        promptId: 8,
        request: {
          kind: "askpass",
          text: "Are you sure you want to continue connecting (yes/no)? ",
          hint: "confirm",
        },
      },
    }),
  );
  const dialog = await vi.waitFor(() => {
    const el = byTestId("git-askpass-dialog");
    if (!el) throw new Error("no askpass dialog");
    return el;
  });
  const no = button(dialog, "아니오");
  expect(no).toBeDefined();
  expect(document.activeElement).toBe(no);
  // 브라우저는 포커스된 단추에 Enter · Space 가 오면 그 단추를 누른다(happy-dom 은 그 기본 동작이 없어 직접 누른다).
  await click(document.activeElement as HTMLElement);
  await vi.waitFor(() => expect(calls("git.prompt.answer")).toHaveLength(1));
  expect(calls("git.prompt.answer")[0]!.params).toEqual({
    projectId: "p1",
    promptId: 8,
    answer: { kind: "cancel" },
  });
});

it("lists push, update and fetch in the VCS popup (⌃V) — not the args-only undo offer — and fetches every remote", async () => {
  host.answers.set("git.fetch", { workTree: "/work/demo" });
  await mount();
  await press({ code: "KeyV", key: "v", ctrlKey: true });
  const vcs = document.querySelector<HTMLElement>("[data-git-popup=vcs]")!;
  const items = [...vcs.querySelectorAll<HTMLElement>("[cmdk-item]")];
  const ids = items.map((i) => i.getAttribute("data-value"));
  expect(ids).toEqual(expect.arrayContaining(["git.push.open", "git.pull.open", "git.fetch"]));
  expect(ids).not.toContain("git.undo.offer");
  expect(
    items.find((i) => i.getAttribute("data-value") === "git.push.open")!.textContent,
  ).toContain("⇧⌘K");
  await click(items.find((i) => i.getAttribute("data-value") === "git.fetch")!);
  await vi.waitFor(() => expect(calls("git.fetch")).toHaveLength(1));
  expect(calls("git.fetch")[0]!.params).toEqual({ projectId: "p1", worktree: null });
  await vi.waitFor(() => expect(document.body.textContent).toContain("원격을 모두 가져왔다"));
});
