// 열린 트랜잭션 확인(14-m3-plan.md M3-15, 결정 M3-D21): 수동 트랜잭션이 열린 콘솔 탭을 닫거나, 그런 탭이 있는 창을 닫거나 앱을
// 끝낼 때 [커밋] [롤백] [취소] 를 묻는다. 묻는 때는 작업 공간의 닫기 흐름(editor-workspace.tsx 의 `guardClose` — 미저장 확인 다음)이
// 정하고, 답에 따라 하는 일은 콘솔(console-store.ts 의 `settleClose`)이 한다. 여기는 묻기만 한다.
//
// M1 의 미저장 확인(unsaved-dialog.tsx)과 같은 틀(위쪽 시트 · 바깥 · Esc 는 [취소] · 떠 있는 동안 창의 키맵은 쉰다)이다. 처음
// 포커스는 [취소] 다 — 커밋도 롤백도 되돌릴 수 없어 Enter 한 번에 어느 쪽도 되지 않게(M2-20 위험 작업 확인과 같은 규칙). 오류 뒤
// 롤백만 되는 트랜잭션(`failed`)이면 [커밋] 을 끈다.

import * as Dialog from "@radix-ui/react-dialog";
import { type ReactNode, useCallback, useEffect, useRef, useState } from "react";

import { useKeymapPopup } from "../../keymap/keymap-provider";
import { ConnectionMark, type DbConnectionMarkInput } from "../safety/connection-mark";
import { useDbConsoleStrings } from "./strings";

const BUTTON =
  "rounded-sm border border-ui-border px-3 py-1 hover:bg-ui-hover disabled:opacity-50 disabled:hover:bg-transparent";

export type TransactionAnswer = "commit" | "rollback" | "cancel";

export interface TransactionQuestion {
  connection: DbConnectionMarkInput & { name: string };
  /** 오류 뒤 롤백만 되는 트랜잭션 — [커밋] 을 끈다. */
  failed: boolean;
}

function TransactionDialog({
  question,
  onAnswer,
}: {
  question: TransactionQuestion;
  onAnswer(answer: TransactionAnswer): void;
}) {
  const T = useDbConsoleStrings().close;
  useKeymapPopup(true);
  return (
    <Dialog.Root open onOpenChange={(open) => !open && onAnswer("cancel")}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content
          data-testid="db-transaction-dialog"
          className="sheet-top fixed left-1/2 flex w-[460px] max-w-[94vw] -translate-x-1/2 flex-col gap-3 rounded-b-md border border-t-0 border-ui-border bg-ui-panel px-4 py-3 text-ui-foreground shadow-lg"
        >
          <Dialog.Title className="text-sm font-semibold">
            {question.failed ? T.failedTitle : T.title}
          </Dialog.Title>
          <div className="flex items-center gap-1">
            <ConnectionMark {...question.connection} />
          </div>
          <Dialog.Description className="whitespace-pre-line text-ui-muted-foreground">
            {question.failed ? T.failedBody : T.body}
          </Dialog.Description>
          <div className="flex justify-end gap-2">
            <button
              type="button"
              autoFocus
              data-answer="cancel"
              className={BUTTON}
              onClick={() => onAnswer("cancel")}
            >
              {T.cancel}
            </button>
            <button
              type="button"
              data-answer="rollback"
              className={BUTTON}
              onClick={() => onAnswer("rollback")}
            >
              {T.rollback}
            </button>
            <button
              type="button"
              data-answer="commit"
              disabled={question.failed}
              className={BUTTON}
              onClick={() => onAnswer("commit")}
            >
              {T.commit}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/** 열린 트랜잭션을 묻는 함수와 그 대화상자. `element` 를 그린다. 한 번에 하나만 묻고, 주인이 사라지면 [취소] 로 끝난다. */
export function useTransactionPrompt(): {
  ask(question: TransactionQuestion): Promise<TransactionAnswer>;
  element: ReactNode;
} {
  const [asked, setAsked] = useState<{
    question: TransactionQuestion;
    answer(answer: TransactionAnswer): void;
  }>();
  const current = useRef<(answer: TransactionAnswer) => void>(undefined);
  useEffect(() => () => current.current?.("cancel"), []);
  const ask = useCallback(
    (question: TransactionQuestion) =>
      new Promise<TransactionAnswer>((resolve) => {
        if (current.current) return resolve("cancel");
        const answer = (value: TransactionAnswer) => {
          if (current.current !== answer) return;
          current.current = undefined;
          setAsked(undefined);
          resolve(value);
        };
        current.current = answer;
        setAsked({ question, answer });
      }),
    [],
  );
  return {
    ask,
    element: asked && <TransactionDialog question={asked.question} onAnswer={asked.answer} />,
  };
}
