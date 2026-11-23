// 다른 머신에서 도는 것이 있는 탭 · 창을 닫을 때의 확인(18-m7-plan M7-14 ⑤ · M7-09 ④ — 10-ui.md 2.9 「끊긴 머신」): [끝내기] ·
// [두고 닫기] · [취소]. 처음 포커스는 [취소]다. 바깥을 누르거나 Esc 로 닫아도 [취소]다. 떠 있는 동안 창의 키맵은 쉰다.
//
// - [끝내기]: 그 머신에서 끝낸다 — 셸은 `terminal.close`(스트림을 끊는 것만으로는 그 머신이 셸을 떼어 둘 뿐이다), 실행은 `run.stop`.
// - [두고 닫기]: 셸을 그 머신에 떼어 둔다(`terminal.detach`). 떼어 둔 셸은 머신당 상한이 있어 넘으면 가장 오래된 것부터 끝난다 —
//   그 머신이 끝낼 것을 알리면(이 프로젝트의 수 · 다른 프로젝트의 수) 한 번 더 묻고 `endOldest` 로 다시 부른다. 미신뢰 프로젝트의
//   셸은 연결 수명이라 그 머신이 거절한다 — 그 까닭을 알리고 탭은 그대로 둔다. 실행은 Host 의 것이라 그대로 돈다.
// - 문장에 그 머신의 이름이 든다(M7-11 의 `TargetMachineLine`).

import * as Dialog from "@radix-ui/react-dialog";
import { type ReactNode, useCallback, useRef, useState } from "react";

import { LOCAL_MACHINE } from "../checkout/checkout";
import { TargetMachineLine, TargetMachineProvider } from "../checkout/machine-mark";
import { useKeymapPopup } from "../keymap/keymap-provider";
import { useProjectStrings } from "../layout/strings";
import { useNotify } from "../notifications/notifications";
import { errorText } from "../run/run-configs";
import type { RunTabs } from "../run/run-tabs";
import { KeepLimit, type TerminalTabs } from "./terminal-tabs";

const BUTTON = "rounded-sm border border-ui-border px-3 py-1 hover:bg-ui-hover";

export type RemoteCloseAnswer = "end" | "keep" | "cancel";

export interface RemoteCloseQuestion {
  title: string;
  /** 대상 머신(한 머신이면). 여러 머신이면 없다. */
  machine?: string;
  /** [두고 닫기] 를 보이는가(떼어 둘 셸이 없으면 — 실행만이면 — 빼지 않고 그대로 둔다: 실행은 Host 에서 돈다). */
  keep?: boolean;
}

