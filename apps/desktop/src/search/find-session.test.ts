import type { SearchFile, SearchTextChunk, SearchTextParams } from "@pitwall/rpc-client";
import { describe, expect, it } from "vitest";

import {
  DEFAULT_OPTIONS,
  FindSession,
  isBusyError,
  parseMask,
  type SearchRunner,
  searchParams,
} from "./find-session";

const place = { projectId: "p1", worktree: undefined, root: "/work/demo" };

describe("searchParams", () => {
  // 완료 기준의 rg 비교(`TODO` · 정규식 · 대소문자 · `*.rs`)가 쓰는 인자. 같은 옵션의 rg: `rg -c -s -g '*.rs' TODO`(ignore 존중,
  // 숨김 파일 뺌 — rg 의 기본).
  it("maps the regex, case and *.rs mask options the way rg reads them", () => {
    const params = searchParams(
      { ...DEFAULT_OPTIONS, query: "TODO", regex: true, matchCase: true, mask: "*.rs" },
      place,
    );
    expect(params).toEqual({
      projectId: "p1",
      worktree: null,
      paths: ["/work/demo"],
      query: "TODO",
      regex: true,
      matchCase: true,
      wholeWord: false,
      include: ["*.rs"],
      exclude: [],
      respectIgnore: true,
      excludeVendored: false,
      excludeGenerated: false,
      hidden: false,
    } satisfies SearchTextParams);
  });

  it("searches the chosen folder and carries the worktree", () => {
    const params = searchParams(
      { ...DEFAULT_OPTIONS, query: "x", scope: { kind: "folder", path: "/work/wt/demo/src" } },
      { ...place, worktree: "/work/wt", root: "/work/wt/demo" },
    );
    expect(params.paths).toEqual(["/work/wt/demo/src"]);
    expect(params.worktree).toBe("/work/wt");
  });
});

describe("searchParams, regex + case + include + exclude", () => {
  // 구분력 있는 rg 비교(결과가 있는 조합): `rg -c -s -g '*.rs' -g '!*_test.rs' 'fn \\w+_test'`(ignore 존중, 숨김 뺌)와 Host 의
  // 이 인자(crates/pw-fs/examples/search_first_chunk.rs)의 파일 · 줄 수를 맞춘다. 수치는 보고에 있다.
  it("maps regex, case, include and exclude globs together", () => {
    const params = searchParams(
      {
        ...DEFAULT_OPTIONS,
        query: "fn \\w+_test",
        regex: true,
        matchCase: true,
        mask: "*.rs, !*_test.rs",
      },
      place,
    );
    expect(params).toMatchObject({
      query: "fn \\w+_test",
      regex: true,
      matchCase: true,
      include: ["*.rs"],
      exclude: ["*_test.rs"],
      respectIgnore: true,
      hidden: false,
    });
  });

  it("re-roots a remembered folder scope when the worktree's root changes", () => {
    const scope = { kind: "folder", path: "/work/demo/src", root: "/work/demo" } as const;
    const wt = { ...place, worktree: "/work/wt", root: "/work/wt/demo" };
    expect(searchParams({ ...DEFAULT_OPTIONS, query: "x", scope }, wt).paths).toEqual([
      "/work/wt/demo/src",
    ]);
    // 루트 그대로면 건드리지 않는다.
    expect(searchParams({ ...DEFAULT_OPTIONS, query: "x", scope }, place).paths).toEqual([
      "/work/demo/src",
    ]);
    // 루트 자체를 고른 경우.
    expect(
      searchParams({ ...DEFAULT_OPTIONS, query: "x", scope: { ...scope, path: "/work/demo" } }, wt)
        .paths,
    ).toEqual(["/work/wt/demo"]);
    // 루트 밖을 직접 친 절대 경로는 그대로 둔다(Host 가 막는다).
    expect(
      searchParams({ ...DEFAULT_OPTIONS, query: "x", scope: { ...scope, path: "/etc" } }, wt).paths,
    ).toEqual(["/etc"]);
  });
});

describe("parseMask", () => {
  it("splits globs on commas and takes a leading ! as an exclusion", () => {
    expect(parseMask(" *.rs,  !*_test.rs ,, *.toml , !")).toEqual({
      include: ["*.rs", "*.toml"],
      exclude: ["*_test.rs"],
    });
    expect(parseMask("")).toEqual({ include: [], exclude: [] });
  });
});

function file(path: string, lines: number[]): SearchFile {
  return {
    path,
    lines: lines.map((line) => ({
      line,
      preview: `line ${line}`,
      previewStart: 0,
      ranges: [{ start: 0, end: 4 }],
      more: false,
    })),
  };
}

