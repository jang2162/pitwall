// @vitest-environment happy-dom
import { Text } from "@codemirror/state";
import { describe, expect, it, vi } from "vitest";

import { LiveText } from "../../editor/live-text";
import { gitDocumentKey, isGitDocumentSpec } from "../documents/spec";
import { RevisionBlame } from "./blame-extension";
import { gitGutterActions } from "./git-gutter";
import { BLAME_REVISION_ACTION } from "./gutter-host";
import { baseLineOf, revertChange } from "./gutter-model";
import { diffLines, keysOf, splitLines } from "./line-diff";
import { applyLines, planStage, storedLines } from "./stage-plan";
import { gitGutterStrings } from "./strings";
import { FakeSource, openEditor, textBlob, TOP, unifiedFile } from "./testing";

describe("baseLineOf", () => {
  const hunks = [
    { oldFrom: 2, oldTo: 2, newFrom: 2, newTo: 4 }, // 두 줄 더함
    { oldFrom: 5, oldTo: 7, newFrom: 7, newTo: 8 }, // 둘 → 하나
    { oldFrom: 9, oldTo: 10, newFrom: 10, newTo: 10 }, // 하나 지움
  ];
  it("maps lines outside hunks by the shift so far and gives nothing inside", () => {
    expect([1, 2, 3, 4, 5, 6, 7, 8, 9, 10].map((n) => baseLineOf(hunks, n))).toEqual([
      1,
      undefined,
      undefined,
      2,
      3,
      4,
      undefined,
      7,
      8,
      10,
    ]);
    expect(baseLineOf([], 3)).toBe(3);
  });
});

describe("revertChange", () => {
  const lf = (lines: string[], eol = true) => ({ lines, eol });
  it("replaces, removes and re-inserts lines up to the next line's start", () => {
    const doc = Text.of(["a", "B", "c", ""]);
    expect(
      revertChange(doc, { oldFrom: 2, oldTo: 3, newFrom: 2, newTo: 3 }, lf(["a", "b", "c"]), "\n"),
    ).toEqual({ from: 2, to: 4, insert: "b\n" });
    expect(
      revertChange(doc, { oldFrom: 2, oldTo: 2, newFrom: 2, newTo: 3 }, lf(["a", "c"]), "\n"),
    ).toEqual({ from: 2, to: 4, insert: "" });
    expect(
      revertChange(
        doc,
        { oldFrom: 4, oldTo: 5, newFrom: 4, newTo: 4 },
        lf(["a", "B", "c", "x"]),
        "\n",
      ),
    ).toEqual({ from: 6, to: 6, insert: "x\n" });
  });

  it("keeps HEAD's missing final newline when the hunk reaches HEAD's end", () => {
    const doc = Text.of(["a", "b", "c", ""]);
    // HEAD "a\nb" → 지금 "a\nb\nc\n": 끝 줄(개행 없음 → 있음)과 더한 줄이 한 hunk 다.
    expect(
      revertChange(
        doc,
        { oldFrom: 2, oldTo: 3, newFrom: 2, newTo: 4 },
        lf(["a", "b"], false),
        "\n",
      ),
    ).toEqual({ from: 2, to: 6, insert: "b" });
  });
});

