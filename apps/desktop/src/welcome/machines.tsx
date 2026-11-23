// 웰컴 · 프로젝트 스위처의 줄은 묶음 하나다(18-m7-plan M7-11b ②, 10-ui.md 2.9 「웰컴 · 프로젝트 스위처」, 08-remote-mobile.md 「묶음」).
// 사람에게는 묶음이 「프로젝트」 다 — 로컬 프로젝트 하나뿐인 암묵 묶음, 다른 머신의 체크아웃과 묶인 묶음, 원격만인 묶음이 모두 한
// 줄이다. 줄을 고르면 그 묶음의 창(`project-<묶음 id>` — 열린 로컬 프로젝트 창이 있으면 그 창, src-tauri/src/launch.rs
// `open_bundle_as`)을 연다. 다른 머신과 묶였거나 원격만인 줄에는 묶음의 머신을 글로 단다(표지 부품은 checkout/machine-mark.tsx).
//
// 줄의 목록 정보(이름 · 즐겨찾기 · 태그 · 연 시각)는 암묵 묶음이면 그 프로젝트의 것(`project.list` — 웰컴 창이 바꾸고 곧바로 다시
// 읽는다), 행 있는 묶음이면 묶음 행의 것이다(만들 때 로컬 프로젝트에서 복사 — 바꾸는 RPC 는 아직 없다). 「열었음」 은 로컬 멤버의
// 프로젝트에 적히므로 연 시각은 둘 중 늦은 것이다. 로컬 멤버의 프로젝트(`project`)가 있는 줄만 폴더 보기 · 외부 도구 · 목록에서 빼기가
// 된다 — 원격만인 줄은 열기만.

import type { Project, ProjectBundle } from "@pitwall/rpc-client";
import { useMemo } from "react";

import { useRpcQuery } from "../rpc/host";

export { LOCAL_MACHINE, MachineMarks, useMachineName } from "../checkout/machine-mark";
import { LOCAL_MACHINE } from "../checkout/checkout";

/** 웰컴 · 스위처의 줄 하나(묶음 하나). */
export interface BundleRow {
  /** 창을 열 때 넘기는 id — 묶음 id(암묵 묶음이면 그 로컬 프로젝트 id). */
  id: string;
  name: string;
  favorite: boolean;
  tags: readonly string[];
  openedAt: number | null;
  /** 로컬 멤버의 프로젝트. 원격만인 묶음 · 목록을 아직 읽지 못했으면 없다. */
  project: Project | undefined;
  /** 표지로 다는 머신(더한 순서). 이 머신 하나뿐인 줄은 비어 있다. */
  machines: readonly string[];
}

function implicitRow(project: Project): BundleRow {
  return {
    id: project.id,
    name: project.name,
    favorite: project.favorite,
    tags: project.tags,
    openedAt: project.openedAt,
    project,
    machines: [],
  };
}

/**
 * 묶음 목록과 로컬 프로젝트 목록 → 줄. 묶음을 읽지 못했으면(옛 Host · 읽는 중) 로컬 프로젝트마다 한 줄이다. 묶음 목록에 아직 없는
 * 로컬 프로젝트(두 목록을 읽는 사이 만든 것)도 한 줄로 둔다. 순서는 즐겨찾기 → 최근에 연 것 → 연 적 없는 것(이름순).
 */
export function bundleRows(
  bundles: readonly ProjectBundle[] | undefined,
  projects: readonly Project[],
): BundleRow[] {
  const byId = new Map(projects.map((p) => [p.id, p]));
  const rows: BundleRow[] = [];
  const seen = new Set<string>();
  for (const bundle of bundles ?? []) {
    const localId = bundle.members.find((m) => m.machine === LOCAL_MACHINE)?.projectId;
    const project = localId === undefined ? undefined : byId.get(localId);
    if (localId !== undefined) seen.add(localId);
    // 지워진 로컬 프로젝트의 암묵 묶음(목록을 읽는 사이)은 줄이 아니다.
    if (bundle.implicit && !project) continue;
    if (bundle.implicit && project) {
      rows.push(implicitRow(project));
      continue;
    }
    const opened = Math.max(bundle.openedAt ?? -1, project?.openedAt ?? -1);
    rows.push({
      id: bundle.id,
      name: bundle.name,
      favorite: bundle.favorite,
      tags: bundle.tags,
      openedAt: opened < 0 ? null : opened,
      project,
      machines: bundle.members.map((m) => m.machine),
    });
  }
  for (const project of projects) if (!seen.has(project.id)) rows.push(implicitRow(project));
  return rows.sort(
    (a, b) =>
      Number(b.favorite) - Number(a.favorite) ||
      (b.openedAt ?? -1) - (a.openedAt ?? -1) ||
      a.name.localeCompare(b.name, "ko"),
  );
}

/** 그 창(창 쿼리의 id — 로컬 프로젝트 id 또는 묶음 id)의 줄인가. */
export function isWindowRow(row: BundleRow, windowId: string | undefined): boolean {
  return windowId !== undefined && (row.id === windowId || row.project?.id === windowId);
}

/** 묶음 단위 줄(`bundle.list` · `project.list` 를 읽는다). 묶음을 읽지 못하면 로컬 프로젝트 줄만. */
export function useBundleRows() {
  const projects = useRpcQuery("project.list", null);
  const bundles = useRpcQuery("bundle.list", null);
  const rows = useMemo(
    () => bundleRows(bundles.data, projects.data ?? []),
    [bundles.data, projects.data],
  );
  return { rows, projects };
}
