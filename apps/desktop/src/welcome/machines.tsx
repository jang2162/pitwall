// 묶음(18-m7-plan M7-10, 08-remote-mobile.md 「묶음」)의 머신 표지 — 웰컴 · 프로젝트 스위처의 프로젝트 줄(10-ui.md 2.9 「웰컴 ·
// 프로젝트 스위처」). 줄은 아직 로컬 프로젝트(`project.list`)이고, 그 프로젝트가 다른 머신의 체크아웃과 묶였으면(`bundle.list`)
// 묶음의 머신을 글로 단다. 표지 부품은 checkout/machine-mark.tsx(M7-11). 원격만인 묶음의 줄은 M7-11b 가 연다.

import type { ProjectBundle } from "@pitwall/rpc-client";

import { useRpcQuery } from "../rpc/host";

export { LOCAL_MACHINE, MachineMarks, useMachineName } from "../checkout/machine-mark";
import { LOCAL_MACHINE } from "../checkout/checkout";

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
