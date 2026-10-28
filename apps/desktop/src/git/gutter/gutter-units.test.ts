// @vitest-environment happy-dom
import { Text } from "@codemirror/state";
import type { DiffFile } from "@pitwall/rpc-client";
import { describe, expect, it, vi } from "vitest";

import { LiveText } from "../../editor/live-text";
import { gitDocumentKey, isGitDocumentSpec } from "../documents/spec";
import { RevisionBlame } from "./blame-extension";
import { gitGutterActions } from "./git-gutter";
import { BLAME_REVISION_ACTION } from "./gutter-host";
import { baseLineOf, revertChange, stageSelection } from "./gutter-model";
import { gitGutterStrings } from "./strings";
import { FakeSource, openEditor, textBlob, TOP } from "./testing";

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
  it("replaces, removes and re-inserts lines with the document's line break", () => {
    const doc = Text.of(["a", "B", "c"]);
    expect(revertChange(doc, { oldFrom: 2, oldTo: 3, newFrom: 2, newTo: 3 }, ["b"], "\n")).toEqual({
      from: 2,
      to: 3,
      insert: "b",
    });
    expect(revertChange(doc, { oldFrom: 2, oldTo: 2, newFrom: 2, newTo: 3 }, [], "\n")).toEqual({
      from: 2,
      to: 4,
      insert: "",
    });
    expect(revertChange(doc, { oldFrom: 3, oldTo: 4, newFrom: 3, newTo: 3 }, ["x"], "\n")).toEqual({
      from: 4,
      to: 4,
      insert: "x\n",
    });
  });
});

/** index ↔ 작업 트리 diff 하나: 3번 줄 앞에서 두 줄을 지우고(옛 3 · 4) 5번 줄을 더했다. */
const file: Pick<DiffFile, "hunks"> = {
  hunks: [
    {
      oldStart: 2,
      oldLines: 4,
      newStart: 2,
      newLines: 3,
      header: "",
      lines: [
        { kind: "context", text: "b", oldLine: 2, newLine: 2, noNewline: false },
        { kind: "removed", text: "c", oldLine: 3, newLine: null, noNewline: false },
        { kind: "removed", text: "d", oldLine: 4, newLine: null, noNewline: false },
        { kind: "context", text: "e", oldLine: 5, newLine: 3, noNewline: false },
        { kind: "added", text: "N", oldLine: null, newLine: 4, noNewline: false },
      ],
    },
  ],
};

describe("stageSelection", () => {
  it("picks the removed lines of a deletion hunk at its place and the added lines of an added hunk", () => {
    expect(stageSelection(file, { oldFrom: 3, oldTo: 5, newFrom: 3, newTo: 3 })).toEqual([
      { hunk: file.hunks[0], lines: [1, 2] },
    ]);
    expect(stageSelection(file, { oldFrom: 6, oldTo: 6, newFrom: 4, newTo: 5 })).toEqual([
      { hunk: file.hunks[0], lines: [4] },
    ]);
    // 두 hunk 를 다 덮으면 hunk 전체(`lines: null`).
    expect(stageSelection(file, { oldFrom: 3, oldTo: 6, newFrom: 3, newTo: 5 })).toEqual([
      { hunk: file.hunks[0], lines: null },
    ]);
  });

  it("finds nothing when the gutter hunk is already staged (absent from the work tree diff)", () => {
    expect(stageSelection(file, { oldFrom: 1, oldTo: 2, newFrom: 1, newTo: 2 })).toEqual([]);
    expect(stageSelection({ hunks: [] }, { oldFrom: 1, oldTo: 2, newFrom: 1, newTo: 2 })).toEqual(
      [],
    );
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
