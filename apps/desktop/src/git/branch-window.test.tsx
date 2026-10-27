// @vitest-environment happy-dom
// 프로젝트 창(App kind="project")의 브랜치 기능(13-m2-plan M2-13 완료 기준)을 가짜 Host 에 붙여 본다: 상태바가 `git.refs` 의 ↑↓ 를 보이고
// 터미널의 체크아웃(`git.refsChanged`) 뒤 다시 한 번만 읽는다 — 감시(`git.watch`)는 하나다; 한글 입력 상태의 ⌃⇧B 가 브랜치 팝업을 열고
// 원격 브랜치를 고르면 추적 브랜치로 체크아웃한다; 병합 안 된 브랜치 삭제는 2101 뒤 「강제 삭제」 확인을 거친다; 즐겨찾기가 프로젝트
// 상태에 남는다; 워크트리를 고르면 M1-25 의 경로로 활성 워크트리가 바뀐다; ⇧⇧ 브랜치 탭에서 찾아 체크아웃한다; ⌃V 가 git 액션을 보인다.

import type {
  ConfigView,
  FsEntry,
  FsStat,
  GitRefs,
  GitWorktrees,
  HostInfo,
  KeymapFile,
  OperationInfo,
  Project,
  ProjectTrust,
  Resolution,
} from "@pitwall/rpc-client";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import { App } from "../App";
import { resetWindowFileTree } from "../file-tree/project-view";
import { fakeTreeLayout } from "../file-tree/testing";
import type { KeymapNative } from "../keymap/native";
import { activeWorktreeStore } from "../open-with/active-worktree";
import { createThemeStore, type ThemeStore } from "../theme/theme-store";
import { ThemeStoreProvider } from "../theme/theme-switcher";
import { parseTheme } from "../theme/tokens";
import { resetWindowBranch } from "./branch-state";
import { resetWindowGitStatus } from "./git-status";

declare global {
  var IS_REACT_ACT_ENVIRONMENT: boolean;
}

const MAIN = "abc1234567890abc1234567890abc1234567890a";
const FEATURE = "fea7000000000000000000000000000000000000";

const host = vi.hoisted(() => ({
  refs: undefined as unknown as GitRefs,
  operation: null as OperationInfo | null,
  worktrees: undefined as unknown as GitWorktrees,
  calls: [] as { method: string; params: Record<string, unknown> }[],
  watches: [] as { worktree: string | null }[],
  stateWrites: [] as unknown[],
  /** `project.state.get` 이 돌려줄 저장본. */
  savedState: null as unknown,
  emit: (_topic: string, _payload: unknown) => {},
}));

