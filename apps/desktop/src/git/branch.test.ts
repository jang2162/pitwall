// 브랜치 기능의 부품(13-m2-plan M2-13): 창의 ref 읽기(`watchBranch`)는 `git.watch` 를 열지 않고 `git.refsChanged` · 작업 트리 변화에서만
// 다시 읽는다(읽는 동안 온 이벤트는 한 번으로 모은다); 상태바의 표기; 동작(branch-ops.ts)이 고른 ref 를 어떤 RPC 인자로 보내는지;
// 비교 도우미가 M2-15 의 액션을 인자와 함께 부르는지; 즐겨찾기를 프로젝트 상태에서 읽는지; VCS 작업 팝업에 넣는 액션.

import { ActionRegistry } from "@pitwall/keymap";
import {
  createMemoryTransportPair,
  type GitRefs,
  RpcCallError,
  RpcClient,
} from "@pitwall/rpc-client";
import { describe, expect, it, vi } from "vitest";

import { readStateFile } from "../layout/project-state";
import { type BranchOpsDeps, branchNameProblem, createBranchOps } from "./branch-ops";
import { isVcsAction, keywordFilter } from "./branch-popup";
import { BranchStore, watchBranch } from "./branch-state";
import { headName, trackText } from "./branch-status";
import { type BranchStrings, branchStrings } from "./branch-strings";
import { canCompareCommits, compareCommits } from "./compare-action";
import { switchWorktree } from "./git-branch-feature";
import { GitStatusStore } from "./git-status";

const T: BranchStrings = branchStrings((id, vars) => (vars ? `${id} ${JSON.stringify(vars)}` : id));

function refs(extra: Partial<GitRefs> = {}): GitRefs {
  return {
    workTree: "/repo",
    head: { kind: "branch", name: "main", commit: "a".repeat(40) },
    branches: [
      {
        name: "main",
        refName: "refs/heads/main",
        commit: "a".repeat(40),
        head: true,
        upstream: {
          name: "origin/main",
          refName: "refs/remotes/origin/main",
          track: { ahead: 3, behind: 0 },
        },
      },
      {
        name: "topic",
        refName: "refs/heads/topic",
        commit: "b".repeat(40),
        head: false,
        upstream: null,
      },
    ],
    remoteBranches: [
      {
        remote: "origin",
        name: "origin/main",
        refName: "refs/remotes/origin/main",
        commit: "a".repeat(40),
      },
      {
        remote: "origin",
        name: "origin/topic",
        refName: "refs/remotes/origin/topic",
        commit: "c".repeat(40),
      },
    ],
    tags: [],
    unsupported: null,
    broken: [],
    ...extra,
  };
}

/** `git.refs` · `git.operation` 을 답하는 가짜 Host. 답을 늦추면(`hold`) 읽는 중에 온 이벤트를 볼 수 있다. */
function fakeHost() {
  const [clientSide, serverSide] = createMemoryTransportPair();
  const client = new RpcClient(clientSide);
  const send = (m: object) => serverSide.send(JSON.stringify({ jsonrpc: "2.0", ...m }));
  const subs: { id: number; topics: string[] }[] = [];
  const pending: (() => void)[] = [];
  const host = {
    client,
    methods: [] as string[],
    hold: false,
    current: refs(),
    release() {
      for (const p of pending.splice(0)) p();
    },
    emit(topic: string, payload: object) {
      for (const s of subs)
        if (s.topics.includes(topic))
          send({
            method: "events.event",
            params: { subscriptionId: s.id, projectId: "p1", topic, payload },
          });
    },
  };
  let next = 1;
  serverSide.onMessage((text) => {
    const m = JSON.parse(text) as { id?: number; method: string; params: Record<string, unknown> };
    host.methods.push(m.method);
    const reply = (result: unknown) => send({ id: m.id, result });
    switch (m.method) {
      case "events.subscribe": {
        const id = next++;
        subs.push({ id, topics: m.params.topics as string[] });
        return reply({ subscriptionId: id });
      }
      case "events.unsubscribe":
        return reply(null);
      case "git.refs": {
        const answer = host.current;
        if (host.hold) return pending.push(() => reply(answer));
        return reply(answer);
      }
      case "git.operation":
        return reply({ operation: null });
      case "request.cancel":
        return;
    }
  });
  return host;
}

