import {
  ERROR_CODE,
  type FileMatch,
  RpcCallError,
  type SearchFilesParams,
} from "@pitwall/rpc-client";
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
    signal: AbortSignal;
    resolve(files: FileMatch[]): void;
    reject(e: unknown): void;
  }[] = [];
  const call = (params: SearchFilesParams, signal: AbortSignal) =>
    new Promise<{ files: FileMatch[]; matched: number; indexed: number }>((resolve, reject) =>
      calls.push({
        params,
        signal,
        resolve: (files) => resolve({ files, matched: files.length, indexed: 10 }),
        reject,
      }),
    );
  return { calls, call };
}

/** Host 의 바쁨(crates/pw-fs/src/index/mod.rs `turn`): `RpcError::internal(사유)` — message 는 「internal error」, 사유는 data. */
const busyError = () =>
  new RpcCallError({
    code: ERROR_CODE.INTERNAL_ERROR,
    message: "internal error",
    data: "file search is busy: 4 searches are already running",
  });

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
    host.calls[1]!.reject(busyError());
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

describe("FileSearch.stop", () => {
  it("cancels the request in flight and the busy retry, and keeps the results", async () => {
    vi.useFakeTimers();
    const host = fakeHost();
    const s = new FileSearch(host.call);
    s.request("a", scope);
    host.calls[0]!.resolve([file("a.rs")]);
    await vi.advanceTimersByTimeAsync(0);
    s.request("ab", scope);
    host.calls[1]!.reject(busyError());
    await vi.advanceTimersByTimeAsync(BUSY_RETRY_MS);
    expect(host.calls).toHaveLength(3);
    s.stop();
    expect(host.calls[2]!.signal.aborted).toBe(true);
    expect(s.get()).toMatchObject({ query: "a", pending: false, busy: false, error: undefined });
    expect(s.get().files.map((f) => f.relative)).toEqual(["a.rs"]);
    // 취소된 요청이 늦게 「바쁨」 으로 끝나도 다시 보내지 않는다.
    host.calls[2]!.reject(busyError());
    await vi.advanceTimersByTimeAsync(BUSY_RETRY_MS * 4);
    expect(host.calls).toHaveLength(3);
    expect(s.get().busy).toBe(false);
  });

  it("cancels a pending busy retry", async () => {
    vi.useFakeTimers();
    const host = fakeHost();
    const s = new FileSearch(host.call);
    s.request("a", scope);
    host.calls[0]!.reject(busyError());
    await vi.advanceTimersByTimeAsync(0);
    expect(s.get().busy).toBe(true);
    s.stop();
    await vi.advanceTimersByTimeAsync(BUSY_RETRY_MS * 4);
    expect(host.calls).toHaveLength(1);
  });

  it("sends the same query again right away, without waiting for the cancelled answer", async () => {
    const host = fakeHost();
    const s = new FileSearch(host.call);
    s.request("a", scope);
    s.forget();
    s.request("a", scope);
    expect(host.calls.map((c) => c.params.query)).toEqual(["a", "a"]);
    host.calls[0]!.resolve([file("old.rs")]);
    host.calls[1]!.resolve([file("a.rs")]);
    await flush();
    expect(s.get()).toMatchObject({ query: "a", pending: false });
    expect(s.get().files.map((f) => f.relative)).toEqual(["a.rs"]);
  });

  it("remembers where the results were found", async () => {
    const host = fakeHost();
    const s = new FileSearch(host.call);
    s.request("a", { projectId: "p1", worktree: "/wt" });
    host.calls[0]!.resolve([file("a.rs")]);
    await flush();
    expect(s.get().scope).toEqual({ projectId: "p1", worktree: "/wt" });
  });
});