function refs(head: "main" | "feature"): GitRefs {
  return {
    workTree: "/work/demo",
    head: { kind: "branch", name: head, commit: head === "main" ? MAIN : FEATURE },
    branches: [
      {
        name: "feature",
        refName: "refs/heads/feature",
        commit: FEATURE,
        head: head === "feature",
        upstream: null,
      },
      {
        name: "main",
        refName: "refs/heads/main",
        commit: MAIN,
        head: head === "main",
        upstream: {
          name: "origin/main",
          refName: "refs/remotes/origin/main",
          track: { ahead: 1, behind: 2 },
        },
      },
    ],
    remoteBranches: [
      { remote: "origin", name: "origin/main", refName: "refs/remotes/origin/main", commit: MAIN },
      {
        remote: "origin",
        name: "origin/topic",
        refName: "refs/remotes/origin/topic",
        commit: FEATURE,
      },
    ],
    tags: [{ name: "v1.0", refName: "refs/tags/v1.0", target: MAIN, peeled: null }],
    unsupported: null,
    broken: [],
  };
}

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
        const params = (m.params ?? {}) as Record<string, unknown>;
        if (m.method.startsWith("git.") || m.method.startsWith("project.state"))
          host.calls.push({ method: m.method, params });
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
            return reply({ state: host.savedState, problem: null });
          case "project.state.set":
            host.stateWrites.push(params.state);
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
            host.watches.push({ worktree: (params.worktree as string | null) ?? null });
            send({ id: m.id, result: { streamId } });
            return send({
              method: "stream.chunk",
              params: { streamId, data: { workTree: host.refs.workTree } },
            });
          }
          case "stream.cancel":
            return;
          case "git.status":
            return reply({ workTree: host.refs.workTree, entries: [], unsupported: null });
          case "git.refs":
            return reply(host.refs);
          case "git.operation":
            return reply({ operation: host.operation });
          case "git.worktrees":
            return reply(host.worktrees);
          case "git.checkout": {
            const target = params.target as { kind: string; name?: string; branch?: string };
            const name = target.kind === "branch" ? target.name! : target.branch!;
            return reply({
              workTree: "/work/demo",
              head: { kind: "branch", name, commit: FEATURE },
            });
          }
          case "git.branch.delete":
            if (!params.force)
              return send({
                id: m.id,
                error: {
                  code: 2101,
                  message: "the branch is not fully merged",
                  data: {
                    stderr: "error: the branch 'feature' is not fully merged",
                    commit: FEATURE,
                  },
                },
              });
            return reply({
              workTree: "/work/demo",
              deleted: { refName: "refs/heads/feature", previous: FEATURE, current: null },
            });
          case "git.merge":
            return reply({
              workTree: "/work/demo",
              outcome: "done",
              head: MAIN,
              moved: { refName: "refs/heads/main", previous: MAIN, current: FEATURE },
              operation: null,
            });
          case "events.subscribe": {
            const id = next++;
            subs.set(id, m.params as { projectId?: string; topics: string[] });
            return reply({ subscriptionId: id });
          }
          case "events.unsubscribe":
            subs.delete((m.params as { subscriptionId: number }).subscriptionId);
            return reply(null);
          default:
            return m.id === undefined
              ? undefined
              : send({ id: m.id, error: { code: -32601, message: `no ${m.method}` } });
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

let root: Root | undefined;
let theme: ThemeStore | undefined;

beforeEach(() => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
  );
  vi.spyOn(console, "info").mockImplementation(() => {});
  fakeTreeLayout(600);
  window.history.replaceState(null, "", "/?window=project&project=p1");
  host.refs = refs("main");
  host.operation = null;
  host.worktrees = {
    workTree: "/work/demo",
    worktrees: [
      {
        path: "/work/demo",
        head: { kind: "branch", name: "main", commit: MAIN },
        main: true,
        current: true,
        locked: false,
        missing: false,
        switchTo: { worktree: null },
      },
      {
        path: "/work/wt",
        head: { kind: "branch", name: "feature", commit: FEATURE },
        main: false,
        current: false,
        locked: false,
        missing: false,
        switchTo: { worktree: "/work/wt" },
      },
    ],
  };
  host.calls = [];
  host.watches = [];
  host.stateWrites = [];
  host.savedState = null;
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
  await vi.waitFor(() => expect(status()).not.toBeNull());
  await settle();
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

const status = () => document.querySelector<HTMLElement>("[data-testid=status-branch]");
const popup = () => document.querySelector<HTMLElement>("[data-git-popup]");
const input = () => popup()!.querySelector<HTMLInputElement>("[cmdk-input]")!;
const items = () => [...popup()!.querySelectorAll<HTMLElement>("[cmdk-item]")];
const values = () => items().map((i) => i.getAttribute("data-value"));
const reads = (method: string) => host.calls.filter((c) => c.method === method);

async function type(value: string, into: HTMLInputElement = input()) {
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(into, value);
    into.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await settle();
}

async function click(element: Element) {
  await act(async () => (element as HTMLElement).click());
  await settle();
}

async function choose(value: string) {
  const item = items().find((i) => i.getAttribute("data-value") === value);
  expect(item, `${value} in ${JSON.stringify(values())}`).toBeDefined();
  await click(item!);
}

/** 한글 입력 상태의 ⌃⇧B — `key` 는 「ㅠ」, `code` 는 KeyB. */
const branchKey = () =>
  press({ code: "KeyB", key: "ㅠ", ctrlKey: true, shiftKey: true, isComposing: false });

