// Git 로그의 낱개 규칙: 필터 → RPC 값, 스트림 갈아타기, 고르기, 브랜치 패널, 파일 트리, 그리기 계산(접힌 레인 색 · 그래프 칸 폭 ·
// 화면 범위), 쓰기의 확인 흐름(reset 이 그사이 바뀌면 다시 묻기, 되돌리기가 잃을 것이 있으면 묻기).

import {
  createMemoryTransportPair,
  ERROR_CODE,
  type GitRefs,
  type LogChunk,
  RpcCallError,
  RpcClient,
} from "@pitwall/rpc-client";
import { describe, expect, it, vi } from "vitest";

import { bundledTranslator } from "../../i18n/testing";
import { type GitConfirm, lostEntries } from "../dialogs/git-dialogs";
import { gitDialogStrings } from "../dialogs/strings";
import defaultTheme from "../../../../../extensions/theme-default/themes/default.json";
import { parseTheme } from "../../theme/tokens";
import { panelRows, pickRef } from "./branch-panel";
import { fileRows } from "./commit-detail";
import { selectRows } from "./git-log-tool-window";
import { graphWidth, laneAt, laneColor, paletteOf, visibleRange } from "./graph-render";
import { FOLDED_COLOR } from "./lanes";
import {
  dayToUnix,
  EMPTY_QUERY,
  isHashText,
  listFiltered,
  LogModel,
  logParams,
  splitPaths,
} from "./log-model";
import { LogOps, refFullName, refNameProblem, undoModeOf } from "./log-ops";
import { gitLogStrings } from "./strings";
import { fixtureRows } from "./testing";

const T = gitLogStrings(bundledTranslator("ko"));
const D = gitDialogStrings(bundledTranslator("ko"));
const KEEP = { kind: "head", mode: "keep" } as const;

describe("logParams", () => {
  it("passes every filter field through as LogFilter, and a hex text as a hash prefix only when told it resolves", () => {
    expect(logParams(EMPTY_QUERY)).toEqual({
      refs: null,
      filter: {
        author: null,
        message: null,
        regex: false,
        ignoreCase: false,
        since: null,
        until: null,
        paths: [],
        hash: null,
      },
    });
    const all = logParams({
      refs: ["main", "origin/main"],
      text: " fix(ed)? ",
      regex: true,
      ignoreCase: true,
      author: " Kim ",
      since: "2026-01-02",
      until: "2026-01-03",
      paths: "src/a.rs,\n docs ,",
    });
    expect(all).toEqual({
      refs: ["main", "origin/main"],
      filter: {
        author: "Kim",
        message: "fix(ed)?",
        regex: true,
        ignoreCase: true,
        since: Math.floor(new Date(2026, 0, 2, 0, 0, 0).getTime() / 1000),
        until: Math.floor(new Date(2026, 0, 3, 23, 59, 59).getTime() / 1000),
        paths: ["src/a.rs", "docs"],
        hash: null,
      },
    });
    // 모양이 해시여도 풀린다고 알기 전에는 메시지다.
    expect(logParams({ ...EMPTY_QUERY, text: "DEADbeef" }).filter).toMatchObject({
      message: "DEADbeef",
      hash: null,
    });
    expect(logParams({ ...EMPTY_QUERY, text: "DEADbeef" }, true).filter).toMatchObject({
      message: null,
      hash: "deadbeef",
    });
    // 해시 모양이 아니면 풀린다고 해도 메시지다.
    expect(logParams({ ...EMPTY_QUERY, text: "added" }, true).filter).toMatchObject({
      message: "added",
      hash: null,
    });
    for (const word of [
      "abc",
      "added",
      "2024",
      "cafe",
      "decade",
      "abcdef",
      "a".repeat(65),
      "deadbeeg",
    ])
      expect(isHashText(word)).toBe(false);
    for (const hex of ["deadbee", "1234567", "a".repeat(40), "a".repeat(64), " ABCDEF0 "])
      expect(isHashText(hex)).toBe(true);
    expect(dayToUnix("nope", false)).toBeNull();
  });

  it("splits the path field on commas and newlines, keeping a quoted path's commas and spaces", () => {
    expect(splitPaths("src/a.rs,\n docs ,")).toEqual(["src/a.rs", "docs"]);
    expect(splitPaths('"a, b.txt", docs')).toEqual(["a, b.txt", "docs"]);
    expect(splitPaths(' "x,y" , "  lead.txt"')).toEqual(["x,y", "  lead.txt"]);
    expect(splitPaths('src/"one, two"/z.rs')).toEqual(["src/one, two/z.rs"]);
    expect(splitPaths('my dir/file.txt, "open, unclosed')).toEqual([
      "my dir/file.txt",
      "open, unclosed",
    ]);
    expect(splitPaths(" , ,\n")).toEqual([]);
    expect(logParams({ ...EMPTY_QUERY, paths: '"a,b", c' }).filter.paths).toEqual(["a,b", "c"]);
  });

  it("knows which filters drop the graph (everything but the branch filter)", () => {
    expect(listFiltered(logParams({ ...EMPTY_QUERY, refs: ["main"] }).filter)).toBe(false);
    for (const patch of [
      { text: "x" },
      { text: "abcd" },
      { author: "k" },
      { since: "2026-01-01" },
      { until: "2026-01-01" },
      { paths: "src" },
    ])
      expect(listFiltered(logParams({ ...EMPTY_QUERY, ...patch }).filter)).toBe(true);
  });
});