export function RemoteCloseDialog({
  question,
  onAnswer,
}: {
  question: RemoteCloseQuestion;
  onAnswer(answer: RemoteCloseAnswer): void;
}) {
  const T = useProjectStrings().terminal.remoteClose;
  useKeymapPopup(true);
  return (
    <Dialog.Root open onOpenChange={(open) => !open && onAnswer("cancel")}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        {/* 늦게 오는 포커스(터미널이 다음 프레임에 잡는 포커스 등)는 답이 아니다 — 바깥 포커스로 닫지 않는다. */}
        <Dialog.Content
          onFocusOutside={(e) => e.preventDefault()}
          data-testid="remote-close-dialog"
          className="sheet-top fixed left-1/2 flex w-[480px] max-w-[94vw] -translate-x-1/2 flex-col gap-3 rounded-b-md border border-t-0 border-ui-border bg-ui-panel px-4 py-3 text-ui-foreground shadow-lg"
        >
          <Dialog.Title className="text-sm font-semibold">{question.title}</Dialog.Title>
          {question.machine !== undefined && (
            <TargetMachineProvider machine={question.machine}>
              <TargetMachineLine />
            </TargetMachineProvider>
          )}
          <Dialog.Description className="text-ui-muted-foreground">{T.body}</Dialog.Description>
          <div className="flex justify-end gap-2">
            <button
              type="button"
              data-answer="end"
              className={BUTTON}
              onClick={() => onAnswer("end")}
            >
              {T.end}
            </button>
            {question.keep !== false && (
              <button
                type="button"
                data-answer="keep"
                className={BUTTON}
                onClick={() => onAnswer("keep")}
              >
                {T.keep}
              </button>
            )}
            <button
              type="button"
              data-answer="cancel"
              autoFocus
              className={`${BUTTON} border-ui-accent`}
              onClick={() => onAnswer("cancel")}
            >
              {T.cancel}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/** [두고 닫기] 가 상한에 걸렸을 때: 「끝내고 두기」 · 「취소」(처음 포커스 [취소]). */
export function KeepLimitDialog({
  limit,
  onAnswer,
}: {
  limit: KeepLimit;
  onAnswer(confirm: boolean): void;
}) {
  const T = useProjectStrings().terminal.remoteClose;
  useKeymapPopup(true);
  return (
    <Dialog.Root open onOpenChange={(open) => !open && onAnswer(false)}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content
          onFocusOutside={(e) => e.preventDefault()}
          data-testid="keep-limit-dialog"
          className="sheet-top fixed left-1/2 flex w-[460px] max-w-[94vw] -translate-x-1/2 flex-col gap-3 rounded-b-md border border-t-0 border-ui-border bg-ui-panel px-4 py-3 text-ui-foreground shadow-lg"
        >
          <Dialog.Title className="text-sm font-semibold">{T.limitTitle(limit.limit)}</Dialog.Title>
          <Dialog.Description className="text-ui-muted-foreground">
            {T.limitBody(limit.wouldEnd.length, limit.otherProjects)}
          </Dialog.Description>
          <div className="flex justify-end gap-2">
            <button
              type="button"
              data-answer="confirm"
              className={BUTTON}
              onClick={() => onAnswer(true)}
            >
              {T.limitConfirm}
            </button>
            <button
              type="button"
              data-answer="cancel"
              autoFocus
              className={`${BUTTON} border-ui-accent`}
              onClick={() => onAnswer(false)}
            >
              {T.cancel}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

type Asked =
  | { kind: "close"; question: RemoteCloseQuestion; answer(a: RemoteCloseAnswer): void }
  | { kind: "limit"; limit: KeepLimit; answer(confirm: boolean): void };

/**
 * 묻는 함수 둘과 그 대화상자. `element` 를 창에 그린다. 한 번에 하나만 묻는다 — 떠 있는 동안 온 질문은 [취소] 다.
 */
export function useRemoteCloseAsk(): {
  ask(question: RemoteCloseQuestion): Promise<RemoteCloseAnswer>;
  askLimit(limit: KeepLimit): Promise<boolean>;
  element: ReactNode;
} {
  const [asked, setAsked] = useState<Asked>();
  const busy = useRef(false);
  const ask = useCallback(
    (question: RemoteCloseQuestion) =>
      new Promise<RemoteCloseAnswer>((resolve) => {
        if (busy.current) return resolve("cancel");
        busy.current = true;
        setAsked({
          kind: "close",
          question,
          answer: (a) => {
            busy.current = false;
            setAsked(undefined);
            resolve(a);
          },
        });
      }),
    [],
  );
  const askLimit = useCallback(
    (limit: KeepLimit) =>
      new Promise<boolean>((resolve) => {
        if (busy.current) return resolve(false);
        busy.current = true;
        setAsked({
          kind: "limit",
          limit,
          answer: (confirm) => {
            busy.current = false;
            setAsked(undefined);
            resolve(confirm);
          },
        });
      }),
    [],
  );
  const element =
    asked?.kind === "close" ? (
      <RemoteCloseDialog question={asked.question} onAnswer={asked.answer} />
    ) : asked?.kind === "limit" ? (
      <KeepLimitDialog limit={asked.limit} onAnswer={asked.answer} />
    ) : null;
  return { ask, askLimit, element };
}

/**
 * [두고 닫기] 하나: 상한에 걸리면 한 번 더 묻고 `endOldest` 로. 사람이 취소하면 거짓(탭은 그대로). 그 머신이 거절하면 그 오류를
 * 던진다(부르는 쪽이 알린다 — 탭은 그대로).
 */
export async function keepWithLimit(
  keep: (endOldest: boolean) => Promise<void>,
  askLimit: (limit: KeepLimit) => Promise<boolean>,
): Promise<boolean> {
  try {
    await keep(false);
    return true;
  } catch (e) {
    if (!(e instanceof KeepLimit)) throw e;
    if (!(await askLimit(e))) return false;
    await keep(true);
    return true;
  }
}

/**
 * 창을 닫기 전에 묻는 함수와 그 대화상자: 다른 머신에서 도는 셸 · 실행이 있으면 [끝내기] · [두고 닫기] · [취소]. 이 머신의 실행은
 * 창이 닫히면 Host 가 끝낸다(run-close.tsx 가 따로 묻는다). [두고 닫기] 는 셸을 그 머신에 떼어 두고 실행은 그대로 둔다(그 머신 Host 의
 * 것). `element` 를 창에 그린다.
 */
export function useRemoteWindowClose(
  terminals: TerminalTabs,
  runs: RunTabs,
): { confirm(): Promise<boolean>; element: ReactNode } {
  const T = useProjectStrings().terminal.remoteClose;
  const notify = useNotify();
  const { ask, askLimit, element } = useRemoteCloseAsk();
  const confirm = useCallback(async () => {
    const shells = terminals.remoteAlive();
    const remoteRuns = runs.liveTabs().filter((t) => t.machine !== LOCAL_MACHINE);
    if (shells.length === 0 && remoteRuns.length === 0) return true;
    const machines = new Set([
      ...shells.map((id) => terminals.instance(id)?.machine ?? LOCAL_MACHINE),
      ...remoteRuns.map((t) => t.machine),
    ]);
    const answer = await ask({
      title: T.titleWindow(shells.length, remoteRuns.length),
      ...(machines.size === 1 && { machine: [...machines][0]! }),
      // 실행만 돌아도 [두고 닫기] 가 있다 — 실행은 그 머신 Host 에서 그대로 돈다(리뷰 F4).
    });
    if (answer === "cancel") return false;
    if (answer === "end") {
      await Promise.all([
        ...shells.map((id) => terminals.end(id)),
        ...remoteRuns.map((t) =>
          runs.stop(t.key).catch((e: unknown) => console.warn("[run] not stopped", e)),
        ),
      ]);
      return true;
    }
    for (const id of shells) {
      try {
        if (!(await keepWithLimit((endOldest) => terminals.keep(id, endOldest), askLimit)))
          return false;
      } catch (e) {
        notify({ level: "error", title: T.keepFailed(errorText(e)) });
        return false;
      }
    }
    return true;
  }, [terminals, runs, ask, askLimit, notify, T]);
  return { confirm, element };
}