it("shows the branch with the upstream's ahead/behind and follows a terminal checkout with one read on the one watch", async () => {
  await mount();
  expect(status()!.textContent).toBe("main↑1↓2");
  expect(status()!.title).toContain("/work/demo");
  expect(status()!.title).toContain("origin/main");
  // 상태바 왼쪽 맨 앞.
  const left = [...document.querySelectorAll("footer [data-status-item]")].map((e) =>
    e.getAttribute("data-status-item"),
  );
  expect(left[0]).toBe("branch");
  await vi.waitFor(() => expect(reads("git.refs").length).toBeGreaterThan(0));
  await settle(50);
  const before = reads("git.refs").length;

  // 터미널에서 `git checkout feature` — Host 감시가 낸 이벤트로 한 번 다시 읽는다.
  host.refs = refs("feature");
  await act(async () => host.emit("git.refsChanged", { worktree: null }));
  await vi.waitFor(() => expect(status()!.textContent).toBe("feature"));
  await settle(50);
  expect(reads("git.refs").length).toBe(before + 1);
  // 다른 워크트리의 이벤트는 읽지 않는다.
  await act(async () => host.emit("git.refsChanged", { worktree: "/work/wt" }));
  await settle(50);
  expect(reads("git.refs").length).toBe(before + 1);
  // 상태바 · 팝업 · SE 탭이 있어도 감시는 창의 git status 가 연 하나다.
  expect(host.watches).toEqual([{ worktree: null }]);
});

it("marks an operation in progress, a detached HEAD and an unsupported repository", async () => {
  host.operation = {
    kind: "rebase",
    heads: [],
    branch: "main",
    onto: FEATURE,
    step: { current: 2, total: 5 },
    remaining: 3,
    actions: ["continue", "abort", "skip"],
  };
  await mount();
  await vi.waitFor(() =>
    expect(document.querySelector("[data-testid=status-branch-operation]")?.textContent).toBe(
      "REBASING 2/5",
    ),
  );

  host.operation = null;
  host.refs = { ...refs("main"), head: { kind: "detached", commit: FEATURE } };
  host.refs.branches = host.refs.branches.map((b) => ({ ...b, head: false }));
  await act(async () => host.emit("git.refsChanged", { worktree: null }));
  await vi.waitFor(() => expect(status()!.textContent).toBe("fea7000"));

  host.refs = {
    ...refs("main"),
    branches: [],
    remoteBranches: [],
    tags: [],
    head: null,
    unsupported: "reftable",
  };
  await act(async () => host.emit("git.refsChanged", { worktree: null }));
  await vi.waitFor(() => expect(status()!.getAttribute("data-unsupported")).toBe("reftable"));
  expect(status()!.textContent).toBe("지원하지 않는 저장소");
});

it("opens the branch popup with ⌃⇧B under a Korean IME and checks a remote branch out as a tracking branch", async () => {
  await mount();
  await branchKey();
  expect(popup()?.getAttribute("data-git-popup")).toBe("branches");
  expect(values()).toEqual([
    "action:new",
    "local:refs/heads/feature",
    "local:refs/heads/main",
    "remote:refs/remotes/origin/main",
    "remote:refs/remotes/origin/topic",
    "tag:refs/tags/v1.0",
    "worktree:/work/demo",
    "worktree:/work/wt",
  ]);
  // 줄의 값(`remote:refs/…`)은 거르는 데 쓰지 않는다 — 「remote」 라는 이름의 ref 는 없다.
  await type("remote");
  expect(values()).toEqual([]);
  await type("topic");
  expect(values()).toEqual(["remote:refs/remotes/origin/topic"]);
  await choose("remote:refs/remotes/origin/topic");
  expect(popup()?.getAttribute("data-git-popup")).toBe("branch-menu");
  // 비교 액션(M2-15)이 없는 창에서는 「비교」 를 두지 않는다.
  expect(values()).toEqual([
    "menu:back",
    "menu:checkout",
    "menu:new",
    "menu:merge",
    "menu:rebase",
    "menu:favorite",
  ]);
  await choose("menu:checkout");
  await vi.waitFor(() => expect(reads("git.checkout")).toHaveLength(1));
  expect(reads("git.checkout")[0]!.params).toEqual({
    projectId: "p1",
    worktree: null,
    target: { kind: "remote", remote: "origin", branch: "topic" },
  });
  expect(popup()).toBeNull();
});

