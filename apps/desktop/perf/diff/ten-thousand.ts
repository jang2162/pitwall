// 1만 줄 바뀐 파일의 `git.diff` · `git.blob` 응답(IPC 처럼 JSON 문자열): 2만 줄 중 짝수 줄 1만 개를 바꿨다(묶음 1만 개, 가장 잘게
// 쪼개진 경우). crates/pw-git 의 `diff_of_ten_thousand_changed_lines_in_release` · `testing::synth::perf_fixture` 와 같은 꼴이다.
// diff 첫 그림 하네스(diff-bench.tsx)와 Git 시나리오 스모크(perf/git-log/scenario-bench.tsx)가 쓴다.

import type { DiffFile, DiffHunk, DiffLine } from "@pitwall/rpc-client";

export const LINES = 20_000;

/** pw-git 의 `git.diff` 와 같은 꼴의 hunk(문맥 3줄 — 짝수 줄마다 바뀌어 hunk 하나로 이어진다). */
export function input(path = "big.rs") {
  const before: string[] = [];
  const after: string[] = [];
  for (let i = 0; i < LINES; i++) {
    before.push(`    let value_${i} = ${i};`);
    after.push(i % 2 === 0 ? `    let value_${i} = ${i} + 1;` : `    let value_${i} = ${i};`);
  }
  const lines: DiffLine[] = [];
  for (let i = 0; i < LINES; i++) {
    const n = i + 1;
    if (i % 2 === 0) {
      lines.push({
        kind: "removed",
        text: before[i]!,
        oldLine: n,
        newLine: null,
        noNewline: false,
      });
      lines.push({ kind: "added", text: after[i]!, oldLine: null, newLine: n, noNewline: false });
    } else
      lines.push({ kind: "context", text: before[i]!, oldLine: n, newLine: n, noNewline: false });
  }
  const hunk: DiffHunk = {
    oldStart: 1,
    oldLines: LINES,
    newStart: 1,
    newLines: LINES,
    header: "",
    lines,
  };
  const file: DiffFile = {
    status: "modified",
    path,
    oldPath: null,
    oldMode: "100644",
    newMode: "100644",
    oldId: null,
    newId: null,
    binary: false,
    tooLarge: false,
    hunksOmitted: false,
    generated: false,
    submodule: null,
    added: LINES / 2,
    removed: LINES / 2,
    hunks: [hunk],
    error: null,
  };
  const blob = (text: string) => ({
    content: { kind: "text", text },
    judgment: { highlight: true },
  });
  return {
    diff: JSON.stringify({ workTree: "/r", oldCommit: "a", newCommit: "b", files: [file] }),
    old: JSON.stringify(blob(`${before.join("\n")}\n`)),
    new: JSON.stringify(blob(`${after.join("\n")}\n`)),
  };
}