describe("LogModel", () => {
  it("drops a stream it replaced, so late chunks of the old query never land", async () => {
    const [client, server] = createMemoryTransportPair();
    const rpc = new RpcClient(client);
    const streams: number[] = [];
    const cancelled: number[] = [];
    let next = 1;
    server.onMessage((text) => {
      const m = JSON.parse(text) as { id?: number; method: string; params: { streamId?: number } };
      if (m.method === "git.log") {
        const streamId = next++;
        streams.push(streamId);
        server.send(JSON.stringify({ jsonrpc: "2.0", id: m.id, result: { streamId } }));
      } else if (m.method === "stream.cancel") cancelled.push(m.params.streamId!);
    });
    const chunk = (streamId: number, data: LogChunk) =>
      server.send(
        JSON.stringify({ jsonrpc: "2.0", method: "stream.chunk", params: { streamId, data } }),
      );
    const model = new LogModel();
    const source = { client: rpc, projectId: "p1" };
    void model.load(source, EMPTY_QUERY);
    await vi.waitFor(() => expect(streams).toEqual([1]));
    void model.load(source, { ...EMPTY_QUERY, text: "x" });
    await vi.waitFor(() => expect(streams).toEqual([1, 2]));
    expect(cancelled).toEqual([1]);
    const rows = fixtureRows("synth").slice(0, 3);
    chunk(1, { rows, total: 3 });
    chunk(2, { rows: rows.slice(0, 1).map((r) => ({ ...r, graph: null })), total: null });
    await vi.waitFor(() => expect(model.rows.length).toBe(1));
    expect(model.filtered).toBe(true);
    expect(model.indexOf(rows[0]!.hash)).toBe(0);
    expect(model.indexOf(rows[1]!.hash)).toBe(-1);
    model.dispose();
    rpc.close();
  });
});

