// 머지 문서의 결과 칸이 git 과 같은 바이트를 만드는지(13-m2-plan M2-19): fixtures/merge-cases.json 의 무작위 충돌 240건(LF ·
// 순수 CRLF · 섞인 줄바꿈 각 80 — crates/pw-git `conflict::tests::merge::window_merge_cases_match_pw_git_and_git_merge_file` 가 만들고
// 지금 pw-git · git 의 출력과 같은지 본다)마다 모든 충돌에 `≫` · `≪` · `≫` 뒤 `≪` 를 눌러 저장할 글을 `git merge-file -p --ours` ·
// `--theirs` · `--union` 과 바이트로 견준다. 처음 결과 칸 · 두 쪽 다 `✕` · 한쪽만 바꾼 구간의 `✕` 뒤 「모두 적용」 은 자동 병합
// 결과 그대로여야 한다. 차이는 0건이어야 한다.

import { getChunks, getOriginalDoc } from "@codemirror/merge";
import { EditorState } from "@codemirror/state";
import type { GitConflictDetail, MergeRegion } from "@pitwall/rpc-client";
import { expect, it } from "vitest";

import cases from "./fixtures/merge-cases.json";
import {
  applyNonConflicting,
  lineOffset,
  mergeInput,
  mergeResultExtension,
  regionAction,
  serialize,
  sliceLines,
} from "./merge-model";
import { compareWithBase } from "./merge-panes";

interface Case {
  style: "lf" | "crlf" | "mixed";
  base: string;
  ours: string;
  theirs: string;
  merged: string;
  regions: MergeRegion[];
  ours_result: string;
  theirs_result: string;
  union_result: string;
}

const side = (text: string) => ({
  id: "x",
  mode: "100644",
  size: text.length,
  tooLarge: false,
  content: { kind: "text" as const, text },
  judgment: null,
});

function inputOf(c: Case) {
  return mergeInput({
    merged: c.merged,
    regions: c.regions,
    base: side(c.base),
    ours: side(c.ours),
    theirs: side(c.theirs),
  } as unknown as GitConflictDetail)!;
}

function session(c: Case) {
  const input = inputOf(c);
  let state = EditorState.create({
    doc: input.docs.result,
    extensions: [
      input.model.separator ? EditorState.lineSeparator.of(input.model.separator) : [],
      mergeResultExtension(input),
    ],
  });
  return {
    act(i: number, s: "ours" | "theirs", a: "apply" | "ignore") {
      const spec = regionAction(state, input, i, s, a);
      if (!spec) throw new Error(`no ${a} on ${s} of ${i}`);
      state = state.update(spec).state;
    },
    applyAll() {
      const spec = applyNonConflicting(state, input);
      if (spec) state = state.update(spec).state;
    },
    text: () => serialize(state.doc.toString(), input.model),
  };
}

const ALL = cases as Case[];

it("covers the three line ending styles with conflicts", () => {
  for (const style of ["lf", "crlf", "mixed"])
    expect(ALL.filter((c) => c.style === style)).toHaveLength(80);
  expect(ALL.filter((c) => /\r(?!\n)/.test(c.ours + c.theirs + c.base)).length).toBeGreaterThan(20);
});

it("writes the bytes of git merge-file for ≫, ≪ and ≫ then ≪ on every conflict", () => {
  const wrong: string[] = [];
  ALL.forEach((c, n) => {
    const conflicts = c.regions.flatMap((r, i) => (r.kind === "conflict" ? [i] : []));
    const check = (what: string, got: string, want: string) => {
      if (got !== want)
        wrong.push(
          `#${n} ${c.style} ${what}: got ${JSON.stringify(got)} want ${JSON.stringify(want)}`,
        );
    };
    let m = session(c);
    check("start", m.text(), c.merged);
    for (const i of conflicts) m.act(i, "ours", "apply");
    check("ours", m.text(), c.ours_result);
    m = session(c);
    for (const i of conflicts) m.act(i, "theirs", "apply");
    check("theirs", m.text(), c.theirs_result);
    m = session(c);
    for (const i of conflicts) {
      m.act(i, "ours", "apply");
      m.act(i, "theirs", "apply");
    }
    check("union", m.text(), c.union_result);
    m = session(c);
    for (const i of conflicts) {
      m.act(i, "ours", "ignore");
      m.act(i, "theirs", "ignore");
    }
    check("ignore both", m.text(), c.merged);
    m = session(c);
    c.regions.forEach((r, i) => {
      if (r.kind !== "conflict") m.act(i, r.kind, "ignore");
    });
    m.applyAll();
    check("apply all", m.text(), c.merged);
  });
  expect(wrong.slice(0, 5)).toEqual([]);
  expect(wrong).toHaveLength(0);
});

it("compares each side with base on every case without a range error, over the regions' lines", () => {
  const wrong: string[] = [];
  ALL.forEach((c, n) => {
    const input = inputOf(c);
    for (const side of ["ours", "theirs"] as const) {
      const state = EditorState.create({
        doc: input.docs[side],
        extensions: [
          input.model.separator ? EditorState.lineSeparator.of(input.model.separator) : [],
          compareWithBase(input, side),
        ],
      });
      if (getOriginalDoc(state).toString() !== input.docs.base)
        wrong.push(`#${n} ${side}: base text`);
      const chunks = getChunks(state)?.chunks ?? [];
      for (const r of input.regions) {
        if (r.kind !== side && r.kind !== "conflict") continue;
        // 그쪽이 base 와 같은 충돌 구간(다른 쪽만 바뀜)은 보일 차이가 없다.
        if (sliceLines(input.texts[side], r[side]) === sliceLines(input.texts.base, r.base))
          continue;
        // merge 는 덩어리 앞뒤의 같은 줄을 잘라 내므로 구간과 겹치는 덩어리가 있는지만 본다.
        const from = lineOffset(input.texts[side], r[side].start);
        const to = lineOffset(input.texts[side], r[side].start + r[side].count);
        if (!chunks.some((k) => k.fromB <= to && k.toB >= from))
          wrong.push(`#${n} ${c.style} ${side}: no chunk over ${from}..${to}`);
      }
    }
  });
  expect(wrong.slice(0, 5)).toEqual([]);
});
