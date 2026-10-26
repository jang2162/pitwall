// git 상태의 글자 표지와 툴팁(파일 트리 행 · 에디터 탭이 함께 쓴다, 13-m2-plan M2-12). 색만으로 가르지 않는다(10-ui.md 「접근성 · 기타」
// 절) — 글자는 색과 함께 보이고, 스크린 리더는 글자 대신 상태 이름(「수정됨」)을 읽는다.

import type { ProjectStrings } from "../layout/strings";
import type { GitDecoration, GitLetter } from "./git-status";

/** `useProjectStrings().git.status`. 부르는 쪽이 한 번 받아 넘긴다 — 행마다 문자열 묶음을 다시 만들지 않게. */
export type GitStatusStrings = ProjectStrings["git"]["status"];

/** 글자가 읽히는 이름. */
export function letterName(letter: GitLetter, T: GitStatusStrings): string {
  switch (letter) {
    case "A":
      return T.change.added;
    case "C":
      return T.change.copied;
    case "M":
      return T.change.modified;
    case "T":
      return T.change.typeChanged;
    case "D":
      return T.change.deleted;
    case "R":
      return T.change.renamed;
    case "?":
      return T.change.untracked;
    case "U":
      return T.conflicted;
  }
}

/** 툴팁: 충돌이면 그 종류, 아니면 두 쪽(스테이징됨 · 작업 트리) 중 있는 것. 폴더 색이면 「아래에 바뀐 파일이 있다」. */
export function gitStatusTitle(decoration: GitDecoration, T: GitStatusStrings): string {
  if (decoration.conflict) return T.conflict[decoration.conflict];
  const parts: string[] = [];
  if (decoration.staged) parts.push(T.staged(T.change[decoration.staged]));
  if (decoration.unstaged) parts.push(T.unstaged(T.change[decoration.unstaged]));
  return parts.length > 0 ? parts.join(" · ") : T.folder;
}

/** 글자 표지. 폴더 색(글자 없음)이면 아무것도 그리지 않는다. */
export function GitStatusMark({
  decoration,
  strings: T,
  className = "",
}: {
  decoration: GitDecoration | undefined;
  strings: GitStatusStrings;
  className?: string;
}) {
  if (!decoration?.letter) return null;
  return (
    <span
      role="img"
      aria-label={letterName(decoration.letter, T)}
      data-git-letter={decoration.letter}
      className={`flex-none font-mono text-[11px] ${className}`}
    >
      {decoration.letter}
    </span>
  );
}

/** `text-git-<tone>` 클래스(styles.css). */
export function gitToneClass(decoration: GitDecoration | undefined): string {
  return decoration ? GIT_TONE_CLASS[decoration.tone] : "";
}

// Tailwind 는 소스의 글자 그대로를 클래스로 찾으므로 조립하지 않고 다 적는다.
const GIT_TONE_CLASS: Record<GitDecoration["tone"], string> = {
  added: "text-git-added",
  modified: "text-git-modified",
  deleted: "text-git-deleted",
  renamed: "text-git-renamed",
  untracked: "text-git-untracked",
  ignored: "text-git-ignored",
  conflicted: "text-git-conflicted",
};
