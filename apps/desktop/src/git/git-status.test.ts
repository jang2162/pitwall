// 창의 git status 저장소(M2-12): porcelain 의 `XY` 에서 고른 글자 · 색, 바뀐 경로만 바꾸는 갱신, 폴더 색, 그리고 `git.watch` ·
// `git.statusChanged` 구독의 수명(닫으면 스트림 취소 · 구독 해제 · 비움, 읽는 동안 온 이벤트는 모아 한 번 더 읽기, 닫을 때 닿은 응답은
// 버리기), 저장소가 없는 동안의 다시 열기.

import {
  createMemoryTransportPair,
  type GitStatus,
  type GitStatusEntry,
  RpcClient,
} from "@pitwall/rpc-client";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { FsChanged } from "../file-tree/tree-model";

import {
  entryDecoration,
  entryLetter,
  type GitStatusChanged,
  GitStatusStore,
  RETRY_MS,
  watchGitStatus,
} from "./git-status";
import {
  EXPECTED_LETTERS,
  MERGE_LETTERS,
  MERGE_PORCELAIN,
  PORCELAIN,
  porcelainEntries,
} from "./testing";

const status = (entries: GitStatusEntry[], workTree: string | null = "/repo"): GitStatus => ({
  workTree,
  entries,
  unsupported: null,
});

const entry = (path: string, change: Partial<GitStatusEntry> = {}): GitStatusEntry => ({
  path,
  origPath: null,
  staged: null,
  unstaged: "modified",
  conflict: null,
  ...change,
});

describe("entryLetter", () => {
  it.each([
    ["a real repository", PORCELAIN, EXPECTED_LETTERS],
    ["a merge in progress (every conflict kind, RM, a submodule)", MERGE_PORCELAIN, MERGE_LETTERS],
  ])("picks the porcelain letter git shows for every XY of %s", (_, porcelain, expected) => {
    const entries = porcelainEntries(porcelain);
    const letters = Object.fromEntries(entries.map((e) => [e.path, entryLetter(e)]));
    expect(letters).toEqual(expected);
    for (const [i, line] of porcelain.split("\n").entries()) {
      const letter = expected[entries[i]!.path]!;
      // 고른 글자는 그 줄의 X · Y 중 하나다(untracked 는 `??`). 충돌은 어느 종류든 `U` 다.
      if (entries[i]!.conflict) expect(letter).toBe("U");
      else expect(line.slice(0, 2)).toContain(letter);
    }
  });

  it("gives every letter its own status color", () => {
    const tones = Object.fromEntries(
      porcelainEntries(PORCELAIN).map((e) => [e.path, entryDecoration(e).tone]),
    );
    expect(tones).toMatchObject({
      "src/added.rs": "added",
      "src/modified.rs": "modified",
      "src/type.rs": "modified",
      "src/deleted.rs": "deleted",
      "src/new-name.rs": "renamed",
      "untracked.txt": "untracked",
      "src/conflict.rs": "conflicted",
    });
    expect(entryDecoration(entry("c", { staged: "copied", unstaged: null })).tone).toBe("added");
  });

  it("hands back the same object for the same status so unchanged rows do not repaint", () => {
    expect(entryDecoration(entry("a"))).toBe(entryDecoration(entry("b")));
    expect(entryDecoration(entry("a"))).not.toBe(
      entryDecoration(entry("a", { staged: "modified" })),
    );
  });
});