it("checks out the local branch that already tracks the chosen remote branch", async () => {
  host.refs = refs("feature");
  await mount();
  await branchKey();
  await choose("remote:refs/remotes/origin/main");
  await choose("menu:checkout");
  await vi.waitFor(() => expect(reads("git.checkout")).toHaveLength(1));
  expect(reads("git.checkout")[0]!.params.target).toEqual({ kind: "branch", name: "main" });
});

it("goes back from the action list with Esc and keeps the popup open", async () => {
  await mount();
  await branchKey();
  await choose("local:refs/heads/feature");
  expect(popup()?.getAttribute("data-git-popup")).toBe("branch-menu");
  await press({ code: "Escape", key: "Escape" }, input());
  expect(popup()?.getAttribute("data-git-popup")).toBe("branches");
  await press({ code: "Escape", key: "Escape" }, input());
  expect(popup()).toBeNull();
});

it("asks to force-delete only after the Host says the branch is not merged (2101)", async () => {
  await mount();
  await branchKey();
  await choose("local:refs/heads/feature");
  await choose("menu:delete");
  await vi.waitFor(() =>
    expect(document.querySelector("[data-testid=file-confirm-dialog]")).not.toBeNull(),
  );
  expect(reads("git.branch.delete").map((c) => c.params.force)).toEqual([false]);
  const dialog = document.querySelector("[data-testid=file-confirm-dialog]")!;
  expect(dialog.textContent).toContain("not fully merged");
  const force = [...dialog.querySelectorAll("button")].find((b) => b.textContent === "강제 삭제")!;
  await click(force);
  await vi.waitFor(() =>
    expect(reads("git.branch.delete").map((c) => c.params.force)).toEqual([false, true]),
  );
});

it("cancelling the force-delete question deletes nothing", async () => {
  await mount();
  await branchKey();
  await choose("local:refs/heads/feature");
  await choose("menu:delete");
  await vi.waitFor(() =>
    expect(document.querySelector("[data-testid=file-confirm-dialog]")).not.toBeNull(),
  );
  const dialog = document.querySelector("[data-testid=file-confirm-dialog]")!;
  await click([...dialog.querySelectorAll("button")].find((b) => b.textContent === "취소")!);
  await settle(50);
  expect(reads("git.branch.delete").map((c) => c.params.force)).toEqual([false]);
});

it("merges after one confirmation", async () => {
  await mount();
  await branchKey();
  await choose("local:refs/heads/feature");
  await choose("menu:merge");
  await vi.waitFor(() =>
    expect(document.querySelector("[data-testid=file-confirm-dialog]")).not.toBeNull(),
  );
  expect(reads("git.merge")).toHaveLength(0);
  const dialog = document.querySelector("[data-testid=file-confirm-dialog]")!;
  await click([...dialog.querySelectorAll("button")].find((b) => b.textContent === "진행")!);
  await vi.waitFor(() => expect(reads("git.merge")).toHaveLength(1));
  // ref 전체 이름 — 같은 이름의 태그가 있어도 브랜치를 merge 한다.
  expect(reads("git.merge")[0]!.params).toEqual({
    projectId: "p1",
    worktree: null,
    rev: "refs/heads/feature",
  });
});

it("keeps favorites in the project state and lists them first", async () => {
  await mount();
  await branchKey();
  const star = items()
    .find((i) => i.getAttribute("data-value") === "local:refs/heads/feature")!
    .querySelector("[data-favorite-toggle]")!;
  await click(star);
  expect(values().slice(0, 2)).toEqual(["action:new", "favorite:refs/heads/feature"]);
  await vi.waitFor(() =>
    expect(host.stateWrites.at(-1)).toMatchObject({ favoriteBranches: ["refs/heads/feature"] }),
  );
  await press({ code: "Escape", key: "Escape" }, input());
  await branchKey();
  expect(values()[1]).toBe("favorite:refs/heads/feature");
});

