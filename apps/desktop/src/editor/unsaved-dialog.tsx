// 미저장 확인(12-m1-plan M1-13): 탭 · 창을 닫거나 앱을 끝내면 잃는 미저장 편집이 있을 때 [저장] [저장 안 함] [취소] 를 묻는다.
// 묻는 때와 답에 따라 하는 일은 작업 공간(editor-workspace.tsx 의 `closeTabs` · `confirmWindowClose`)이 정하고, 여기는 묻기만
// 한다. 바깥을 누르거나 Esc 로 닫으면 [취소] 다. 떠 있는 동안 창의 키맵은 쉰다(창 팝업 규칙, keymap-provider.tsx).

import * as Dialog from "@radix-ui/react-dialog";
import { type ReactNode, useCallback, useRef, useState } from "react";

import { useKeymapPopup } from "../keymap/keymap-provider";
import { useProjectStrings } from "../layout/strings";
import { baseName } from "./editor-tabs";
import type { UnsavedAnswer } from "./editor-workspace";

const BUTTON = "rounded-sm border border-ui-border px-3 py-1 hover:bg-ui-hover";

function UnsavedDialog({
  paths,
  onAnswer,
}: {
  paths: readonly string[];
  onAnswer(answer: UnsavedAnswer): void;
}) {
  const T = useProjectStrings().editor.unsavedDialog;
  useKeymapPopup(true);
  return (
    <Dialog.Root open onOpenChange={(open) => !open && onAnswer("cancel")}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content
          data-testid="unsaved-dialog"
          className="sheet-top fixed left-1/2 flex w-[460px] max-w-[94vw] -translate-x-1/2 flex-col gap-3 rounded-b-md border border-t-0 border-ui-border bg-ui-panel px-4 py-3 text-ui-foreground shadow-lg"
        >
          <Dialog.Title className="text-sm font-semibold">{T.title}</Dialog.Title>
          <Dialog.Description className="whitespace-pre-line text-ui-muted-foreground">
            {T.body(...paths.map(baseName))}
          </Dialog.Description>
          <div className="flex justify-end gap-2">
            <button type="button" className={BUTTON} onClick={() => onAnswer("cancel")}>
              {T.cancel}
            </button>
            <button type="button" className={BUTTON} onClick={() => onAnswer("discard")}>
              {T.discard}
            </button>
            <button
              type="button"
              autoFocus
              className={`${BUTTON} border-ui-accent`}
              onClick={() => onAnswer("save")}
            >
              {T.save}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/** 미저장 확인을 묻는 함수와 그 대화상자. `element` 를 창에 그린다. 한 번에 하나만 묻는다. */
export function useUnsavedPrompt(): {
  ask(paths: readonly string[]): Promise<UnsavedAnswer>;
  element: ReactNode;
} {
  const [question, setQuestion] = useState<{
    paths: readonly string[];
    answer(answer: UnsavedAnswer): void;
  }>();
  const open = useRef(false);
  const ask = useCallback(
    (paths: readonly string[]) =>
      new Promise<UnsavedAnswer>((resolve) => {
        if (open.current) return resolve("cancel");
        open.current = true;
        setQuestion({
          paths,
          answer: (answer) => {
            open.current = false;
            setQuestion(undefined);
            resolve(answer);
          },
        });
      }),
    [],
  );
  return {
    ask,
    element: question && <UnsavedDialog paths={question.paths} onAnswer={question.answer} />,
  };
}
