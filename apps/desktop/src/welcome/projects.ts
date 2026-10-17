// 웰컴 창의 화면 밖 규칙: 목록 순서, 후보의 이유, 경로 조각, 저장소 상태 배너, 폴더를 열 때 기존 프로젝트로 갈지.

import {
  ERROR_CODE,
  type Project,
  type Resolution,
  type RootCandidate,
  RpcCallError,
  type StorageStatus,
} from "@pitwall/rpc-client";

import type { FindProjectRequest, OpenProjectWindowArgs } from "./native";
import type { WelcomeStrings } from "./strings";

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
export function candidateReasons(candidate: RootCandidate, T: WelcomeStrings): string {
  return [
    ...(candidate.worktreeMain ? [T.find.worktreeMain] : []),
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
export function storageBanner(
  status: StorageStatus | undefined,
  T: WelcomeStrings,
): StorageBanner | undefined {
  switch (status?.kind) {
    case undefined:
    case "ok":
      return undefined;
    case "recovered":
      return { text: T.storage.recovered(status.reason, status.backup), readOnly: false };
    case "readOnly":
      return { text: T.storage.readOnly(status.reason), readOnly: true };
    case "memory":
      return { text: T.storage.memory(status.reason), readOnly: true };
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

/**
 * 경로(`pitwall <path>` · 끌어다 놓은 파일)를 `projectId` 의 창으로 열 때 넘길 인자. `resolution` 은 그 경로를 `project.resolve`
 * 한 결과다. 프로젝트가 그 경로를 포함하지 않으면(S2 에서 다른 폴더를 골랐다 등) `undefined` — 창만 열고 파일은 넘기지 않는다.
 * 경로가 프로젝트 루트 · 워크트리 꼭대기 · 워크트리 안 루트 자리면 폴더라 파일로 넘기지 않는다. 그 밖의 하위 폴더는 여기서
 * 가릴 수 없다(JS 는 디렉터리인지 모른다) — `open_project_window` 가 디렉터리 `file` 을 버린다(src-tauri, M0-10).
 */
export function openArgsFor(
  resolution: Resolution | null,
  projectId: string,
  request: FindProjectRequest,
): OpenProjectWindowArgs | undefined {
  if (resolution?.project.id !== projectId) return undefined;
  const { worktree, path } = resolution;
  const folder = path === resolution.root || path === worktree?.root || path === worktree?.top;
  return {
    projectId,
    ...(worktree && { worktree: worktree.top }),
    ...(!folder && {
      file: path,
      ...(request.line != null && { line: request.line }),
      ...(request.col != null && { col: request.col }),
    }),
  };
}

/** 레지스트리 파일을 쓸 수 없어 거절된 쓰기(`STORAGE_UNAVAILABLE`, docs/design/03-architecture.md §5-0). */
export function isStorageUnavailable(e: unknown): boolean {
  return e instanceof RpcCallError && e.code === ERROR_CODE.STORAGE_UNAVAILABLE;
}

/** RPC 오류 등을 한 줄로. */
export function errorText(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === "object" && e !== null && "message" in e) return String(e.message);
  return String(e);
}