/** 시험이 청크를 하나씩 밀어 넣는 가짜 스트림. 열린 스트림마다 `signal` 이 남는다. */
function controlledRunner() {
  const streams: {
    params: SearchTextParams;
    signal: AbortSignal;
    push(chunk: SearchTextChunk): void;
    end(error?: Error): void;
  }[] = [];
  const run: SearchRunner = (params, signal) => {
    const queue: SearchTextChunk[] = [];
    let wake: (() => void) | undefined;
    let ended: { error?: Error } | undefined;
    streams.push({
      params,
      signal,
      push: (chunk) => {
        queue.push(chunk);
        wake?.();
      },
      end: (error) => {
        ended = { error };
        wake?.();
      },
    });
    return {
      async *[Symbol.asyncIterator]() {
        for (;;) {
          if (queue.length) {
            yield queue.shift()!;
            continue;
          }
          if (ended) {
            if (ended.error) throw ended.error;
            return;
          }
          await new Promise<void>((r) => (wake = r));
        }
      },
    };
  };
  return { run, streams };
}

const tick = () => new Promise((r) => setTimeout(r, 0));

describe("FindSession", () => {
  it("collects files in arrival order, flattens their lines and reports 「more」", async () => {
    const { run, streams } = controlledRunner();
    const session = new FindSession(run);
    const done = session.start(searchParams({ ...DEFAULT_OPTIONS, query: "x" }, place));
    streams[0]!.push({ kind: "files", files: [file("/work/demo/b.rs", [3, 9])] });
    await tick();
    expect(session.getSnapshot().status).toBe("searching");
    expect(session.getSnapshot().rows.map((r) => `${r.file.path}:${r.line.line}`)).toEqual([
      "/work/demo/b.rs:3",
      "/work/demo/b.rs:9",
    ]);
    streams[0]!.push({ kind: "files", files: [file("/work/demo/a.rs", [1])] });
    streams[0]!.push({
      kind: "done",
      matchedFiles: 2,
      lines: 3,
      truncated: true,
      searchedFiles: 10,
      binaryFiles: 0,
      largeFiles: 0,
    });
    streams[0]!.end();
    await done;
    const s = session.getSnapshot();
    expect(s.status).toBe("done");
    expect(s.truncated).toBe(true);
    expect(s.files.map((f) => f.path)).toEqual(["/work/demo/b.rs", "/work/demo/a.rs"]);
    expect(s.rows).toHaveLength(3);
  });

  it("closes the previous stream when a new search starts and drops its late chunks", async () => {
    const { run, streams } = controlledRunner();
    const session = new FindSession(run);
    void session.start(searchParams({ ...DEFAULT_OPTIONS, query: "a" }, place));
    void session.start(searchParams({ ...DEFAULT_OPTIONS, query: "ab" }, place));
    expect(streams.map((s) => s.signal.aborted)).toEqual([true, false]);
    streams[1]!.push({ kind: "files", files: [file("/work/demo/new.rs", [2])] });
    await tick();
    // 이전 검색의 청크가 새 검색의 것보다 늦게 와도 덮지 않는다.
    streams[0]!.push({ kind: "files", files: [file("/work/demo/old.rs", [1])] });
    await tick();
    expect(session.getSnapshot().params?.query).toBe("ab");
    expect(session.getSnapshot().files.map((f) => f.path)).toEqual(["/work/demo/new.rs"]);
  });

  it("stops on cancel and keeps what it got", async () => {
    const { run, streams } = controlledRunner();
    const session = new FindSession(run);
    void session.start(searchParams({ ...DEFAULT_OPTIONS, query: "a" }, place));
    streams[0]!.push({ kind: "files", files: [file("/work/demo/a.rs", [1])] });
    await tick();
    session.cancel();
    expect(streams[0]!.signal.aborted).toBe(true);
    expect(session.getSnapshot().status).toBe("cancelled");
    expect(session.getSnapshot().rows).toHaveLength(1);
  });

  it("cancel ends a search whose stream was closed by interrupt but never restarted", () => {
    const { run } = controlledRunner();
    const session = new FindSession(run);
    void session.start(searchParams({ ...DEFAULT_OPTIONS, query: "a" }, place));
    session.interrupt();
    expect(session.searchDue).toBe(true);
    expect(session.getSnapshot().status).toBe("searching");
    session.cancel();
    expect(session.searchDue).toBe(false);
    expect(session.getSnapshot().status).toBe("cancelled");
  });

  it("recognises the Host's busy error", () => {
    expect(isBusyError("search is busy: 2 searches are already running (-32603)")).toBe(true);
    expect(isBusyError("regex parse error")).toBe(false);
    expect(isBusyError(undefined)).toBe(false);
  });

  it("shows the Host's error when the stream fails", async () => {
    const { run, streams } = controlledRunner();
    const session = new FindSession(run);
    const done = session.start(searchParams({ ...DEFAULT_OPTIONS, query: "(" }, place));
    streams[0]!.end(new Error("regex parse error"));
    await done;
    expect(session.getSnapshot()).toMatchObject({ status: "failed", error: "regex parse error" });
  });
});
