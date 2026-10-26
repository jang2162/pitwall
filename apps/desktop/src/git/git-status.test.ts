// 창의 git status 저장소(M2-12): porcelain 의 `XY` 에서 고른 글자 · 색, 바뀐 경로만 바꾸는 갱신, 폴더 색, 그리고 `git.watch` ·
// `git.statusChanged` 구독의 수명(닫으면 스트림 취소 · 구독 해제 · 비움, 읽는 동안 온 이벤트는 모아 한 번 더 읽기).

import {
  createMemoryTransportPair,
  type GitStatus,
  type GitStatusEntry,
  RpcClient,
} from "@pitwall/rpc-client";
import { describe, expect, it, vi } from "vitest";

import {
  entryDecoration,
  entryLetter,
  type GitStatusChanged,
  GitStatusStore,
  watchGitStatus,
} from "./git-status";
import { EXPECTED_LETTERS, PORCELAIN, porcelainEntries } from "./testing";

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
  it("picks the porcelain letter git shows for every XY of a real repository", () => {
    const letters = Object.fromEntries(
      porcelainEntries(PORCELAIN).map((e) => [e.path, entryLetter(e)]),
    );
    expect(letters).toEqual(EXPECTED_LETTERS);
    for (const line of PORCELAIN.split("\n")) {
      const path = line.slice(3).split(" -> ").at(-1)!;
      const letter = EXPECTED_LETTERS[path]!;
      // 고른 글자는 그 줄의 X · Y 중 하나다(충돌은 `UU` 처럼 `U`, untracked 는 `??`).
      expect(line.slice(0, 2)).toContain(letter);
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

/** `git.watch` · `git.status` · 구독을 흉내 내는 Host. `git.status` 는 `answer` 를 부를 때까지 답하지 않는다. */
function fakeHost(workTree: string | null = "/repo") {
  const [clientSide, serverSide] = createMemoryTransportPair();
  const send = (message: object) => serverSide.send(JSON.stringify({ jsonrpc: "2.0", ...message }));
  const host = {
    client: new RpcClient(clientSide),
    watches: [] as { streamId: number; params: unknown }[],
    cancelled: [] as number[],
    subscriptions: [] as { id: number; topics: string[] }[],
    unsubscribed: [] as number[],
    /** 답을 기다리는 `git.status` 의 `paths`. */
    reads: [] as { paths: string[] | null; answer(status: GitStatus): void }[],
    emit(payload: GitStatusChanged) {
      for (const s of host.subscriptions)
        if (!host.unsubscribed.includes(s.id) && s.topics.includes("git.statusChanged"))
          send({
            method: "events.event",
            params: { subscriptionId: s.id, projectId: "p1", topic: "git.statusChanged", payload },
          });
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
        return send({ method: "stream.chunk", params: { streamId, data: { workTree } } });
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

  it("does not read when the project has no repository", async () => {
    const host = fakeHost(null);
    const store = new GitStatusStore();
    const watching = vi.fn();
    const stop = watchGitStatus(host.client, "p1", undefined, store, watching);
    await vi.waitFor(() => expect(watching).toHaveBeenCalledWith({ workTree: null }));
    await new Promise((r) => setTimeout(r, 20));
    expect(host.reads).toEqual([]);
    stop();
  });
});