describe("GitStatusStore", () => {
  it("decorates absolute paths under the work tree and nothing outside it", () => {
    const store = new GitStatusStore();
    store.apply(status(porcelainEntries(PORCELAIN)));
    expect(store.decoration("/repo/src/modified.rs")).toMatchObject({
      letter: "M",
      tone: "modified",
      unstaged: "modified",
    });
    expect(store.decoration("/repo/src/both.rs")).toMatchObject({
      staged: "modified",
      unstaged: "modified",
    });
    expect(store.decoration("/repo")).toBeUndefined();
    expect(store.decoration("/repo2/src/modified.rs")).toBeUndefined();
    expect(store.decoration("/other/src/modified.rs")).toBeUndefined();
  });

  it("colors folders by what is below them, without a letter", () => {
    const store = new GitStatusStore();
    store.apply(status(porcelainEntries(PORCELAIN)));
    expect(store.decoration("/repo/src")).toEqual({ tone: "conflicted" });
    expect(store.decoration("/repo/new")).toEqual({ tone: "untracked" });

    store.apply(
      status([
        entry("a/x", { staged: "added", unstaged: null }),
        entry("a/b/y", { staged: "added", unstaged: null }),
        entry("m/x", { staged: "added", unstaged: null }),
        entry("m/y", { unstaged: "untracked" }),
      ]),
    );
    expect(store.decoration("/repo/a")).toEqual({ tone: "added" });
    expect(store.decoration("/repo/a/b")).toEqual({ tone: "added" });
    expect(store.decoration("/repo/m")).toEqual({ tone: "modified" });
    expect(store.decoration("/repo/docs")).toBeUndefined();
  });

  it("gives a folder that is itself an entry (untracked nested repository) its letter", () => {
    const store = new GitStatusStore();
    store.apply(status([entry("vendor/nested/", { unstaged: "untracked" })]));
    expect(store.decoration("/repo/vendor/nested")).toMatchObject({ letter: "?" });
    expect(store.decoration("/repo/vendor")).toEqual({ tone: "untracked" });
  });

  it("replaces only the entries inside the re-read paths and recounts their folders", () => {
    const store = new GitStatusStore();
    store.apply(
      status([entry("src/a.rs"), entry("src/b.rs", { unstaged: "untracked" }), entry("README")]),
    );
    expect(store.decoration("/repo/src")).toEqual({ tone: "modified" });

    // `src/a.rs` 가 깨끗해졌다. 다른 경로는 그대로다.
    store.apply(status([]), ["src/a.rs"]);
    expect(store.decoration("/repo/src/a.rs")).toBeUndefined();
    expect(store.decoration("/repo/src/b.rs")).toMatchObject({ letter: "?" });
    expect(store.decoration("/repo/README")).toMatchObject({ letter: "M" });
    expect(store.decoration("/repo/src")).toEqual({ tone: "untracked" });

    // 폴더 경로는 그 아래 전체다.
    store.apply(status([entry("src/c.rs", { staged: "added", unstaged: null })]), ["src"]);
    expect(store.decoration("/repo/src/b.rs")).toBeUndefined();
    expect(store.decoration("/repo/src/c.rs")).toMatchObject({ letter: "A" });
    expect(store.decoration("/repo/src")).toEqual({ tone: "added" });
    expect(store.decoration("/repo/README")).toMatchObject({ letter: "M" });

    store.apply(status([]), ["src/c.rs"]);
    expect(store.decoration("/repo/src")).toBeUndefined();
  });

  it("replaces everything for a whole read, a new work tree, or an unsupported repository", () => {
    const store = new GitStatusStore();
    store.apply(status([entry("a"), entry("b")]));
    store.apply(status([entry("b")]));
    expect(store.decoration("/repo/a")).toBeUndefined();

    store.apply(status([entry("c")], "/other"), ["c"]);
    expect(store.workTree()).toBe("/other");
    expect(store.decoration("/repo/b")).toBeUndefined();
    expect(store.decoration("/other/c")).toBeDefined();

    store.apply({ workTree: "/other", entries: [], unsupported: "reftable" }, ["x"]);
    expect(store.unsupported()).toBe("reftable");
    expect(store.decoration("/other/c")).toBeUndefined();
    expect(store.entries()).toEqual([]);

    store.apply(status([entry("a")], null));
    expect(store.workTree()).toBeNull();
    expect(store.decoration("/repo/a")).toBeUndefined();
  });

  it("maps Windows work trees with backslashes to the repository's slash paths", () => {
    const store = new GitStatusStore();
    store.apply(status([entry("src/a.rs")], "C:\\repo"));
    expect(store.decoration("C:\\repo\\src\\a.rs")).toMatchObject({ letter: "M" });
    expect(store.decoration("C:\\repo\\src")).toEqual({ tone: "modified" });
  });
});

/**
 * `git.watch` · `git.status` · 구독을 흉내 내는 Host. `git.status` 는 `answer` 를 부를 때까지 답하지 않는다. `git.watch` 는 열 때의
 * `repo` 를 첫 항목으로 낸다(바꾸면 `git init` 처럼 다음에 연 스트림부터 보인다).
 */
