// 키 동작의 입력 다이얼로그(TTL 수정 · 이름 바꾸기): 칸 하나 + [바꾸기] [취소]. 바탕은 M2-20 의 확인 부품과 같은 위쪽 시트다 —
// 바깥 · Esc 는 취소, 떠 있는 동안 창의 키맵은 쉰다, 한글 조합 중의 Enter 는 조합을 끝낼 뿐이고 조합 중의 Esc 는 닫지 않는다.
// 바꾸기 자체의 확인(등급 · 서버에 갈 명령)은 Host 의 `3001` 로 M3-19 다이얼로그가 따로 묻는다.

import * as Dialog from "@radix-ui/react-dialog";
import { useState } from "react";

import { BUTTON, SHEET } from "../../git/dialogs/git-dialogs";
import { useKeymapPopup } from "../../keymap/keymap-provider";
import { isComposingKey, keepOpenWhileComposing } from "../../ui/ime";

export interface KeyPromptQuestion {
  title: string;
  label: string;
  initial: string;
  apply: string;
  cancel: string;
  /** 칸의 값이 틀렸으면 그 까닭(그동안 [바꾸기] 가 꺼진다). */
  check(value: string): string | undefined;
}

export function KeyPromptDialog({
  question,
  onAnswer,
}: {
  question: KeyPromptQuestion;
  onAnswer(value: string | undefined): void;
}) {
  useKeymapPopup(true);
  const [value, setValue] = useState(question.initial);
  const problem = question.check(value);
  const submit = () => {
    if (problem === undefined) onAnswer(value);
  };
  return (
    <Dialog.Root open onOpenChange={(open) => !open && onAnswer(undefined)}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content
          data-testid="redis-key-prompt"
          className={SHEET}
          onEscapeKeyDown={keepOpenWhileComposing}
          aria-describedby={undefined}
        >
          <Dialog.Title className="text-sm font-semibold break-all">{question.title}</Dialog.Title>
          <label className="flex flex-col gap-1">
            <span>{question.label}</span>
            <input
              data-testid="redis-key-prompt-input"
              aria-label={question.label}
              aria-invalid={problem !== undefined}
              value={value}
              spellCheck={false}
              autoComplete="off"
              onChange={(e) => setValue(e.target.value)}
              onKeyDown={(e) => {
                if (e.key !== "Enter" || isComposingKey(e.nativeEvent)) return;
                e.preventDefault();
                submit();
              }}
              className="rounded-sm border border-ui-border bg-transparent px-2 py-1 font-mono outline-none select-text focus:border-ui-accent"
            />
            {problem !== undefined && value !== question.initial && (
              <span className="text-[12px] text-ui-accent">{problem}</span>
            )}
          </label>
          <div className="flex justify-end gap-2">
            <button
              type="button"
              data-action="cancel"
              className={BUTTON}
              onClick={() => onAnswer(undefined)}
            >
              {question.cancel}
            </button>
            <button
              type="button"
              data-action="apply"
              disabled={problem !== undefined}
              className={`${BUTTON} border-ui-accent`}
              onClick={submit}
            >
              {question.apply}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