const count = (host: ReturnType<typeof fakeHost>, method: string) =>
  host.methods.filter((m) => m === method).length;

describe("watchBranch", () => {
  it("reads once after subscribing, again on git.refsChanged of its worktree, and never opens git.watch", async () => {
    const host = fakeHost();
    const store = new BranchStore();
    const status = new GitStatusStore();
    const stop = watchBranch(host.client, "p1", undefined, store, status);
    await vi.waitFor(() => expect(store.get().refs?.workTree).toBe("/repo"));
    expect(count(host, "git.refs")).toBe(1);
    expect(count(host, "events.subscribe")).toBe(1);

    host.current = refs({ head: { kind: "branch", name: "topic", commit: "b".repeat(40) } });
    host.emit("git.refsChanged", { worktree: null });
    await vi.waitFor(() => expect(store.get().refs?.head).toMatchObject({ name: "topic" }));
    expect(count(host, "git.refs")).toBe(2);
    // 다른 워크트리 창의 이벤트는 읽지 않는다.
    host.emit("git.refsChanged", { worktree: "/wt" });
    await new Promise((r) => setTimeout(r, 20));
    expect(count(host, "git.refs")).toBe(2);
    expect(host.methods).not.toContain("git.watch");
    stop();
    expect(store.get().refs).toBeUndefined();
  });

  it("folds the events that come while a read is in flight into one more read", async () => {
    const host = fakeHost();
    const store = new BranchStore();
    const stop = watchBranch(host.client, "p1", undefined, store, new GitStatusStore());
    await vi.waitFor(() => expect(count(host, "git.refs")).toBe(1));
    await vi.waitFor(() => expect(store.get().refs).toBeDefined());
    host.hold = true;
    host.emit("git.refsChanged", { worktree: null });
    await vi.waitFor(() => expect(count(host, "git.refs")).toBe(2));
    for (let i = 0; i < 5; i++) host.emit("git.refsChanged", { worktree: null });
    await new Promise((r) => setTimeout(r, 20));
    expect(count(host, "git.refs")).toBe(2);
    host.hold = false;
    host.release();
    await vi.waitFor(() => expect(count(host, "git.refs")).toBe(3));
    await new Promise((r) => setTimeout(r, 20));
    expect(count(host, "git.refs")).toBe(3);
    stop();
  });

  it("reads again when the window's status finds the repository (the watch opened)", async () => {
    const host = fakeHost();
    const store = new BranchStore();
    const status = new GitStatusStore();
    const stop = watchBranch(host.client, "p1", undefined, store, status);
    await vi.waitFor(() => expect(count(host, "git.refs")).toBe(1));
    status.apply({ workTree: "/repo", entries: [], unsupported: null });
    await vi.waitFor(() => expect(count(host, "git.refs")).toBe(2));
    // 항목만 바뀐 것은 ref 를 다시 읽지 않는다.
    status.apply({ workTree: "/repo", entries: [], unsupported: null }, ["a.txt"]);
    await new Promise((r) => setTimeout(r, 20));
    expect(count(host, "git.refs")).toBe(2);
    stop();
  });
});

describe("status bar text", () => {
  it("drops the zero side of ahead/behind and names a detached HEAD by its short hash", () => {
    expect(trackText(refs())).toBe("↑3");
    expect(headName(refs())).toBe("main");
    expect(headName(refs({ head: { kind: "detached", commit: "0123456789abcdef" } }))).toBe(
      "0123456",
    );
    expect(headName(refs({ head: { kind: "unborn", name: "trunk" } }))).toBe("trunk");
    const noUpstream = refs();
    noUpstream.branches[0]!.upstream = null;
    expect(trackText(noUpstream)).toBe("");
  });
});

