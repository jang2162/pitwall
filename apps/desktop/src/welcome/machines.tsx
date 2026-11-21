// 묶음(18-m7-plan M7-10, 08-remote-mobile.md 「묶음」)의 머신 표지 — 웰컴 · 프로젝트 스위처의 프로젝트 줄(10-ui.md 2.9 「웰컴 ·
// 프로젝트 스위처」). 줄은 아직 로컬 프로젝트(`project.list`)이고, 그 프로젝트가 다른 머신의 체크아웃과 묶였으면(`bundle.list`)
// 묶음의 머신을 글로 단다. 색만으로 가르지 않는다(10-ui.md 「접근성 · 기타」). 원격만인 묶음의 줄과 묶음 창은 창 행(M7-11)이 연다.

import type { ProjectBundle } from "@pitwall/rpc-client";

import { useAppStrings } from "../i18n/app-strings";
import { useRpcQuery } from "../rpc/host";

/** 이 머신의 머신 id(`pw_core::registry::bundle::LOCAL_MACHINE`). */
export const LOCAL_MACHINE = "local";

/**
 * 로컬 프로젝트 id → 그 프로젝트가 든 묶음의 머신 id(더한 순서). 다른 머신과 묶이지 않은 프로젝트는 빠진다 — 표지를 달지 않는다.
 */
export function machinesByProject(bundles: readonly ProjectBundle[]): Map<string, string[]> {
  const out = new Map<string, string[]>();
  for (const bundle of bundles) {
    const local = bundle.members.find((m) => m.machine === LOCAL_MACHINE);
    if (!local || bundle.members.length < 2) continue;
    out.set(
      local.projectId,
      bundle.members.map((m) => m.machine),
    );
  }
  return out;
}

/** 묶음 목록을 읽어 로컬 프로젝트별 머신 id 를 낸다. 읽지 못하면(옛 Host 등) 표지 없이 둔다. */
export function useProjectMachines(): Map<string, string[]> {
  const bundles = useRpcQuery("bundle.list", null);
  return machinesByProject(bundles.data ?? []);
}

/**
 * 머신 이름. 이 머신은 「이 컴퓨터」, 다른 머신은 아직 머신 id 그대로다 — Desktop 의 별명(`remote_hosts`)은 원격 호스트 행(M7-08)이
 * 들인다(메인 판정 M7-P19: 머신 이름은 Relay 가 댄 것이 아니라 Desktop 의 별명이다).
 */
export function useMachineName(): (machine: string) => string {
  const T = useAppStrings().machines;
  return (machine) => (machine === LOCAL_MACHINE ? T.thisMachine : machine);
}

/** 프로젝트 줄의 머신 표지. 머신이 없으면 그리지 않는다. */
export function MachineMarks({ machines }: { machines: readonly string[] | undefined }) {
  const T = useAppStrings().machines;
  const name = useMachineName();
  if (!machines?.length) return null;
  return (
    <span role="list" aria-label={T.label} className="flex flex-none items-center gap-1">
      {machines.map((m) => (
        <span
          key={m}
          role="listitem"
          data-machine={m}
          className="max-w-[120px] truncate rounded-sm border border-ui-border px-1 text-[11px] text-ui-muted-foreground"
        >
          {name(m)}
        </span>
      ))}
    </span>
  );
}