/** 진짜 git(2.54 `git diff -U3`)의 출력으로 거터 hunk 의 스테이징 계획을 본다. */
describe("planStage", () => {
  /** HEAD(= index)와 작업 트리, git 의 diff 로 거터 hunk 마다의 계획. */
  function plansOf(head: string, work: string, patch: string) {
    const diff = unifiedFile(patch);
    const index = storedLines(head);
    const hunks = diffLines(
      keysOf(splitLines(head, undefined)),
      keysOf(splitLines(work, undefined)),
    );
    return hunks.map((hunk) => {
      const plan = planStage({
        head: index,
        index,
        diff,
        hunk,
        workTreeLines: splitLines(work, undefined).lines.length,
      });
      const staged =
        plan.kind === "ready"
          ? applyLines(keysOf(index), diff, (hi, li) =>
              plan.hunks.some(
                (s) => s.hunk === diff.hunks[hi] && (s.lines == null || s.lines.includes(li)),
              ),
            )
          : undefined;
      return { hunk, kind: plan.kind, staged };
    });
  }

  it("deletes a block at the end of the file where git does (review F1 case)", () => {
    const got = plansOf(
      "fn a() {\n}\n\nfn b() {\n}\n",
      "fn a() {\n}\n",
      "@@ -1,5 +1,2 @@\n fn a() {\n }\n-\n-fn b() {\n-}\n",
    );
    expect(got).toEqual([
      {
        hunk: { oldFrom: 3, oldTo: 6, newFrom: 3, newTo: 3 },
        kind: "ready",
        staged: ["fn a() {", "}"],
      },
    ]);
  });

  it("finds the lines when git lines up the same content at another place", () => {
    const got = plansOf("b\n\n\nc\n", "b\n*\n\nc\n", "@@ -1,4 +1,4 @@\n b\n-\n+*\n \n c\n");
    expect(got.map((p) => [p.kind, p.staged])).toEqual([["ready", ["b", "*", "", "c"]]]);
  });

  it("refuses a hunk whose neighbouring HEAD line is changed in the index", () => {
    // HEAD a b c, index a B c(b → B 를 스테이징), 작업 트리 a b C(B 를 되돌리고 c → C). 거터 hunk 「c → C」 의 앞 줄(HEAD 의 b)이
    // index 에서 바뀌어 index 의 어디에 얹을지 정할 수 없다.
    expect(
      planStage({
        head: storedLines("a\nb\nc\n"),
        index: storedLines("a\nB\nc\n"),
        diff: unifiedFile("@@ -1,3 +1,3 @@\n a\n-B\n-c\n+b\n+C\n"),
        hunk: { oldFrom: 3, oldTo: 4, newFrom: 3, newTo: 4 },
        workTreeLines: 3,
      }),
    ).toEqual({ kind: "inexact" });
  });

  it("refuses when the work tree diff does not match the document's lines", () => {
    expect(
      planStage({
        head: storedLines("a\n"),
        index: storedLines("a\n"),
        diff: unifiedFile("@@ -1 +1 @@\n-a\n+b\n"),
        hunk: { oldFrom: 1, oldTo: 2, newFrom: 1, newTo: 2 },
        workTreeLines: 2,
      }),
    ).toEqual({ kind: "inexact" });
  });

  it("knows a hunk already in the index", () => {
    expect(
      planStage({
        head: storedLines("a\nb\n"),
        index: storedLines("a\nB\n"),
        diff: undefined,
        hunk: { oldFrom: 2, oldTo: 3, newFrom: 2, newTo: 3 },
        workTreeLines: 2,
      }),
    ).toEqual({ kind: "already" });
  });
});

describe("GutterHost", () => {
  it("shares one model between views of a document and drops it with the last view", async () => {
    const source = new FakeSource();
    source.files.set("a.txt", textBlob("a\n"));
    const live = new LiveText({ text: "a\n", lineEnding: "lf", bom: false });
    const first = openEditor({ text: "a\n", source });
    const host = first.host;
    const a = host.acquire(`${TOP}/b.txt`, live);
    const b = host.acquire(`${TOP}/b.txt`, live);
    expect(a.doc).toBe(b.doc);
    a.release();
    expect(a.doc.disposed).toBe(false);
    b.release();
    expect(a.doc.disposed).toBe(true);
    first.destroy();
  });
});

describe("RevisionBlame", () => {
  it("annotates the revision's own lines without moving them", async () => {
    const blame = new RevisionBlame(async function* () {
      yield {
        file: { commit: "r", lines: 2, engine: "gix", moves: false },
        commits: [
          {
            hash: "h",
            authorName: "a",
            authorEmail: "e",
            authorTime: 0,
            subject: "s",
            boundary: false,
          },
        ],
        ranges: [
          { line: 1, count: 2, commit: "h", sourcePath: "a", sourceLine: 1, previous: null },
        ],
      };
    }, "r");
    await vi.waitFor(() => expect(blame.blame.kind).toBe("done"));
    expect(blame.lineBlame(2)).toMatchObject({ kind: "commit", commit: { hash: "h" } });
    expect(blame.lineBlame(3)).toBeUndefined();
    blame.dispose();
  });
});

describe("gitGutterActions", () => {
  it("opens a blame document only with a revision and a repository-relative path", () => {
    const opened: [string, string][] = [];
    const actions = gitGutterActions({
      view: () => undefined,
      strings: gitGutterStrings((id) => id),
      openBlame: (rev, path) => opened.push([rev, path]),
    });
    const run = actions.find((a) => a.id === BLAME_REVISION_ACTION)!;
    expect(run.args).toBe(true);
    void run.run({ rev: "abc", path: "src/a.rs" });
    void run.run({ rev: "abc", path: "/abs" });
    void run.run({ rev: "", path: "x" });
    void run.run(undefined);
    expect(opened).toEqual([["abc", "src/a.rs"]]);
    // 활성 뷰가 없으면 켜지지 않는다.
    expect(actions.filter((a) => a.enabled && !a.enabled()).map((a) => a.id)).toEqual([
      "git.nextChange",
      "git.previousChange",
      "git.toggleBlame",
    ]);
  });

  it("gives the blame document its own key and accepts it as a tab's params", () => {
    const spec = { kind: "blame", rev: "abc", path: "a.txt" } as const;
    expect(gitDocumentKey(spec)).toBe("git:blame:abc:a.txt");
    expect(gitDocumentKey({ kind: "revision", rev: "abc", path: "a.txt" })).not.toBe(
      gitDocumentKey(spec),
    );
    expect(isGitDocumentSpec(spec)).toBe(true);
    expect(isGitDocumentSpec({ kind: "blame", rev: "abc" })).toBe(false);
  });
});
