// 에디터 영역의 git 문서(docs/design/10-ui.md 「에디터 영역 문서 타입」 의 Diff · 커밋 상세 행, 13-m2-plan M2-15)의 꼴. 에디터 탭
// (src/editor/editor-tabs.ts `EDITOR_PANEL`)의 params `git` 칸이 이것이다 — 이 칸이 있는 탭은 파일 문서가 아니다: 문서 모델을 잡지
// 않고(`fs.*` 를 읽지 않는다), 경로를 쓰는 탭 동작(경로 복사 · 저장 · 뷰어 바꾸기 · 외부 도구 문맥)에서 빠지고, 워크트리 탭 묶음에
// 담지 않는다(다시 시작하면 되살리지 않는다). ⌘W · 미리보기 · 고정 · 탭 전환 · 분할은 파일 탭과 같다.
//
// 탭의 `path` 는 문서 키(`gitDocumentKey`)다 — `git:` 으로 시작해 실제 경로(절대 경로)와 겹치지 않는다. 같은 키의 탭이 있으면 새로
// 열지 않고 그 탭을 앞으로 가져온다. 리비전은 연 때 커밋 해시로 풀어 키에 넣는다(`HEAD` 가 나중에 움직여도 같은 탭이 같은 커밋이다).
//
// 이 꼴은 다른 행이 쓴다(M2-14 의 「툴 윈도우를 에디터 탭으로 키우기」, M2-16 의 미리보기, M2-18) — 종류를 더할 수는 있지만 있는
// 종류의 칸 이름 · 뜻 · 키를 바꾸지 않는다.

import type { DiffSide } from "@pitwall/rpc-client";

export type GitDocumentSpec =
  /** 커밋 상세: 메시지 · 부모 · ref + 바뀐 파일 트리 + 고른 파일의 diff(첫 부모와, F5-3). `rev` 는 커밋 해시. */
  | { kind: "commit"; rev: string }
  /** 두 커밋 비교(F5-4): `a` → `b` 의 바뀐 파일과 diff. 둘 다 커밋 해시. */
  | { kind: "compare"; a: string; b: string }
  /** 리비전의 파일 읽기 전용(F5-19, 결정 M2-D5). `rev` 는 커밋 해시, `path` 는 저장소 꼭대기 기준(`/`). */
  | { kind: "revision"; rev: string; path: string }
  /**
   * 파일 하나의 diff(`git.diff` 의 짝 그대로 — 커밋 툴 윈도우의 미리보기 · 파일 히스토리). `path` 는 새 경로(저장소 꼭대기 기준),
   * 이름이 바뀌었으면 `oldPath`.
   */
  | { kind: "diff"; from?: DiffSide; to: DiffSide; path: string; oldPath?: string }
  /**
   * 충돌 중인 파일 하나의 3-way 머지(M2-19, 결정 M2-D4): 왼쪽 ours · 가운데 결과 · 오른쪽 theirs. `path` 는 저장소 꼭대기 기준(`/`).
   * 내용은 연 때의 `git.conflict.get` 이고, 저장하면 `git.conflict.resolve` 로 푼다(src/git/merge/merge-document.tsx).
   */
  | { kind: "merge"; path: string };

const sideKey = (side: DiffSide | undefined) =>
  side === undefined ? "-" : side.kind === "commit" ? side.rev : side.kind;

/** 탭의 `path` 자리에 두는 문서 키. */
export function gitDocumentKey(spec: GitDocumentSpec): string {
  switch (spec.kind) {
    case "commit":
      return `git:commit:${spec.rev}`;
    case "compare":
      return `git:compare:${spec.a}..${spec.b}`;
    case "revision":
      return `git:revision:${spec.rev}:${spec.path}`;
    case "diff":
      return `git:diff:${sideKey(spec.from)}..${sideKey(spec.to)}:${spec.path}`;
    case "merge":
      return `git:merge:${spec.path}`;
  }
}

/**
 * 사람이 읽는 문서 설명(탭 전환기의 둘째 칸 · 탭 툴팁 — 문서 키 `git:…` 를 보이지 않는다). 해시는 전체, 경로는 저장소 꼭대기 기준.
 */
export function gitDocumentDetail(spec: GitDocumentSpec): string {
  switch (spec.kind) {
    case "commit":
      return spec.rev;
    case "compare":
      return `${spec.a}..${spec.b}`;
    case "revision":
      return `${spec.path} @ ${spec.rev}`;
    case "diff":
      return spec.oldPath ? `${spec.oldPath} → ${spec.path}` : spec.path;
    case "merge":
      return spec.path;
  }
}

/** 탭 params 의 `git` 칸이 문서의 꼴인가(옛 · 깨진 params 를 거른다). */
export function isGitDocumentSpec(value: unknown): value is GitDocumentSpec {
  if (typeof value !== "object" || value === null) return false;
  const v = value as Record<string, unknown>;
  const str = (k: string) => typeof v[k] === "string" && v[k] !== "";
  switch (v.kind) {
    case "commit":
      return str("rev");
    case "compare":
      return str("a") && str("b");
    case "revision":
      return str("rev") && str("path");
    case "diff":
      return str("path") && typeof v.to === "object" && v.to !== null;
    case "merge":
      return str("path");
    default:
      return false;
  }
}