it("skips saved favorites whose branch is gone (deleted or renamed) and keeps the rest", async () => {
  host.savedState = {
    version: 1,
    favoriteBranches: ["refs/heads/gone", "refs/heads/feature", "refs/remotes/origin/renamed"],
  };
  await mount();
  await branchKey();
  const favorites = values().filter((v) => v?.startsWith("favorite:"));
  expect(favorites).toEqual(["favorite:refs/heads/feature"]);
  // 저장본에 남은 이름은 지우지 않는다(되살아난 브랜치는 다시 즐겨찾기로 보인다).
  expect(host.stateWrites).toEqual([]);
});

it("switches the active worktree through the open-request path and reopens the watch there", async () => {
  await mount();
  await branchKey();
  // 지금 워크트리는 고를 수 없다.
  expect(
    items()
      .find((i) => i.getAttribute("data-value") === "worktree:/work/demo")!
      .getAttribute("aria-disabled"),
  ).toBe("true");
  await choose("worktree:/work/wt");
  await vi.waitFor(() => expect(activeWorktreeStore.getState().top).toBe("/work/wt"));
  await vi.waitFor(() => expect(host.watches.at(-1)).toEqual({ worktree: "/work/wt" }));
  await vi.waitFor(() =>
    expect(reads("git.refs").some((c) => c.params.worktree === "/work/wt")).toBe(true),
  );

  // 메인 체크아웃으로 돌아온다(`mainCheckout`).
  host.worktrees = {
    ...host.worktrees,
    worktrees: host.worktrees.worktrees.map((w) => ({ ...w, current: w.path === "/work/wt" })),
  };
  await branchKey();
  await choose("worktree:/work/demo");
  await vi.waitFor(() => expect(activeWorktreeStore.getState().top).toBeUndefined());
});

it("finds a branch in the Search Everywhere branch tab and checks it out", async () => {
  await mount();
  for (let i = 0; i < 2; i++) {
    await act(async () => {
      document.activeElement?.dispatchEvent(
        new KeyboardEvent("keydown", {
          bubbles: true,
          code: "ShiftLeft",
          key: "Shift",
          shiftKey: true,
        }),
      );
      document.activeElement?.dispatchEvent(
        new KeyboardEvent("keyup", { bubbles: true, code: "ShiftLeft", key: "Shift" }),
      );
    });
  }
  await settle();
  const se = () => document.querySelector<HTMLElement>("[data-search-everywhere]");
  expect(se()?.getAttribute("data-search-everywhere")).toBe("all");
  const tabs = [...se()!.querySelectorAll("[role=tab]")].map((t) => t.textContent);
  // 이 창에는 프로젝트 창 열기(native)가 없어 프로젝트 탭이 없다.
  expect(tabs).toEqual(["전체", "파일", "액션", "브랜치"]);
  await click([...se()!.querySelectorAll("[role=tab]")].find((t) => t.textContent === "브랜치")!);
  expect(se()?.getAttribute("data-search-everywhere")).toBe("branches");
  const seInput = se()!.querySelector<HTMLInputElement>("[cmdk-input]")!;
  await type("feat", seInput);
  const found = [...se()!.querySelectorAll<HTMLElement>("[cmdk-item]")];
  expect(found.map((i) => i.getAttribute("data-value"))).toEqual(["branch:refs/heads/feature"]);
  await click(found[0]!);
  await vi.waitFor(() => expect(reads("git.checkout")).toHaveLength(1));
  expect(reads("git.checkout")[0]!.params.target).toEqual({ kind: "branch", name: "feature" });
});

it("lists this window's git actions in the VCS operations popup (⌃V)", async () => {
  await mount();
  await press({ code: "KeyV", key: "v", ctrlKey: true });
  expect(popup()?.getAttribute("data-git-popup")).toBe("vcs");
  expect(values()).toContain("git.branches");
  expect(values()).not.toContain("git.vcsOperations");
  const branches = items().find((i) => i.getAttribute("data-value") === "git.branches")!;
  expect(branches.textContent).toContain("⌃⇧B");
  await click(branches);
  await vi.waitFor(() => expect(popup()?.getAttribute("data-git-popup")).toBe("branches"));
});
