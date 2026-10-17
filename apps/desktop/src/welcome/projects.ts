// 웰컴 창의 화면 밖 규칙: 목록 순서, 후보의 이유, 경로 조각, 저장소 상태 배너, 폴더를 열 때 기존 프로젝트로 갈지.

import type { Project, Resolution, RootCandidate, StorageStatus } from "@pitwall/rpc-client";

import { STRINGS } from "./strings";

/** 즐겨찾기 → 최근에 연 것 → 연 적 없는 것(이름순). `project.list` 는 이름순이다. */
export function sortProjects(projects: readonly Project[]): Project[] {
  return [...projects].sort(
    (a, b) =>
      Number(b.favorite) - Number(a.favorite) ||
      (b.openedAt ?? -1) - (a.openedAt ?? -1) ||
      a.name.localeCompare(b.name, "ko"),
  );
}

/** 레지스트리의 Host 가 이 기기가 아니면 원격이다(지금은 언제나 `local`). */
export function isRemote(project: Project): boolean {
  return project.host !== "local";
}

/** 마지막 경로 조각. `/` 와 `\` 를 모두 구분자로 본다(Windows 경로). */
export function baseName(path: string): string {
  const trimmed = path.replace(/[/\\]+$/, "");
  const i = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return i < 0 ? trimmed : trimmed.slice(i + 1);
}

/** 상위 폴더. 조각이 없으면 그대로. */
export function parentDir(path: string): string {
  const trimmed = path.replace(/[/\\]+$/, "");
  const i = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return i <= 0 ? trimmed : trimmed.slice(0, i);
}

/** 후보 줄의 이유(10-ui §2.2 `.git · build.gradle.kts`). */
export function candidateReasons(candidate: RootCandidate): string {
  return [
    ...(candidate.worktreeMain ? [STRINGS.find.worktreeMain] : []),
    ...(candidate.vcs ? [".git"] : []),
    ...candidate.buildFiles,
  ].join(" · ");
}

export interface StorageBanner {
  text: string;
  /** 쓰기(만들기 · 제거)가 거절되는가. */
  readOnly: boolean;
}

/** 정상(`ok`)이 아니면 보여 줄 배너. `project.storage` 문서: 웰컴 화면은 정상이 아니면 `reason` 을 배너로 보여 준다. */
export function storageBanner(status: StorageStatus | undefined): StorageBanner | undefined {
  switch (status?.kind) {
    case undefined:
    case "ok":
      return undefined;
    case "recovered":
      return { text: STRINGS.storage.recovered(status.reason, status.backup), readOnly: false };
    case "readOnly":
      return { text: STRINGS.storage.readOnly(status.reason), readOnly: true };
    case "memory":
      return { text: STRINGS.storage.memory(status.reason), readOnly: true };
  }
}

/**
 * 고르거나 끌어다 놓은 폴더가 이미 등록된 프로젝트의 루트(또는 그 프로젝트의 미등록 워크트리에서 루트에 대응하는 폴더)면 그
 * 프로젝트로 연다. 아니면(하위 폴더 포함) 새 프로젝트를 만든다 — `project.create` 는 이미 루트인 폴더를 거절한다.
 */
export function existingProjectFor(
  resolution: Resolution | null,
): { projectId: string; worktree?: string } | undefined {
  if (!resolution) return undefined;
  if (resolution.worktree) {
    return resolution.path === resolution.worktree.root
      ? { projectId: resolution.project.id, worktree: resolution.worktree.top }
      : undefined;
  }
  return resolution.path === resolution.root ? { projectId: resolution.project.id } : undefined;
}

/** RPC 오류 등을 한 줄로. */
export function errorText(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === "object" && e !== null && "message" in e) return String(e.message);
  return String(e);
}