function fakeHost(workTree: string | null = "/repo") {
  const [clientSide, serverSide] = createMemoryTransportPair();
  const send = (message: object) => serverSide.send(JSON.stringify({ jsonrpc: "2.0", ...message }));
  const host = {
    client: new RpcClient(clientSide),
    clientSide,
    repo: workTree,
    watches: [] as { streamId: number; params: unknown }[],
    cancelled: [] as number[],
    subscriptions: [] as { id: number; topics: string[] }[],
    unsubscribed: [] as number[],
    /** 답을 기다리는 `git.status` 의 `paths`. */
    reads: [] as { paths: string[] | null; answer(status: GitStatus): void }[],
    emit(payload: GitStatusChanged) {
      host.publish("git.statusChanged", payload);
    },
    emitFs(payload: Pick<FsChanged, "worktree" | "paths">) {
      host.publish("fs.changed", { root: "/repo", kinds: [], rescan: false, ...payload });
    },
    publish(topic: string, payload: object) {
      for (const s of host.subscriptions)
        if (!host.unsubscribed.includes(s.id) && s.topics.includes(topic))
          send({
            method: "events.event",
            params: { subscriptionId: s.id, projectId: "p1", topic, payload },
          });
    },
    /** 지금 살아 있는 `topic` 구독 수. */
    following(topic: string) {
      return host.subscriptions.filter(
        (s) => s.topics.includes(topic) && !host.unsubscribed.includes(s.id),
      ).length;
    },
  };
  let next = 1;
  serverSide.onMessage((text) => {
    const m = JSON.parse(text) as { id?: number; method: string; params: Record<string, unknown> };
    switch (m.method) {
      case "events.subscribe": {
        const id = next++;
        host.subscriptions.push({ id, topics: m.params.topics as string[] });
        return send({ id: m.id, result: { subscriptionId: id } });
      }
      case "events.unsubscribe":
        host.unsubscribed.push(m.params.subscriptionId as number);
        return send({ id: m.id, result: null });
      case "git.watch": {
        const streamId = next++;
        host.watches.push({ streamId, params: m.params });
        send({ id: m.id, result: { streamId } });
        return send({
          method: "stream.chunk",
          params: { streamId, data: { workTree: host.repo } },
        });
      }
      case "stream.cancel":
        return host.cancelled.push(m.params.streamId as number);
      case "git.status":
        return host.reads.push({
          paths: (m.params.paths as string[] | null) ?? null,
          answer: (result) => send({ id: m.id, result }),
        });
      case "request.cancel":
        return;
    }
  });
  return host;
}

