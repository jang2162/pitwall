// 승인 계획의 파일 변경(`PlanChange` 의 `file` — 앞뒤 내용)을 diff 뷰(git/diff/diff-view.tsx)가 그리는 꼴로 바꾼다(16-m5-plan
// M5-14 — 「파일 diff 는 기존 diff 화면 부품」). 계획에는 git 의 hunk 가 없으므로 에디터 거터와 같은 줄 diff(git/gutter/line-diff.ts)로
// 묶음을 만들고, 묶음마다 문맥 없는 hunk 하나를 만든다 — 바뀌지 않은 긴 구간은 diff 뷰가 접는다.
//
// 앞뒤 내용은 에이전트가 정한 글이다(쓸 내용). 보이지 않는 글자는 두 쪽 모두 같은 넓은 규칙(agent-text.ts)으로 `⟨U+XXXX⟩` 표지로
// 바꾼 뒤 견준다 — 0폭 글자 하나만 다른 줄도 「바뀐 줄」 로 보이고 그 글자가 드러난다.

import type { DiffFile, DiffHunk } from "@pitwall/rpc-client";

import { showInvisible } from "../db/grid/values";
import type { DiffContent } from "../git/diff/diff-view";
import { diffLines, keysOf, splitLines } from "../git/gutter/line-diff";

const shown = (text: string) => showInvisible(text, { keepEmoji: false });

/** 파일 하나의 앞뒤 내용(없던 파일이면 `before`, 지우는 파일이면 `after` 가 `null`)을 diff 뷰의 내용으로. */
export function planDiff(path: string, before: string | null, after: string | null): DiffContent {
  const oldText = shown(before ?? "");
  const newText = shown(after ?? "");
  const a = splitLines(oldText, undefined);
  const b = splitLines(newText, undefined);
  const groups = diffLines(keysOf(a), keysOf(b));
  let added = 0;
  let removed = 0;
  const hunks: DiffHunk[] = groups.map((g) => {
    const oldLines = g.oldTo - g.oldFrom;
    const newLines = g.newTo - g.newFrom;
    removed += oldLines;
    added += newLines;
    const lines: DiffHunk["lines"] = [];
    for (let n = g.oldFrom; n < g.oldTo; n++)
      lines.push({
        kind: "removed",
        text: a.lines[n - 1]!,
        oldLine: n,
        newLine: null,
        noNewline: !a.eol && n === a.lines.length,
      });
    for (let n = g.newFrom; n < g.newTo; n++)
      lines.push({
        kind: "added",
        text: b.lines[n - 1]!,
        oldLine: null,
        newLine: n,
        noNewline: !b.eol && n === b.lines.length,
      });
    return {
      // 줄이 없는 쪽의 시작은 그 앞 줄 번호다(git 의 `@@ -0,0 +1 @@`).
      oldStart: oldLines === 0 ? g.oldFrom - 1 : g.oldFrom,
      oldLines,
      newStart: newLines === 0 ? g.newFrom - 1 : g.newFrom,
      newLines,
      header: "",
      lines,
    };
  });
  const file: DiffFile = {
    status: before === null ? "added" : after === null ? "deleted" : "modified",
    path,
    oldPath: null,
    oldMode: null,
    newMode: null,
    oldId: null,
    newId: null,
    binary: false,
    tooLarge: false,
    hunksOmitted: false,
    generated: false,
    submodule: null,
    added,
    removed,
    hunks,
    error: null,
  };
  return { file, oldText, newText, language: null };
}
