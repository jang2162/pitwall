// 실행이 남은 채 프로젝트 창을 닫을 때의 확인(17-m6-plan M6-06 ⑥, 03-architecture.md 「프로세스 · 창 모델」): 「실행 중 N개 —
// 끝내고 닫기 / 취소」. 창은 프로젝트마다 하나라 그 창이 닫히면 Host 가 그 프로젝트의 실행을 끝낸다(`pw_run::runs::project_closed`,
// M6-02) — 그래서 「끝내고 닫기」 는 닫기를 막지 않을 뿐이고 끝내기는 Host 가 한다.
//
// - 묻는 때: 창의 닫기 요청(닫기 단추 · ⌘W · 앱 종료 — keymap/native.ts 의 `guardClose`). 그때 Host 에 `run.list` 로 다시 묻는다
//   (탭이 아직 모르는 실행 — 이벤트가 오는 중인 에이전트 실행 — 도 센다). Host 에 닿지 못하면 창의 탭이 아는 것으로 센다.
// - 실행마다 누가 띄웠는지 보인다(사람 · 에이전트와 그 클라이언트) — 에이전트가 돌리던 것을 모르고 끄지 않게.
// - 처음 포커스는 [취소]다. 바깥을 누르거나 Esc 로 닫아도 [취소]다. 떠 있는 동안 창의 키맵은 쉰다(창 팝업 규칙).

import type { RunInfo } from "@pitwall/rpc-client";
import * as Dialog from "@radix-ui/react-dialog";
import { Bot, User } from "lucide-react";
import { type ReactNode, useCallback, useRef, useState } from "react";

import { useKeymapPopup } from "../keymap/keymap-provider";
import { isLive, type RunClient, type RunTabs } from "./run-tabs";
import { useRunStrings } from "./strings";
import { TargetMachineLine } from "../checkout/machine-mark";

const BUTTON = "rounded-sm border border-ui-border px-3 py-1 hover:bg-ui-hover";

export function RunCloseDialog({
  runs,
  onAnswer,
}: {
  runs: readonly RunInfo[];
  onAnswer(close: boolean): void;
}) {
  const T = useRunStrings().close;
  useKeymapPopup(true);
  return (
    <Dialog.Root open onOpenChange={(open) => !open && onAnswer(false)}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content
          data-testid="run-close-dialog"
          className="sheet-top fixed left-1/2 flex w-[460px] max-w-[94vw] -translate-x-1/2 flex-col gap-3 rounded-b-md border border-t-0 border-ui-border bg-ui-panel px-4 py-3 text-ui-foreground shadow-lg"
        >
          <Dialog.Title className="text-sm font-semibold">{T.title(runs.length)}</Dialog.Title>
          <TargetMachineLine />
          <Dialog.Description className="text-ui-muted-foreground">{T.body}</Dialog.Description>
          <ul className="flex max-h-48 flex-col gap-1 overflow-y-auto text-[12px]">
            {runs.map((run) => {
              const agent = run.launchedBy.kind === "agent";
              return (
                <li
                  key={run.runId}
                  data-testid="run-close-item"
                  data-launched-by={run.launchedBy.kind}
                  className="flex items-center gap-2"
                >
                  {agent ? <Bot size={12} aria-hidden /> : <User size={12} aria-hidden />}
                  <span className="truncate font-medium">{run.name}</span>
                  <span className="text-ui-muted-foreground">
                    {run.launchedBy.kind === "agent" ? T.agent(run.launchedBy.client) : T.human}
                  </span>
                </li>
              );
            })}
          </ul>
          <div className="flex justify-end gap-2">
            <button type="button" autoFocus className={BUTTON} onClick={() => onAnswer(false)}>
              {T.cancel}
            </button>
            <button
              type="button"
              className={`${BUTTON} border-ui-accent`}
              onClick={() => onAnswer(true)}
            >
              {T.confirm}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/** 이 프로젝트에서 아직 도는 실행. Host 에 다시 묻고, 닿지 못하면 탭이 아는 것. */
async function liveRuns(
  tabs: RunTabs,
  client: RunClient | undefined,
  projectId: string | undefined,
): Promise<RunInfo[]> {
  if (!client || projectId === undefined) return tabs.live();
  try {
    const { runs } = await client.call("run.list", { projectId });
    return runs.filter(isLive);
  } catch (e) {
    console.warn("[run] could not list runs before closing", e);
    return tabs.live();
  }
}

/** 창을 닫기 전에 묻는 함수와 그 대화상자. `element` 를 창에 그린다. 한 번에 하나만 묻는다. */
export function useRunClosePrompt(
  tabs: RunTabs,
  client: RunClient | undefined,
  projectId: string | undefined,
): { confirm(): Promise<boolean>; element: ReactNode } {
  const [question, setQuestion] = useState<{
    runs: RunInfo[];
    answer(close: boolean): void;
  }>();
  const open = useRef(false);
  const confirm = useCallback(async () => {
    if (open.current) return false;
    open.current = true;
    try {
      const runs = await liveRuns(tabs, client, projectId);
      if (runs.length === 0) return true;
      return await new Promise<boolean>((resolve) =>
        setQuestion({
          runs,
          answer: (close) => {
            setQuestion(undefined);
            resolve(close);
          },
        }),
      );
    } finally {
      open.current = false;
    }
  }, [tabs, client, projectId]);
  return {
    confirm,
    element: question && <RunCloseDialog runs={question.runs} onAnswer={question.answer} />,
  };
}