function opsWith(over: Partial<BranchOpsDeps> = {}) {
  const calls: { method: string; params: unknown }[] = [];
  const client = {
    call: vi.fn(async (method: string, params: unknown) => {
      calls.push({ method, params });
      if (method === "git.branch.create")
        return { workTree: "/repo", refName: "refs/heads/x", commit: null, head: null };
      return { workTree: "/repo", head: { kind: "branch", name: "x", commit: "a" } };
    }),
  } as unknown as RpcClient;
  const notify = vi.fn();
  const deps: BranchOpsDeps = {
    client: () => client,
    projectId: "p1",
    worktree: () => "/wt",
    refs: () => refs(),
    askName: async () => "picked",
    confirm: async () => true,
    notify,
    strings: () => T,
    registry: new ActionRegistry(),
    ...over,
  };
  return { ops: createBranchOps(deps), calls, notify };
}

describe("branch operations", () => {
  it("asks for a local name when a remote branch's name is taken by a branch that tracks something else", async () => {
    const asked = vi.fn(async () => "topic-2");
    const { ops, calls } = opsWith({ askName: asked });
    const topic = refs().remoteBranches[1]!;
    await ops.checkout({ kind: "remote", branch: topic });
    expect(asked).toHaveBeenCalledTimes(1);
    expect(calls).toEqual([
      {
        method: "git.checkout",
        params: {
          projectId: "p1",
          worktree: "/wt",
          target: { kind: "remote", remote: "origin", branch: "topic", local: "topic-2" },
        },
      },
    ]);
  });

  it("checks a tag out as a detached HEAD and makes a new branch from the chosen ref", async () => {
    const { ops, calls } = opsWith();
    await ops.checkout({
      kind: "tag",
      tag: { name: "v1", refName: "refs/tags/v1", target: "t", peeled: "c" },
    });
    await ops.create({ kind: "remote", branch: refs().remoteBranches[1]! });
    expect(calls.map((c) => c.params)).toEqual([
      { projectId: "p1", worktree: "/wt", target: { kind: "commit", rev: "refs/tags/v1" } },
      {
        projectId: "p1",
        worktree: "/wt",
        name: "picked",
        start: "refs/remotes/origin/topic",
        checkout: true,
      },
    ]);
  });

  it("does not report a git refusal (2004) twice — the write task already did", async () => {
    const client = {
      call: vi.fn(async () => {
        throw new RpcCallError({ code: 2004, message: "git failed" });
      }),
    } as unknown as RpcClient;
    const { ops, notify } = opsWith({ client: () => client });
    await ops.checkout({ kind: "local", branch: refs().branches[1]! });
    expect(notify).not.toHaveBeenCalled();
  });

  it("sends full ref names, so a branch and a tag with the same name stay apart", async () => {
    const both = refs({
      branches: [
        ...refs().branches,
        {
          name: "v1",
          refName: "refs/heads/v1",
          commit: "d".repeat(40),
          head: false,
          upstream: null,
        },
      ],
      tags: [{ name: "v1", refName: "refs/tags/v1", target: "e".repeat(40), peeled: null }],
    });
    const registry = new ActionRegistry();
    const compared = vi.fn();
    registry.register({ id: "git.compareCommits", label: "compare", args: true, run: compared });
    const { ops, calls } = opsWith({ refs: () => both, registry });
    const branch = { kind: "local", branch: both.branches[2]! } as const;
    const tag = { kind: "tag", tag: both.tags[0]! } as const;
    await ops.merge(branch);
    await ops.merge(tag);
    await ops.rebase(branch);
    await ops.checkout(tag);
    await ops.create(tag);
    ops.compare(branch);
    expect(calls.map((c) => [c.method, c.params])).toEqual([
      ["git.merge", { projectId: "p1", worktree: "/wt", rev: "refs/heads/v1" }],
      ["git.merge", { projectId: "p1", worktree: "/wt", rev: "refs/tags/v1" }],
      ["git.rebase", { projectId: "p1", worktree: "/wt", upstream: "refs/heads/v1" }],
      [
        "git.checkout",
        { projectId: "p1", worktree: "/wt", target: { kind: "commit", rev: "refs/tags/v1" } },
      ],
      [
        "git.branch.create",
        { projectId: "p1", worktree: "/wt", name: "picked", start: "refs/tags/v1", checkout: true },
      ],
    ]);
    expect(compared).toHaveBeenCalledWith({ a: "refs/heads/main", b: "refs/heads/v1" });
  });

  it("rejects names git would refuse before calling it", () => {
    const taken = new Set(["main"]);
    for (const bad of ["", "a b", "-x", "a..b", "x.lock", "x/", "a~1", "main", "@"])
      expect(branchNameProblem(T, bad, taken), bad).toBeDefined();
    expect(branchNameProblem(T, "feat/ok-1", taken)).toBeUndefined();
  });

  it("compares through the M2-15 action with { a: HEAD's branch, b: the chosen ref }", () => {
    const registry = new ActionRegistry();
    const run = vi.fn();
    registry.register({ id: "git.compareCommits", label: "compare", args: true, run });
    const { ops, notify } = opsWith({ registry });
    ops.compare({ kind: "remote", branch: refs().remoteBranches[1]! });
    expect(run).toHaveBeenCalledWith({ a: "refs/heads/main", b: "refs/remotes/origin/topic" });
    expect(notify).not.toHaveBeenCalled();
  });
});

