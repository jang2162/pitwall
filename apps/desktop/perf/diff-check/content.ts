// 검사용 diff(perf/diff-check/diff-check.tsx).

import type { DiffFile, DiffHunk, DiffLine } from "@pitwall/rpc-client";

/** 묶음 셋(빼기만 · 바뀜 · 더하기만)이 97줄마다 되풀이되는 3,000줄 파일. hunk 는 하나로 둔다(칠하는 줄은 같다). */
export function content() {
  const before: string[] = [];
  const after: string[] = [];
  const lines: DiffLine[] = [];
  let o = 0;
  let n = 0;
  const push = (kind: DiffLine["kind"], text: string) =>
    lines.push({
      kind,
      text,
      oldLine: kind === "added" ? null : o,
      newLine: kind === "removed" ? null : n,
      noNewline: false,
    });
  for (let i = 0; i < 3000; i++) {
    const t = `line ${i}`;
    before.push(t);
    const k = i % 97;
    if (k === 10) {
      o++;
      push("removed", t);
      continue;
    }
    if (k === 20) {
      o++;
      push("removed", t);
      n++;
      after.push(`${t} x`);
      push("added", `${t} x`);
      continue;
    }
    if (k === 30)
      for (let j = 0; j < 5; j++) {
        n++;
        after.push(`new ${i}.${j}`);
        push("added", `new ${i}.${j}`);
      }
    o++;
    n++;
    after.push(t);
    push("context", t);
  }
  const hunk: DiffHunk = { oldStart: 1, oldLines: o, newStart: 1, newLines: n, header: "", lines };
  const file: DiffFile = {
    status: "modified",
    path: "x.txt",
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
    added: 0,
    removed: 0,
    hunks: [hunk],
    error: null,
  };
  return {
    file,
    oldText: `${before.join("\n")}\n`,
    newText: `${after.join("\n")}\n`,
    language: null,
  };
}
