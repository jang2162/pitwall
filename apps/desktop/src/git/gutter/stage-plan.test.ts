// [스테이징] 계획의 차등 시험 둘째 단계(첫째 · 셋째는 crates/pw-git/src/stage/tests/gutter.rs): 진짜 `git.diff` 를 담은 무작위 상태
// (`fixtures/stage-cases.json`)의 거터 hunk 마다 `planStage` 를 세워 `fixtures/stage-plans.json` 과 견준다. 셋째 단계가 그 계획을 진짜
// `git.stageLines` 로 돌려 index 를 목표와 대조한다 — 계획이 바뀌면 `PW_UPDATE_GUTTER_STAGE_PLANS=1` 로 다시 만들고 셋째 단계를 돌린다.

import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import type { DiffFile } from "@pitwall/rpc-client";
import { describe, expect, it } from "vitest";

import { diffLines, keysOf, splitLines } from "./line-diff";
import { applyLines, planStage, storedLines } from "./stage-plan";

const here = (name: string) => fileURLToPath(new URL(`./fixtures/${name}`, import.meta.url));

interface Case {
  head: string;
  index: string;
  work: string;
  diff: DiffFile | null;
}

/** 저장소 꼴의 줄 → 글(`applyLines` 의 결과). 끝 개행 없는 마지막 줄은 표시를 떼고 개행을 붙이지 않는다. */
function textOf(keys: readonly string[]): string {
  const noEol = keysOf({ lines: [""], eol: false })[0]!;
  return keys
    .map((k, i) =>
      i === keys.length - 1 && k.endsWith(noEol) ? k.slice(0, -noEol.length) : `${k}\n`,
    )
    .join("");
}

function plans(cases: readonly Case[]) {
  const out: unknown[] = [];
  cases.forEach((c, i) => {
    const head = splitLines(c.head, undefined);
    const work = splitLines(c.work, undefined);
    for (const hunk of diffLines(keysOf(head), keysOf(work))) {
      const plan = planStage({
        head: storedLines(c.head),
        index: storedLines(c.index),
        diff: c.diff ?? undefined,
        hunk,
        workTreeLines: work.lines.length,
      });
      if (plan.kind !== "ready") {
        out.push({ case: i, hunk, kind: plan.kind });
        continue;
      }
      // 목표 = 계획을 index 에 얹은 것(`planStage` 가 목표와 같은지 이미 보았다 — 셋째 단계가 진짜 git 으로 다시 본다).
      const picked = new Set(
        plan.hunks.flatMap((s) => {
          const hi = c.diff!.hunks.findIndex((h) => h === s.hunk);
          const lines =
            s.lines ?? s.hunk.lines.flatMap((l, li) => (l.kind === "context" ? [] : [li]));
          return lines.map((li) => `${hi}:${li}`);
        }),
      );
      const target = applyLines(keysOf(storedLines(c.index)), c.diff!, (hi, li) =>
        picked.has(`${hi}:${li}`),
      );
      // 고른 hunk 는 그 상태의 diff 안 번호로 싣는다(셋째 단계가 같은 diff 에서 찾는다).
      const hunks = plan.hunks.map((sel) => ({
        hunk: c.diff!.hunks.indexOf(sel.hunk),
        lines: sel.lines ?? null,
      }));
      out.push({ case: i, hunk, kind: "ready", hunks, target: textOf(target) });
    }
  });
  return out;
}

describe("planStage against real git.diff output", () => {
  const cases = JSON.parse(readFileSync(here("stage-cases.json"), "utf8")) as Case[];
  const got = plans(cases);

  it("matches the committed plans that the Rust side stages with git.stageLines", () => {
    if (process.env.PW_UPDATE_GUTTER_STAGE_PLANS) {
      writeFileSync(here("stage-plans.json"), `${JSON.stringify(got)}\n`);
      return;
    }
    const want = JSON.parse(readFileSync(here("stage-plans.json"), "utf8")) as unknown;
    expect(got).toEqual(want);
  });

  it("covers every gutter hunk of every case and refuses few of them", () => {
    const kinds = got.map((p) => (p as { kind: string }).kind);
    const count = (k: string) => kinds.filter((x) => x === k).length;
    console.info(
      `[gutter stage] ${cases.length} cases, ${kinds.length} hunks: ready ${count("ready")}, already ${count("already")}, inexact ${count("inexact")}`,
    );
    expect(cases.length).toBe(240);
    expect(kinds.length).toBeGreaterThanOrEqual(cases.length);
    // 이 fixture 는 빈 줄 · 괄호 · 같은 줄이 많아 두 diff 의 정렬이 일부러 자주 갈린다 — 고르는 비율이 줄면(회귀) 알린다.
    expect(count("ready") * 10).toBeGreaterThanOrEqual(kinds.length * 7);
  });
});
