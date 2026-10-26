// 시험용: 진짜 `git status --porcelain` 의 출력과 그것을 `git.status` 의 항목으로 옮기는 것(git-status.test.ts · 창 시험이 함께 쓴다).

import type { GitChange, GitConflict, GitStatusEntry } from "@pitwall/rpc-client";

/**
 * 진짜 git 의 출력이다: 격리한 HOME · `GIT_CONFIG_GLOBAL=/dev/null` · `GIT_CONFIG_NOSYSTEM=1` 아래 임시 저장소에서 각 상태를 만들고
 * (충돌 merge · 스테이징 · intent-to-add · 파일 → 심링크 …) `git status --porcelain=v1 --untracked-files=all` 로 뽑았다(git 2.54).
 * 오른쪽은 그 줄에서 보여야 하는 글자 — porcelain 의 `X` · `Y` 중 하나다.
 */
export const PORCELAIN = `AD src/added-deleted.rs
AM src/added-modified.rs
A  src/added.rs
MM src/both.rs
UU src/conflict.rs
 D src/deleted.rs
 A src/intent.rs
MD src/mod-deleted.rs
 M src/modified.rs
R  src/old-name.rs -> src/new-name.rs
D  src/staged-deleted.rs
M  src/staged.rs
 T src/type.rs
?? new/untracked.txt
?? untracked.txt`;

export const EXPECTED_LETTERS: Record<string, string> = {
  "src/added-deleted.rs": "D",
  "src/added-modified.rs": "A",
  "src/added.rs": "A",
  "src/both.rs": "M",
  "src/conflict.rs": "U",
  "src/deleted.rs": "D",
  "src/intent.rs": "A",
  "src/mod-deleted.rs": "D",
  "src/modified.rs": "M",
  "src/new-name.rs": "R",
  "src/staged-deleted.rs": "D",
  "src/staged.rs": "M",
  "src/type.rs": "T",
  "new/untracked.txt": "?",
  "untracked.txt": "?",
};

const CHANGE: Record<string, GitChange> = {
  A: "added",
  M: "modified",
  D: "deleted",
  R: "renamed",
  C: "copied",
  T: "typeChanged",
};

const CONFLICT: Record<string, GitConflict> = {
  DD: "bothDeleted",
  AU: "addedByUs",
  UD: "deletedByThem",
  UA: "addedByThem",
  DU: "deletedByUs",
  AA: "bothAdded",
  UU: "bothModified",
};

/** porcelain v1 의 줄 → `git.status` 의 항목(crates/pw-git 의 `GitChange` · `GitConflict` 는 porcelain 의 글자 하나씩이다). */
export function porcelainEntries(text: string): GitStatusEntry[] {
  return text.split("\n").map((line) => {
    const xy = line.slice(0, 2);
    const [path, origPath] = line.slice(3).split(" -> ").reverse() as [string, string?];
    const base = { path, origPath: origPath ?? null, staged: null, unstaged: null, conflict: null };
    if (CONFLICT[xy]) return { ...base, conflict: CONFLICT[xy] };
    if (xy === "??") return { ...base, unstaged: "untracked" };
    return { ...base, staged: CHANGE[xy[0]!] ?? null, unstaged: CHANGE[xy[1]!] ?? null };
  });
}
