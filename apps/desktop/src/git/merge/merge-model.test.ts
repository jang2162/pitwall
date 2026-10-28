import { undo } from "@codemirror/commands";
import { EditorState } from "@codemirror/state";
import type { GitConflictDetail, MergeRegion } from "@pitwall/rpc-client";
import { describe, expect, it } from "vitest";
import { history } from "@codemirror/commands";

import {
  applyNonConflicting,
  hasIgnoredNonConflicting,
  lineModel,
  mergeField,
  mergeInput,
  type MergeInput,
  mergeResultExtension,
  rangeLines,
  rawLines,
  regionAction,
  serialize,
  sideButtons,
  unresolved,
} from "./merge-model";

const lines = (start: number, count: number) => ({ start, count });
const region = (
  kind: MergeRegion["kind"],
  base: [number, number],
  ours: [number, number],
  theirs: [number, number],
  merged: [number, number],
): MergeRegion => ({
  kind,
  base: lines(...base),
  ours: lines(...ours),
  theirs: lines(...theirs),
  merged: lines(...merged),
});

const side = (text: string) => ({
  id: "x",
  mode: "100644",
  size: text.length,
  tooLarge: false,
  content: { kind: "text" as const, text },
  judgment: null,
});

// crates/pw-git/src/testing/conflict.rs 의 BOTH_* 와 같은 세 쪽: 2 번째 줄은 충돌, 6 번째 줄은 그쪽만, 9 번째 줄은 우리 쪽만.
const BASE = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";
const OURS = "1\n2 ours\n3\n4\n5\n6\n7\n8\n9 ours\n10\n";
const THEIRS = "1\n2 theirs\n3\n4\n5\n6 theirs\n7\n8\n9\n10\n";
const MERGED = "1\n3\n4\n5\n6 theirs\n7\n8\n9 ours\n10\n";
const REGIONS = [
  region("conflict", [1, 1], [1, 1], [1, 1], [1, 0]),
  region("theirs", [5, 1], [5, 1], [5, 1], [4, 1]),
  region("ours", [8, 1], [8, 1], [8, 1], [7, 1]),
];

function detail(over: Partial<GitConflictDetail> = {}): GitConflictDetail {
  return {
    workTree: "/repo",
    file: {
      path: "both.txt",
      kind: "bothModified",
      base: null,
      ours: null,
      theirs: null,
      binary: false,
      submodule: false,
      symlink: false,
      large: false,
      tooLarge: false,
    },
    version: "v1",
    base: side(BASE),
    ours: side(OURS),
    theirs: side(THEIRS),
    merged: MERGED,
    clean: false,
    regions: REGIONS,
    ...over,
  };
}

function start(input: MergeInput) {
  return EditorState.create({
    doc: input.result.doc,
    extensions: [history(), mergeResultExtension(input), EditorState.lineSeparator.of("\n")],
  });
}

function act(
  state: EditorState,
  input: MergeInput,
  index: number,
  s: "ours" | "theirs",
  a: "apply" | "ignore",
) {
  const spec = regionAction(state, input, index, s, a);
  if (!spec) throw new Error(`no ${a} on ${s} of ${index}`);
  return state.update(spec).state;
}

describe("line model", () => {
  it("keeps LF, folds a CRLF-only text and leaves a mixed one split on LF", () => {
    expect(lineModel("a\nb\n")).toEqual({ doc: "a\nb\n", eol: "\n", separator: undefined });
    expect(lineModel("a\r\nb\r\n")).toEqual({ doc: "a\nb\n", eol: "\r\n", separator: undefined });
    expect(lineModel("a\r\nb\n")).toEqual({ doc: "a\r\nb\n", eol: "\n", separator: "\n" });
    expect(lineModel("a\rb")).toEqual({ doc: "a\rb", eol: "\n", separator: "\n" });
    expect(serialize("a\nb\n", { eol: "\r\n" })).toBe("a\r\nb\r\n");
    expect(rawLines("a\nb")).toEqual(["a\n", "b"]);
    expect(rawLines("")).toEqual([]);
  });
});