describe("watchGitStatus", () => {
  it("subscribes, opens git.watch, reads the whole status once watching, then only changed paths", async () => {
    const host = fakeHost();
    const store = new GitStatusStore();
    const stop = watchGitStatus(host.client, "p1", "/wt", store);
    await vi.waitFor(() => expect(host.reads).toHaveLength(1));
    expect(host.subscriptions.map((s) => s.topics)).toEqual([["git.statusChanged"]]);
    expect(host.watches.map((w) => w.params)).toEqual([{ projectId: "p1", worktree: "/wt" }]);
    expect(host.reads[0]!.paths).toBeNull();
    host.reads[0]!.answer(status([entry("a"), entry("b")]));
    await vi.waitFor(() => expect(store.decoration("/repo/a")).toBeDefined());

    host.emit({ worktree: "/wt", paths: ["a"] });
    await vi.waitFor(() => expect(host.reads).toHaveLength(2));
    expect(host.reads[1]!.paths).toEqual(["a"]);
    host.reads[1]!.answer(status([]));
    await vi.waitFor(() => expect(store.decoration("/repo/a")).toBeUndefined());
    expect(store.decoration("/repo/b")).toBeDefined();
    stop();
  });

  it("ignores events of another worktree's window", async () => {
    const host = fakeHost();
    const store = new GitStatusStore();
    const stop = watchGitStatus(host.client, "p1", undefined, store);
    await vi.waitFor(() => expect(host.reads).toHaveLength(1));
    host.reads[0]!.answer(status([]));
    host.emit({ worktree: "/elsewhere", paths: ["a"] });
    host.emit({ worktree: null, paths: ["b"] });
    await vi.waitFor(() => expect(host.reads).toHaveLength(2));
    expect(host.reads[1]!.paths).toEqual(["b"]);
    stop();
  });

  it("gathers events that arrive while reading into one more read, whole if any asked for it", async () => {
    const host = fakeHost();
    const store = new GitStatusStore();
    const stop = watchGitStatus(host.client, "p1", undefined, store);
    await vi.waitFor(() => expect(host.reads).toHaveLength(1));
    host.emit({ worktree: null, paths: ["a"] });
    host.emit({ worktree: null, paths: ["b", "a"] });
    await new Promise((r) => setTimeout(r, 20));
    expect(host.reads).toHaveLength(1);
    host.reads[0]!.answer(status([]));
    await vi.waitFor(() => expect(host.reads).toHaveLength(2));
    expect(host.reads[1]!.paths).toEqual(["a", "b"]);

    host.emit({ worktree: null, paths: ["c"] });
    host.emit({ worktree: null, paths: null });
    host.reads[1]!.answer(status([]));
    await vi.waitFor(() => expect(host.reads).toHaveLength(3));
    expect(host.reads[2]!.paths).toBeNull();
    host.reads[2]!.answer(status([]));
    await new Promise((r) => setTimeout(r, 20));
    expect(host.reads).toHaveLength(3);
    stop();
  });

  it("cancels the watch, drops the subscription and clears the store when stopped", async () => {
    const host = fakeHost();
    const store = new GitStatusStore();
    const stop = watchGitStatus(host.client, "p1", undefined, store);
    await vi.waitFor(() => expect(host.reads).toHaveLength(1));
    host.reads[0]!.answer(status([entry("a")]));
    await vi.waitFor(() => expect(store.decoration("/repo/a")).toBeDefined());

    stop();
    expect(store.decoration("/repo/a")).toBeUndefined();
    expect(store.workTree()).toBeNull();
    await vi.waitFor(() => expect(host.cancelled).toEqual([host.watches[0]!.streamId]));
    await vi.waitFor(() => expect(host.unsubscribed).toEqual([host.subscriptions[0]!.id]));

    // 닫은 뒤의 이벤트 · 늦은 응답은 아무것도 바꾸지 않는다.
    host.emit({ worktree: null, paths: ["a"] });
    await new Promise((r) => setTimeout(r, 20));
    expect(host.reads).toHaveLength(1);
  });

  it("drops a git.status answer that lands as the watch stops, so the next worktree's watch never shows the old repository", async () => {
    const host = fakeHost();
    const store = new GitStatusStore();
    let stop = watchGitStatus(host.client, "p1", undefined, store);
    await vi.waitFor(() => expect(host.reads).toHaveLength(1));
    // 응답이 클라이언트에 닿은 바로 그때 — `git.status` 의 await 가 이어지기 전 — 활성 워크트리를 바꾼다(클라이언트의 리스너
    // 다음에 도는 리스너).
    const detach = host.clientSide.onMessage(() => {
      detach();
      stop();
      stop = watchGitStatus(host.client, "p1", "/wt", store);
    });
    host.reads[0]!.answer(status([entry("old")]));
    // 새 워크트리의 첫 읽기가 Host 에 닿았으면 옛 응답의 이어짐은 이미 돌았다.
    await vi.waitFor(() => expect(host.reads).toHaveLength(2));
    expect(store.workTree()).toBeNull();
    expect(store.decoration("/repo/old")).toBeUndefined();

    host.reads[1]!.answer(status([entry("new")], "/wt"));
    await vi.waitFor(() => expect(store.workTree()).toBe("/wt"));
    expect(store.decoration("/wt/new")).toMatchObject({ letter: "M" });
    expect(store.decoration("/repo/old")).toBeUndefined();
    stop();
  });

  it("does not read when the project has no repository", async () => {
    const host = fakeHost(null);
    const store = new GitStatusStore();
    const stop = watchGitStatus(host.client, "p1", undefined, store);
    // 「저장소 없음」 을 들었다는 진행 신호: 저장소를 기다리며 `fs.changed` 를 따른다.
    await vi.waitFor(() => expect(host.following("fs.changed")).toBe(1));
    await new Promise((r) => setTimeout(r, 20));
    expect(host.reads).toEqual([]);
    stop();
  });
});

/** 마이크로태스크(메모리 전송)가 다 돌 때까지 진짜 타이머로 기다리며 `check` 를 다시 본다 — 가짜 타이머를 쓰는 시험에서
 * `vi.waitFor` 는 가짜 시계를 함께 돌려(재시도 주기가 흐른다) 쓰지 않는다. 판정은 `check` 의 진행 신호다. */
