// git 문서(spec.ts)가 읽는 길: 이 창의 Host 연결의 `git.show` · `git.diff` · `git.blob`(M2-04 그대로), 작업 트리 쪽은 `fs.read`,
// 언어는 `lang.detect`, 강조는 구문 분석 서비스. 프로젝트 창의 `GitDocuments`(git-documents.tsx)가 창마다 하나를 걸고, 탭의 문서
// 컴포넌트가 읽는다 — 창마다 웹뷰가 따로라 모듈 상태로 둔다(src/git/git-status.ts 와 같다). 시험은 가짜를 건다.
//
// 모두 읽기다. 미신뢰 프로젝트에서도 Host 가 gix 로만 읽는다(13-m2-plan 결정 M2-D8) — 이 길은 git CLI 를 띄우지 않는다.

import type {
  DiffFile,
  DiffSide,
  FsRead,
  GitBlob,
  GitBlobParams,
  GitDiff,
  GitDiffParams,
  GitShow,
} from "@pitwall/rpc-client";
import { create } from "zustand";

import type { StaticSyntaxSource } from "../diff/revision-syntax";

type Scoped<P> = Omit<P, "projectId" | "worktree">;

export interface GitDocumentSource {
  show(rev: string, worktree: string | undefined): Promise<GitShow>;
  diff(params: Scoped<GitDiffParams>, worktree: string | undefined): Promise<GitDiff>;
  blob(params: Scoped<GitBlobParams>, worktree: string | undefined): Promise<GitBlob>;
  /** 작업 트리의 파일(절대 경로) 전체. */
  readFile(path: string, worktree: string | undefined): Promise<FsRead>;
  /** 저장소의 작업 트리 꼭대기(절대 경로, `git.repo`). 저장소가 없으면 오류. */
  workTree(worktree: string | undefined): Promise<string>;
  /** 언어(Linguist 이름). 플레인 텍스트면 `null`. */
  detect(path: string, head: string, worktree: string | undefined): Promise<string | null>;
  syntax?: StaticSyntaxSource;
}

export const gitDocumentSource = create<{ source: GitDocumentSource | undefined }>(() => ({
  source: undefined,
}));

/** 한쪽 내용. 텍스트가 아니면(바이너리 · 너무 큼 · 읽지 못함) 까닭. */
export type SideText =
  | { kind: "text"; text: string; highlight: boolean }
  | { kind: "binary" }
  | { kind: "tooLarge"; size: number }
  | { kind: "missing" };

/** 언어 판별에 넘기는 앞부분(`lang.detect` 는 앞 50 KiB 까지만 본다). */
const DETECT_HEAD = 50 * 1024;

function fromContent(r: Pick<GitBlob, "content" | "judgment">): SideText {
  if (!r.content || !r.judgment) return { kind: "missing" };
  if (r.judgment.binary || r.content.kind !== "text") return { kind: "binary" };
  return { kind: "text", text: r.content.text, highlight: r.judgment.highlight };
}

/** 리비전(`rev` 커밋 해시, `null` 은 index)의 파일 내용. */
export async function revisionText(
  source: GitDocumentSource,
  rev: string | null,
  path: string,
  worktree: string | undefined,
): Promise<SideText> {
  const blob = await source.blob({ rev, path }, worktree);
  if (blob.tooLarge) return { kind: "tooLarge", size: blob.size };
  return fromContent(blob);
}

/** 그 쪽(옛 · 새)의 종류. `empty` 는 빈 트리(루트 커밋의 부모) · 그쪽에 파일이 없음. */
export type SideKind =
  | { kind: "commit"; rev: string }
  | { kind: "index" }
  | { kind: "workTree" }
  | { kind: "empty" };

/** `git.diff` 의 결과와 요청한 짝에서 두 쪽의 종류. 커밋 쪽은 결과의 해시(첫 부모를 고른 것 포함)다. */
export function sidesOf(
  diff: GitDiff,
  from: DiffSide | undefined,
  to: DiffSide,
): [SideKind, SideKind] {
  const newSide: SideKind =
    to.kind === "commit" ? { kind: "commit", rev: diff.newCommit ?? to.rev } : { kind: to.kind };
  let oldSide: SideKind;
  if (diff.oldCommit) oldSide = { kind: "commit", rev: diff.oldCommit };
  else if (to.kind === "workTree" || from?.kind === "index") oldSide = { kind: "index" };
  else oldSide = { kind: "empty" };
  return [oldSide, newSide];
}

async function sideText(
  source: GitDocumentSource,
  side: SideKind,
  path: string,
  workTree: string,
  worktree: string | undefined,
): Promise<SideText> {
  switch (side.kind) {
    case "empty":
      return { kind: "text", text: "", highlight: true };
    case "commit":
      return revisionText(source, side.rev, path, worktree);
    case "index":
      return revisionText(source, null, path, worktree);
    case "workTree": {
      const r = await source.readFile(`${workTree}/${path}`, worktree);
      return fromContent(r);
    }
  }
}

export interface FileTexts {
  old: SideText;
  new: SideText;
  language: string | null;
}

/** 파일 하나의 두 쪽 내용과 언어. 더한 파일의 옛 쪽 · 지운 파일의 새 쪽은 빈 텍스트다. */
export async function loadFileTexts(
  source: GitDocumentSource,
  diff: GitDiff,
  file: DiffFile,
  sides: [SideKind, SideKind],
  worktree: string | undefined,
): Promise<FileTexts> {
  const empty: SideText = { kind: "text", text: "", highlight: true };
  const [oldText, newText] = await Promise.all([
    file.status === "added"
      ? empty
      : sideText(source, sides[0], file.oldPath ?? file.path, diff.workTree, worktree),
    file.status === "deleted"
      ? empty
      : sideText(source, sides[1], file.path, diff.workTree, worktree),
  ]);
  const sample = file.status === "deleted" ? oldText : newText;
  let language: string | null = null;
  if (sample.kind === "text" && sample.highlight) {
    try {
      language = await source.detect(
        `${diff.workTree}/${file.path}`,
        sample.text.slice(0, DETECT_HEAD),
        worktree,
      );
    } catch (e) {
      console.warn("[git] language detection failed", e);
    }
  }
  return { old: oldText, new: newText, language };
}
