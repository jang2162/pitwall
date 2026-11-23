// 머신 표지(18-m7-plan M7-11 ③ — 「공유 부품의 주인」 표의 「머신 표지 부품」). 어느 머신인지 늘 보이게 한다(08 「창과 활성
// 체크아웃」): 활성 체크아웃이 다른 머신이면 타이틀바 · 상태바에, 활성 체크아웃과 다른 머신의 탭은 탭 머리에, 위험 작업의 확인
// 다이얼로그는 문장에 머신 이름을 넣는다. **색만으로 가르지 않는다** — 표지는 늘 글(머신 이름)이다(10-ui.md 「접근성 · 기타」).
//
// 머신 이름은 Desktop 의 별명이다(메인 판정 M7-P19 — Relay 가 댄 이름이 아니다): 이 머신은 「이 컴퓨터」, 다른 머신은 그 머신 id 를
// 기억한 원격 호스트 항목의 이름(`remote.hosts.list` — 설정의 별명이나 `~/.ssh/config` 의 호스트 이름). 모르면 머신 id 그대로다.
// 이름은 창의 로컬 연결로 읽는다(활성 체크아웃이 다른 머신이어도 — 그 머신의 Relay 에는 `remote.*` 가 없다).

import { createContext, type ReactNode, useContext } from "react";

import { useAppStrings } from "../i18n/app-strings";
import { useRpcQueryOn } from "../rpc/host";
import { LOCAL_MACHINE } from "./checkout";
import { hostNamesOf, useLocalHost } from "./checkout-hosts";

export { LOCAL_MACHINE };

/** 머신 id → 사람이 보는 이름. */
export function useMachineName(): (machine: string) => string {
  const T = useAppStrings().machines;
  const names = hostNamesOf(
    useRpcQueryOn(useLocalHost(), "remote.hosts.list", null, { retry: false }).data,
  );
  return (machine) => (machine === LOCAL_MACHINE ? T.thisMachine : (names.get(machine) ?? machine));
}

/** 머신 하나의 표지(글). `machine` 이 없으면 그리지 않는다 — 그때는 이름을 읽지 않는다(Host · 쿼리 문맥 없이 그려지는 자리). */
export function MachineMark({
  machine,
  className = "",
}: {
  machine: string | undefined;
  className?: string;
}) {
  return machine === undefined ? null : <NamedMark machine={machine} className={className} />;
}

function NamedMark({ machine, className }: { machine: string; className: string }) {
  const T = useAppStrings().machines;
  const text = useMachineName()(machine);
  return (
    <span
      data-machine={machine}
      title={T.markTitle(text)}
      aria-label={T.markTitle(text)}
      className={`max-w-[120px] flex-none truncate rounded-sm border border-ui-border px-1 text-[11px] text-ui-muted-foreground ${className}`}
    >
      {text}
    </span>
  );
}

/** 프로젝트 줄의 머신 표지 목록(웰컴 · 스위처). 머신이 없으면 그리지 않는다. */
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

/**
 * 위험 작업 확인의 대상 머신(④). 활성 체크아웃 · 그 작업의 체크아웃이 다른 머신이면 그 머신 id, 이 머신이면 `undefined`.
 * 확인 다이얼로그는 [`TargetMachineLine`] 으로 문장에 넣는다. 공급자 밖(웰컴 · 설정 · 로컬만인 시험)은 이 머신이다.
 */
const TargetMachineContext = createContext<string | undefined>(undefined);

export function TargetMachineProvider({
  machine,
  children,
}: {
  machine: string;
  children: ReactNode;
}) {
  return (
    <TargetMachineContext value={machine === LOCAL_MACHINE ? undefined : machine}>
      {children}
    </TargetMachineContext>
  );
}

export function useTargetMachine(): string | undefined {
  return useContext(TargetMachineContext);
}

/** 확인 다이얼로그의 대상 머신 문장(「<머신> 의 체크아웃에서 한다」). 이 머신이면 그리지 않는다. */
export function TargetMachineLine() {
  const machine = useTargetMachine();
  return machine === undefined ? null : <TargetMachineText machine={machine} />;
}

function TargetMachineText({ machine }: { machine: string }) {
  const T = useAppStrings().machines;
  const name = useMachineName();
  return (
    <p data-testid="target-machine" data-machine={machine} className="text-[12px] font-semibold">
      {T.onMachine(name(machine))}
    </p>
  );
}