describe("compareCommits", () => {
  it("passes the arguments to the registered action and says false when there is none", () => {
    const registry = new ActionRegistry();
    expect(compareCommits(registry, { a: "x", b: "y" })).toBe(false);
    const run = vi.fn();
    registry.register({ id: "git.compareCommits", label: "compare", args: true, run });
    expect(compareCommits(registry, { a: "x", b: "y" })).toBe(true);
    expect(run).toHaveBeenCalledWith({ a: "x", b: "y" });
  });

  // M2-15 의 액션은 `args: true` 라 `isEnabled` 가 늘 거짓이다 — 그걸로 보이면 [비교] 가 사라진다.
  it("shows the compare entry for M2-15's args action while it is enabled, not otherwise", () => {
    const registry = new ActionRegistry();
    expect(canCompareCommits(registry)).toBe(false);
    let ready = true;
    registry.register({
      id: "git.compareCommits",
      label: "compare",
      args: true,
      enabled: () => ready,
      run: vi.fn(),
    });
    expect(canCompareCommits(registry)).toBe(true);
    ready = false;
    expect(canCompareCommits(registry)).toBe(false);
    expect(compareCommits(registry, { a: "x", b: "y" })).toBe(false);
  });
});

describe("worktree switch", () => {
  it("sends the worktree top, or mainCheckout for the project's main checkout, and nothing for the current one", () => {
    const push = vi.fn();
    const base = { head: null, main: false, current: false, locked: false, missing: false };
    switchWorktree({ ...base, path: "/wt", switchTo: { worktree: "/wt" } }, push);
    switchWorktree({ ...base, path: "/repo", main: true, switchTo: { worktree: null } }, push);
    switchWorktree({ ...base, path: "/cur", current: true, switchTo: { worktree: "/cur" } }, push);
    switchWorktree({ ...base, path: "/other", switchTo: null }, push);
    expect(push.mock.calls).toEqual([[{ worktree: "/wt" }], [{ mainCheckout: true }]]);
  });
});

describe("popup helpers", () => {
  it("filters by keywords only, not by the value that keeps groups apart", () => {
    expect(keywordFilter("local:refs/heads/main", "refs", ["main"])).toBe(0);
    expect(keywordFilter("local:refs/heads/main", "mai", ["main"])).toBeGreaterThan(0);
  });

  it("puts git actions and the Git · commit tool windows in the VCS popup, not itself", () => {
    expect(isVcsAction("git.branches")).toBe(true);
    expect(isVcsAction("toolWindow.git")).toBe(true);
    expect(isVcsAction("git.vcsOperations")).toBe(false);
    expect(isVcsAction("toolWindow.project")).toBe(false);
  });

  it("reads favorite branches from the project state and drops junk", () => {
    expect(
      readStateFile({
        version: 1,
        favoriteBranches: ["refs/heads/a", 3, "", "refs/heads/a", "refs/remotes/o/b"],
      })?.favoriteBranches,
    ).toEqual(["refs/heads/a", "refs/remotes/o/b"]);
    expect(readStateFile({ version: 1 })?.favoriteBranches).toBeUndefined();
  });
});
