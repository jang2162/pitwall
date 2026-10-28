// 커밋 툴 윈도우의 목록 모델 · hunk 묶음(13-m2-plan M2-16). 그룹은 진짜 `git status --porcelain` 의 줄(src/git/testing.ts)로 본다.

import type { GitDiff } from "@pitwall/rpc-client";
import { describe, expect, it } from "vitest";

import { buildDiffModel } from "../diff/diff-model";
import fixture from "../diff/fixtures/commit.json";
import { MERGE_PORCELAIN, PORCELAIN, porcelainEntries } from "../testing";
import {
  commitRows,
  groupEntries,
  itemCheck,
  selectItems,
  templateUnedited,
  unstagePaths,
  withoutTemplateComments,
} from "./commit-model";
import { stageGroups } from "./hunk-staging";
import stageFixture from "./fixtures/stage-commit.json";

const keys = (items: readonly { path: string }[]) => items.map((i) => i.path);

describe("groupEntries", () => {
  it("puts each porcelain state in the groups git's XY says", () => {
    const g = groupEntries(porcelainEntries(PORCELAIN));
    expect(keys(g.conflicts)).toEqual(["src/conflict.rs"]);
    expect(keys(g.staged)).toEqual([
      "src/added-deleted.rs",
      "src/added-modified.rs",
      "src/added.rs",
      "src/both.rs",
      "src/mod-deleted.rs",
      "src/new-name.rs",
      "src/staged-deleted.rs",
      "src/staged.rs",
    ]);
    expect(keys(g.changes)).toEqual([
      "src/added-deleted.rs",
      "src/added-modified.rs",
      "src/both.rs",
      "src/deleted.rs",
      "src/intent.rs",
      "src/mod-deleted.rs",
      "src/modified.rs",
      "src/type.rs",
    ]);
    expect(keys(g.untracked)).toEqual(["new/untracked.txt", "untracked.txt"]);
    // `MM` 은 두 그룹에 들고 변경 쪽 체크가 「부분」 이다.
    const both = g.changes.find((i) => i.path === "src/both.rs")!;
    expect(itemCheck(both)).toBe("mixed");
    expect(itemCheck(g.changes.find((i) => i.path === "src/modified.rs")!)).toBe("unchecked");
    expect(itemCheck(g.staged[0]!)).toBe("checked");
    // 이름 바뀜은 스테이징을 풀 때 옛 경로도 넘긴다.
    const renamed = g.staged.find((i) => i.path === "src/new-name.rs")!;
    expect(renamed.letter).toBe("R");
    expect(unstagePaths([renamed])).toEqual(["src/new-name.rs", "src/old-name.rs"]);
  });

  it("keeps every conflict kind in the conflicts group only", () => {
    const g = groupEntries(porcelainEntries(MERGE_PORCELAIN));
    expect(g.conflicts).toHaveLength(7);
    expect(g.conflicts.every((i) => i.letter === "U" && itemCheck(i) === "none")).toBe(true);
    const all = [...g.staged, ...g.changes, ...g.untracked].map((i) => i.path);
    for (const c of g.conflicts) expect(all).not.toContain(c.path);
  });

  it("drops empty groups and the files of collapsed ones; the stash header is always last", () => {
    const g = groupEntries(porcelainEntries("?? a.txt\n M b.txt"));
    const rows = commitRows(g, new Set(["untracked"]), []);
    expect(
      rows.map((r) => (r.kind === "file" ? r.item.key : r.kind === "group" ? r.group : r.kind)),
    ).toEqual(["changes", "changes:b.txt", "untracked", "stashHeader"]);
  });
});

it("selects with click, ⌘ and ⇧ in row order", () => {
  const rows = commitRows(groupEntries(porcelainEntries(" M a\n M b\n M c")), new Set(), []);
  let s = selectItems(rows, [], undefined, "changes:a", "single");
  s = selectItems(rows, s.selected, s.anchor, "changes:c", "range");
  expect(s.selected).toEqual(["changes:a", "changes:b", "changes:c"]);
  s = selectItems(rows, s.selected, s.anchor, "changes:b", "toggle");
  expect(s.selected).toEqual(["changes:a", "changes:c"]);
});

it("drops only the template's untouched comment lines", () => {
  const template = "Subject\n\n# Why?\n# Ticket:\n";
  expect(withoutTemplateComments("feat: x\n\n# Why?\n# Ticket: 12\n#7 fixed\n", template)).toBe(
    "feat: x\n\n# Ticket: 12\n#7 fixed\n",
  );
  expect(withoutTemplateComments("# keep\n", undefined)).toBe("# keep\n");
  // `core.commentChar` 가 `;` 면 `;` 줄만 템플릿 주석이다.
  expect(withoutTemplateComments("x\n; why\n# kept\n", "; why\n# kept\n", ";")).toBe("x\n# kept\n");
});

it("sees an unedited template the way git does (comments and surrounding blanks ignored)", () => {
  const template = "Subject\n\n# Why?\n";
  expect(templateUnedited(template, template)).toBe(true);
  expect(templateUnedited("Subject\r\n\r\n", template)).toBe(true);
  expect(templateUnedited("Subject changed\n\n# Why?\n", template)).toBe(false);
  expect(templateUnedited("anything", undefined)).toBe(false);
});

describe("stageGroups", () => {
  // diff 뷰가 칠하는 묶음(diff-model.ts)과 체크를 다는 묶음이 같은 차례 · 같은 줄이어야 묶음 체크가 그 묶음을 고른다.
  const files = [
    ...(fixture as unknown as { diff: GitDiff }).diff.files,
    ...(stageFixture as unknown as { steps: { diff: GitDiff }[] }).steps.flatMap(
      (s) => s.diff.files,
    ),
  ];
  it.each(files.map((f) => [f.path, f] as const))(
    "matches the diff view's groups (%s)",
    (_, file) => {
      const model = buildDiffModel(file);
      const groups = stageGroups(file);
      expect(groups.map((g) => [g.oldFrom, g.newFrom])).toEqual(
        model.groups.map((g) => [g.oldFrom, g.newFrom]),
      );
      for (const g of groups) {
        const hunk = file.hunks[g.hunk]!;
        expect(g.lines.every((i) => hunk.lines[i]!.kind !== "context")).toBe(true);
        expect([...g.added.keys()].every((n) => model.added.has(n))).toBe(true);
        expect([...g.removed.keys()].every((n) => model.deleted.has(n))).toBe(true);
      }
      expect(groups.reduce((n, g) => n + g.lines.length, 0)).toBe(
        model.added.size + model.deleted.size,
      );
    },
  );
});