describe("selection and panels", () => {
  const rows = fixtureRows("synth").slice(0, 10);
  it("selects one, toggles with ⌘, and takes a range with ⇧ from the anchor", () => {
    let s = selectRows(rows, [], -1, 2, "single");
    expect(s).toEqual({ selected: [rows[2]!.hash], anchor: 2 });
    s = selectRows(rows, s.selected, s.anchor, 5, "toggle");
    expect(s.selected).toEqual([rows[2]!.hash, rows[5]!.hash]);
    s = selectRows(rows, s.selected, s.anchor, 5, "toggle");
    expect(s.selected).toEqual([rows[2]!.hash]);
    s = selectRows(rows, s.selected, 2, 0, "range");
    expect(s.selected).toEqual(rows.slice(0, 3).map((r) => r.hash));
  });

  it("chooses branches like the branch panel: click = only it, again = none, ⌘ = add or remove", () => {
    expect(pickRef([], "main", false)).toEqual(["main"]);
    expect(pickRef(["main"], "main", false)).toEqual([]);
    expect(pickRef(["main", "x"], "main", false)).toEqual(["main"]);
    expect(pickRef(["main"], "x", true)).toEqual(["main", "x"]);
    expect(pickRef(["main", "x"], "main", true)).toEqual(["x"]);
  });

  it("names a log label by its full ref name, so a tag of the same name never stands in for the branch", () => {
    expect(refFullName({ name: "v1", kind: "branch", current: false })).toBe("refs/heads/v1");
    expect(refFullName({ name: "v1", kind: "tag", current: false })).toBe("refs/tags/v1");
    expect(refFullName({ name: "origin/v1", kind: "remote", current: false })).toBe(
      "refs/remotes/origin/v1",
    );
    expect(refFullName({ name: "HEAD", kind: "head", current: false })).toBe("HEAD");
  });

  it("lists HEAD, local, remote and tags, without the items of a closed group", () => {
    const refs: GitRefs = {
      workTree: "/r",
      head: { kind: "detached", commit: "c" },
      branches: [
        { name: "main", refName: "refs/heads/main", commit: "c", head: false, upstream: null },
      ],
      remoteBranches: [],
      tags: [{ name: "v1", refName: "refs/tags/v1", target: "c", peeled: null }],
      unsupported: null,
      broken: [],
    };
    expect(panelRows(refs, new Set(["tags"]))).toEqual([
      { kind: "all" },
      { kind: "head", name: "HEAD" },
      { kind: "section", section: "local", count: 1, open: true },
      { kind: "ref", section: "local", name: "main", refName: "refs/heads/main", current: false },
      { kind: "section", section: "tags", count: 1, open: false },
    ]);
    expect(panelRows({ ...refs, workTree: null }, new Set())).toEqual([]);
  });

  it("builds the changed-file tree with single-child folders joined", () => {
    const file = (path: string) => ({ path }) as Parameters<typeof fileRows>[0][number];
    expect(
      fileRows([
        file("src/main/kt/App.kt"),
        file("src/main/kt/Repo.kt"),
        file("README.md"),
        file("src/lib.rs"),
      ]).map((r) => (r.kind === "dir" ? `${r.depth}:${r.path}/` : `${r.depth}:${r.name}`)),
    ).toEqual(["0:src/", "1:main/kt/", "2:App.kt", "2:Repo.kt", "1:lib.rs", "0:README.md"]);
  });
});

describe("drawing math", () => {
  const palette = paletteOf(parseTheme(defaultTheme).variants.light!);
  it("takes the folded lane's color before the palette remainder", () => {
    expect(laneColor(palette, FOLDED_COLOR)).toBe(palette.folded);
    expect(laneColor(palette, 7)).toBe(palette.lanes[7]);
    expect(laneColor(palette, 9)).toBe(palette.lanes[1]);
    // 나머지로 골랐으면 graph.lane.8 이었다.
    expect(palette.folded).not.toBe(palette.lanes[FOLDED_COLOR % 8]);
  });

  it("caps the graph column at half the list and finds the lane under the pointer", () => {
    expect(graphWidth(-1, 800)).toBe(0);
    expect(graphWidth(2, 800)).toBe(12 + 3 * 12);
    expect(graphWidth(64, 800)).toBe(400);
    expect(laneAt(6, 48)).toBe(0);
    expect(laneAt(6 + 12 + 1, 48)).toBe(1);
    expect(laneAt(2, 48)).toBe(-1);
    expect(laneAt(60, 48)).toBe(-1);
  });

  it("draws only the rows inside the viewport", () => {
    expect(visibleRange(0, 220, 100_000)).toEqual([0, 10]);
    expect(visibleRange(22 * 500 + 5, 220, 100_000)).toEqual([500, 511]);
    expect(visibleRange(22 * 99_995, 220, 100_000)).toEqual([99_995, 100_000]);
  });
});