async function until(check: () => void) {
  for (let tries = 0; ; tries++) {
    try {
      return check();
    } catch (e) {
      if (tries > 500) throw e;
      await new Promise((r) => setTimeout(r, 1));
    }
  }
}

describe("watchGitStatus without a repository", () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("reopens git.watch on the worktree's fs.changed and every RETRY_MS while visible, and stops once a repository shows up", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const visibility = { visibilityState: "visible" };
    vi.stubGlobal("document", visibility);
    const host = fakeHost(null);
    const store = new GitStatusStore();
    const stop = watchGitStatus(host.client, "p1", undefined, store);
    await until(() => expect(host.following("fs.changed")).toBe(1));
    expect(host.watches).toHaveLength(1);
    expect(vi.getTimerCount()).toBe(1);

    // 다른 워크트리 창의 fs.changed 는 무시하고, 이 창의 것은 닫고 다시 연다(`git init && touch a`).
    host.emitFs({ worktree: "/elsewhere", paths: ["/elsewhere/a"] });
    host.emitFs({ worktree: null, paths: ["/repo/a"] });
    await until(() => expect(host.watches).toHaveLength(2));
    expect(host.cancelled).toEqual([host.watches[0]!.streamId]);

    // 주기: 보이는 동안만 연다. 숨은 동안의 주기는 건너뛴다(뒤의 수가 하나만 늘어난 것으로 본다).
    vi.advanceTimersByTime(RETRY_MS);
    await until(() => expect(host.watches).toHaveLength(3));
    visibility.visibilityState = "hidden";
    vi.advanceTimersByTime(RETRY_MS);
    visibility.visibilityState = "visible";
    vi.advanceTimersByTime(RETRY_MS);
    await until(() => expect(host.watches).toHaveLength(4));
    expect(host.cancelled).toHaveLength(3);
    expect(host.reads).toEqual([]);

    // `git init` 만 했다(fs.changed 없음) — 다음 주기에 저장소를 받아 읽고, 재시도 · fs.changed 구독을 놓는다.
    host.repo = "/repo";
    vi.advanceTimersByTime(RETRY_MS);
    await until(() => expect(host.reads).toHaveLength(1));
    expect(host.reads[0]!.paths).toBeNull();
    expect(vi.getTimerCount()).toBe(0);
    await until(() => expect(host.following("fs.changed")).toBe(0));
    host.reads[0]!.answer(status([entry("a", { unstaged: "untracked" })]));
    await until(() => expect(store.decoration("/repo/a")).toMatchObject({ letter: "?" }));

    // 저장소가 생긴 뒤에는 다시 열지 않는다.
    vi.advanceTimersByTime(RETRY_MS * 3);
    host.emit({ worktree: null, paths: ["a"] });
    await until(() => expect(host.reads).toHaveLength(2));
    expect(host.watches).toHaveLength(5);
    stop();
  });

  it("leaves no retry timer or fs.changed subscription behind when the window closes or the worktree changes", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const host = fakeHost(null);
    const store = new GitStatusStore();
    let stop = watchGitStatus(host.client, "p1", undefined, store);
    await until(() => expect(host.following("fs.changed")).toBe(1));
    expect(vi.getTimerCount()).toBe(1);

    // 워크트리 전환: 닫고 새 워크트리로 다시 연다 — 타이머 · 구독은 늘 하나다.
    stop();
    expect(vi.getTimerCount()).toBe(0);
    stop = watchGitStatus(host.client, "p1", "/wt", store);
    await until(() => expect(host.watches).toHaveLength(2));
    await until(() => expect(vi.getTimerCount()).toBe(1));
    await until(() => expect(host.following("fs.changed")).toBe(1));

    // 창 닫기.
    stop();
    expect(vi.getTimerCount()).toBe(0);
    await until(() => expect(host.following("fs.changed")).toBe(0));
    await until(() => expect(host.following("git.statusChanged")).toBe(0));
    expect(host.cancelled).toEqual(host.watches.map((w) => w.streamId));
    vi.advanceTimersByTime(RETRY_MS * 3);
    host.emitFs({ worktree: "/wt", paths: ["/wt/a"] });
    // 닫은 뒤의 이벤트가 열었다면 Host 에 닿았을 만큼(구독 · 스트림 왕복) 기다린다.
    await new Promise((r) => setTimeout(r, 20));
    expect(host.watches).toHaveLength(2);
  });
});