describe("merge regions", () => {
  const input = mergeInput(detail())!;

  it("starts from the automatic merge with only the conflict to pick", () => {
    const s = start(input);
    expect(s.doc.toString()).toBe(MERGED);
    expect(unresolved(s)).toEqual([0]);
    const { states, ranges } = s.field(mergeField);
    expect(sideButtons(states[0]!, "ours")).toEqual(["apply", "ignore"]);
    expect(sideButtons(states[1]!, "theirs")).toEqual(["ignore"]);
    expect(sideButtons(states[1]!, "ours")).toEqual([]);
    expect(ranges.map((r) => rangeLines(s.doc, r))).toEqual([0, 1, 1]);
  });

  it("puts ours then theirs into the conflict and counts it resolved", () => {
    let s = act(start(input), input, 0, "ours", "apply");
    expect(s.doc.toString()).toBe("1\n2 ours\n3\n4\n5\n6 theirs\n7\n8\n9 ours\n10\n");
    // 한쪽을 넣으면 풀린 것이다 — 그쪽 단추는 남아 이어 붙일 수 있다.
    expect(unresolved(s)).toEqual([]);
    expect(sideButtons(s.field(mergeField).states[0]!, "theirs")).toEqual(["apply", "ignore"]);
    s = act(s, input, 0, "theirs", "apply");
    expect(s.doc.toString()).toBe("1\n2 ours\n2 theirs\n3\n4\n5\n6 theirs\n7\n8\n9 ours\n10\n");
    expect(unresolved(s)).toEqual([]);
    expect(rangeLines(s.doc, s.field(mergeField).ranges[0]!)).toBe(2);
  });

  it("resolves a conflict by ignoring both sides, or by typing into it", () => {
    let s = act(start(input), input, 0, "ours", "ignore");
    expect(unresolved(s)).toEqual([0]);
    s = act(s, input, 0, "theirs", "ignore");
    expect(s.doc.toString()).toBe(MERGED);
    expect(unresolved(s)).toEqual([]);

    const typed = start(input).update({ changes: { from: 2, insert: "two\n" } }).state;
    expect(typed.doc.toString()).toBe("1\ntwo\n3\n4\n5\n6 theirs\n7\n8\n9 ours\n10\n");
    expect(unresolved(typed)).toEqual([]);
    expect(rangeLines(typed.doc, typed.field(mergeField).ranges[0]!)).toBe(1);
    // 고친 충돌은 단추를 거둔다.
    expect(sideButtons(typed.field(mergeField).states[0]!, "ours")).toEqual([]);
  });

  it("an edit elsewhere leaves the conflict open and moves the ranges", () => {
    const s = start(input).update({
      changes: { from: MERGED.indexOf("7"), insert: "seven\n" },
    }).state;
    expect(unresolved(s)).toEqual([0]);
    const next = act(s, input, 0, "theirs", "apply");
    expect(next.doc.toString()).toBe("1\n2 theirs\n3\n4\n5\n6 theirs\nseven\n7\n8\n9 ours\n10\n");
  });

  it("✕ on a one-sided change puts base back and «apply all non-conflicting» redoes it", () => {
    let s = act(start(input), input, 1, "theirs", "ignore");
    s = act(s, input, 2, "ours", "ignore");
    expect(s.doc.toString()).toBe("1\n3\n4\n5\n6\n7\n8\n9\n10\n");
    expect(hasIgnoredNonConflicting(s)).toBe(true);
    s = s.update(applyNonConflicting(s, input)!).state;
    expect(s.doc.toString()).toBe(MERGED);
    expect(hasIgnoredNonConflicting(s)).toBe(false);
    expect(applyNonConflicting(s, input)).toBeUndefined();
  });

  it("undo takes back the text and the button state together", () => {
    const s = act(start(input), input, 0, "ours", "apply");
    let after = s;
    undo({ state: s, dispatch: (tr) => (after = tr.state) });
    expect(after.doc.toString()).toBe(MERGED);
    expect(sideButtons(after.field(mergeField).states[0]!, "ours")).toEqual(["apply", "ignore"]);
  });

  it("keeps CRLF bytes through apply and serialize", () => {
    const crlf = (t: string) => t.replaceAll("\n", "\r\n");
    const i = mergeInput(
      detail({
        base: side(crlf(BASE)),
        ours: side(crlf(OURS)),
        theirs: side(crlf(THEIRS)),
        merged: crlf(MERGED),
      }),
    )!;
    const s = act(start(i), i, 0, "ours", "apply");
    expect(serialize(s.doc.toString(), i.result)).toBe(
      crlf("1\n2 ours\n3\n4\n5\n6 theirs\n7\n8\n9 ours\n10\n"),
    );
  });

  it("adds the missing line break when a side's last line has none", () => {
    // ours 끝 줄에 줄바꿈이 없고 충돌 뒤에 줄이 이어지는 꼴.
    const i = mergeInput(
      detail({
        base: side("a\nz\n"),
        ours: side("b"),
        theirs: side("c\nz\n"),
        merged: "z\n",
        regions: [region("conflict", [0, 1], [0, 1], [0, 1], [0, 0])],
      }),
    )!;
    const s = act(start(i), i, 0, "ours", "apply");
    expect(s.doc.toString()).toBe("b\nz\n");
  });

  it("has no merge for a side-only conflict", () => {
    expect(mergeInput(detail({ merged: null, regions: [] }))).toBeUndefined();
  });
});