describe("LogOps", () => {
  it("rejects names git would refuse before asking the Host", () => {
    for (const bad of ["", " ", "-x", "a..b", "a b", "x.lock", "a:b", "a^", "HEAD", "a/", "@{1}"])
      expect(refNameProblem(bad, T.dialog), bad).toBeDefined();
    for (const good of ["feat/x", "v1.0", "release-2026"])
      expect(refNameProblem(good, T.dialog)).toBeUndefined();
  });

  it("lists lost changes renamed-first-as-written, then the overwritten untracked paths", () => {
    const lost = {
      changes: Array.from({ length: 40 }, (_, i) => ({
        path: `f${i}`,
        origPath: i === 0 ? "old" : null,
        staged: null,
        unstaged: "modified" as const,
        conflict: null,
      })),
      overwritten: ["u"],
    };
    const lines = lostEntries(lost);
    expect(lines[0]).toBe("old → f0");
    expect(lines).toHaveLength(41);
    expect(lines.at(-1)).toBe("u");
  });

  function opsWith(answers: (method: string, params: Record<string, unknown>) => unknown) {
    const calls: { method: string; params: Record<string, unknown> }[] = [];
    const confirms: GitConfirm[] = [];
    const notes: string[] = [];
    const client = {
      call: async (method: string, params: Record<string, unknown>) => {
        calls.push({ method, params });
        return answers(method, params);
      },
    } as unknown as RpcClient;
    const moved = vi.fn();
    const ops = new LogOps({
      client,
      projectId: "p1",
      askName: async () => undefined,
      askBranch: async () => undefined,
      confirm: async (q) => {
        confirms.push(q);
        return true;
      },
      notify: (n) => notes.push(n.title),
      strings: T,
      dialogStrings: D,
      moved,
    });
    return { ops, calls, confirms, notes, moved };
  }
  const conflict = (data: unknown) =>
    new RpcCallError({ code: ERROR_CODE.CONFLICT, message: "changed", data: data as never });
  const lostOne = { changes: [], overwritten: ["new.txt"] };

  it("asks again with the new list when the working tree changed between preview and reset", async () => {
    let resets = 0;
    const { ops, calls, confirms, moved } = opsWith((method) => {
      if (method === "git.resetPreview")
        return {
          workTree: "/r",
          head: "h0",
          target: "t",
          lost: { changes: [], overwritten: [] },
          fingerprint: "a",
        };
      resets++;
      if (resets === 1) throw conflict({ head: "h1", lost: lostOne, fingerprint: "b" });
      return {
        workTree: "/r",
        head: "t",
        moved: { refName: "refs/heads/main", previous: "h1", current: "t" },
        lost: null,
      };
    });
    await ops.reset("t", "hard", "reset");
    expect(confirms).toHaveLength(2);
    expect(confirms[1]!.lost!.changes.overwritten).toEqual(["new.txt"]);
    expect(confirms[1]!.body).toBe(D.lost.changed);
    expect(
      calls
        .filter((c) => c.method === "git.reset")
        .map((c) => [c.params.expected, c.params.fingerprint]),
    ).toEqual([
      ["h0", "a"],
      ["h1", "b"],
    ]);
    expect(moved).toHaveBeenCalledWith(
      { refName: "refs/heads/main", previous: "h1", current: "t" },
      KEEP,
    );
  });

  it("shows that it cannot confirm, and does not reset, when the preview has no fingerprint", async () => {
    const { ops, calls, confirms } = opsWith(() => ({
      workTree: "/r",
      head: "h0",
      target: "t",
      lost: lostOne,
      fingerprint: null,
    }));
    await ops.reset("t", "mixed", "reset");
    // 확인 단추를 눌렀다고 답해도(여기서는 늘 참) 하지 않는다.
    expect(calls.map((c) => c.method)).toEqual(["git.resetPreview"]);
    expect(confirms).toHaveLength(1);
    expect(confirms[0]!.lost!.fingerprint).toBeNull();
  });

  it("resets mixed without asking when nothing would be lost", async () => {
    const { ops, calls, confirms } = opsWith((method) =>
      method === "git.resetPreview"
        ? {
            workTree: "/r",
            head: "h0",
            target: "t",
            lost: { changes: [], overwritten: [] },
            fingerprint: "a",
          }
        : { workTree: "/r", head: "t", moved: null, lost: null },
    );
    await ops.reset("t", "mixed", "reset");
    expect(confirms).toHaveLength(0);
    expect(calls.at(-1)!.params).toMatchObject({ mode: "mixed", fingerprint: "a", expected: "h0" });
  });

  it("previews an undo, asks when it would lose changes, then sends that fingerprint", async () => {
    const { ops, calls, confirms } = opsWith((method) =>
      method === "git.resetPreview"
        ? { workTree: "/r", head: "c", target: "p", lost: lostOne, fingerprint: "k" }
        : { workTree: "/r", head: "p", moved: null, lost: null },
    );
    const done = await ops.undo({ refName: "refs/heads/main", previous: "p", current: "c" }, KEEP);
    expect(done).toBe(true);
    expect(confirms[0]!.lost!.changes.overwritten).toEqual(["new.txt"]);
    expect(calls.map((c) => c.method)).toEqual(["git.resetPreview", "git.reset"]);
    expect(calls[0]!.params).toMatchObject({ commit: "p", mode: "keep" });
    expect(calls[1]!.params).toMatchObject({
      commit: "p",
      mode: "keep",
      expected: "c",
      expectedRef: "refs/heads/main",
      fingerprint: "k",
    });
  });

  it("asks again with the new list when the working tree changed between the undo preview and the reset", async () => {
    let resets = 0;
    const { ops, calls, confirms } = opsWith((method) => {
      if (method === "git.resetPreview")
        return {
          workTree: "/r",
          head: "c",
          target: "p",
          lost: { changes: [], overwritten: [] },
          fingerprint: "a",
        };
      resets++;
      if (resets === 1) throw conflict({ head: "c", lost: lostOne, fingerprint: "k" });
      return { workTree: "/r", head: "p", moved: null, lost: null };
    });
    expect(await ops.undo({ refName: "refs/heads/main", previous: "p", current: "c" }, KEEP)).toBe(
      true,
    );
    expect(confirms).toHaveLength(1);
    expect(calls.filter((c) => c.method === "git.reset").map((c) => c.params.fingerprint)).toEqual([
      "a",
      "k",
    ]);
  });

  it("undoes a soft reset with a soft reset and no preview, a mixed one with mixed", async () => {
    expect(undoModeOf("soft")).toBe("soft");
    expect(undoModeOf("mixed")).toBe("mixed");
    expect(undoModeOf("keep")).toBe("keep");
    expect(undoModeOf("hard")).toBe("keep");
    const { ops, calls, confirms } = opsWith(() => ({
      workTree: "/r",
      head: "p",
      moved: null,
      lost: null,
    }));
    await ops.undo(
      { refName: "refs/heads/main", previous: "p", current: "c" },
      { kind: "head", mode: "soft" },
    );
    expect(confirms).toHaveLength(0);
    expect(calls.map((c) => [c.method, c.params.mode, c.params.fingerprint])).toEqual([
      ["git.reset", "soft", null],
    ]);
  });

  it("does not undo a ref that was created (no previous commit)", async () => {
    const { ops, calls } = opsWith(() => ({}));
    expect(await ops.undo({ refName: "refs/heads/x", previous: null, current: "c" }, KEEP)).toBe(
      false,
    );
    expect(calls).toHaveLength(0);
  });

  it("reverts newest first and reports a conflict stop without an undo", async () => {
    const { ops, calls, notes, moved } = opsWith(() => ({
      workTree: "/r",
      outcome: "stopped",
      head: "h",
      moved: null,
      operation: null,
    }));
    await ops.revert(["new", "old"], "revert");
    expect(calls[0]!.params.commits).toEqual(["new", "old"]);
    expect(notes).toEqual([T.result.stopped]);
    expect(moved).not.toHaveBeenCalled();
  });

  it("shows git's own words when a write is refused", async () => {
    const notes: { title: string; body?: string }[] = [];
    const ops = new LogOps({
      client: {
        call: async () => {
          throw new RpcCallError({
            code: 2004,
            message: "git failed",
            data: { stderr: "error: Your local changes would be overwritten" } as never,
          });
        },
      } as unknown as RpcClient,
      projectId: "p1",
      askName: async () => undefined,
      askBranch: async () => undefined,
      confirm: async () => true,
      notify: (n) => notes.push(n),
      strings: T,
      dialogStrings: D,
      moved: () => {},
    });
    await ops.checkoutCommit("c", "checkout");
    expect(notes[0]).toEqual({
      level: "error",
      title: T.result.failed("checkout"),
      body: "error: Your local changes would be overwritten",
    });
  });
});
