import type { FileMatch, SearchFilesParams } from "@pitwall/rpc-client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { BUSY_RETRY_MS, FILE_LIMIT, FileSearch } from "./file-search";

const scope = { projectId: "p1", worktree: null };

function file(relative: string): FileMatch {
  return {
    path: `/w/${relative}`,
    root: "/w",
    relative,
    positions: [],
    score: 1,
    vendored: false,
    generated: false,
  };
}

/** 부를 때마다 기다리는 답을 하나 만든다. 시험이 `answer` · `fail` 로 끝낸다. */
function fakeHost() {
  const calls: {
    params: SearchFilesParams;
    resolve(files: FileMatch[]): void;
    reject(e: unknown): void;
  }[] = [];
  const call = (params: SearchFilesParams) =>
    new Promise<{ files: FileMatch[]; matched: number; indexed: number }>((resolve, reject) =>
      calls.push({
        params,
        resolve: (files) => resolve({ files, matched: files.length, indexed: 10 }),
        reject,
      }),
    );
  return { calls, call };
}

const flush = () => new Promise((r) => setTimeout(r, 0));

afterEach(() => vi.useRealTimers());

describe("FileSearch", () => {
  it("sends one request at a time and only the last query typed while it waited", async () => {
    const host = fakeHost();
    const s = new FileSearch(host.call);
    s.request("a", scope);
    s.request("ab", scope);
    s.request("abc", scope);
    expect(host.calls.map((c) => c.params.query)).toEqual(["a"]);
    expect(host.calls[0]!.params).toEqual({ ...scope, query: "a", limit: FILE_LIMIT });
    host.calls[0]!.resolve([file("a.rs")]);
    await flush();
    // 옛 질의의 결과도 보이되 아직 기다리는 중이다.
    expect(s.get()).toMatchObject({ query: "a", pending: true });
    expect(host.calls.map((c) => c.params.query)).toEqual(["a", "abc"]);
    host.calls[1]!.resolve([file("abc.rs")]);
    await flush();
    expect(s.get()).toMatchObject({ query: "abc", pending: false });
    expect(s.get().files.map((f) => f.relative)).toEqual(["abc.rs"]);
  });

  it("does not ask again for the query it already has", async () => {
    const host = fakeHost();
    const s = new FileSearch(host.call);
    s.request("a", scope);
    host.calls[0]!.resolve([]);
    await flush();
    s.request("a", scope);
    expect(host.calls).toHaveLength(1);
    // 워크트리가 바뀌면 다시 묻는다.
    s.request("a", { projectId: "p1", worktree: "/wt" });
    expect(host.calls).toHaveLength(2);
  });

  it("keeps the results and retries the last query when the Host is busy", async () => {
    vi.useFakeTimers();
    const host = fakeHost();
    const s = new FileSearch(host.call);
    s.request("a", scope);
    host.calls[0]!.resolve([file("a.rs")]);
    await vi.advanceTimersByTimeAsync(0);
    s.request("ab", scope);
    host.calls[1]!.reject(new Error("file search is busy: 4 searches are already running"));
    await vi.advanceTimersByTimeAsync(0);
    expect(s.get()).toMatchObject({ query: "a", busy: true, pending: true });
    expect(s.get().files.map((f) => f.relative)).toEqual(["a.rs"]);
    expect(host.calls).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(BUSY_RETRY_MS);
    expect(host.calls.map((c) => c.params.query)).toEqual(["a", "ab", "ab"]);
    host.calls[2]!.resolve([file("ab.rs")]);
    await vi.advanceTimersByTimeAsync(0);
    expect(s.get()).toMatchObject({ query: "ab", busy: false, pending: false, error: undefined });
  });

  it("reports other failures without retrying, and asks again when the query is retyped", async () => {
    const host = fakeHost();
    const s = new FileSearch(host.call);
    s.request("a", scope);
    host.calls[0]!.reject(new Error("no such project"));
    await flush();
    expect(s.get()).toMatchObject({ error: "no such project", busy: false, pending: false });
    s.request("a", scope);
    expect(host.calls).toHaveLength(2);
  });
});

describe("FileSearch.forget", () => {
  it("drops the results and the answer of a request sent before it, and asks again", async () => {
    const host = fakeHost();
    const s = new FileSearch(host.call);
    s.request("a", scope);
    host.calls[0]!.resolve([file("a.rs")]);
    await flush();
    s.request("ab", scope);
    s.forget();
    expect(s.get().files).toEqual([]);
    host.calls[1]!.resolve([file("ab.rs")]);
    await flush();
    expect(s.get().files).toEqual([]);
    s.request("a", scope);
    expect(host.calls.map((c) => c.params.query)).toEqual(["a", "ab", "a"]);
  });
});
