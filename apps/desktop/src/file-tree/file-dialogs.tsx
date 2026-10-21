// 파일 조작의 대화상자(M1-21): 이름 묻기(새 파일 · 새 폴더 · 이름 바꾸기)와 확인(삭제 · 영구 삭제). 무엇을 묻고 답대로 무엇을
// 하는지는 file-ops.ts 가 정하고, 여기는 묻기만 한다. 바깥을 누르거나 Esc 로 닫으면 취소다. 떠 있는 동안 창의 키맵은 쉰다(창 팝업
// 규칙, keymap-provider.tsx). 한글 조합 중의 Enter 는 조합을 끝낼 뿐 확인이 아니다.

import * as Dialog from "@radix-ui/react-dialog";
import { type ReactNode, useCallback, useRef, useState } from "react";

import { useKeymapPopup } from "../keymap/keymap-provider";
import { isComposingKey } from "../ui/ime";
import type { ConfirmQuestion, NameQuestion } from "./file-ops";
import { useFileTreeStrings } from "./strings";

const BUTTON = "rounded-sm border border-ui-border px-3 py-1 hover:bg-ui-hover disabled:opacity-50";
const SHEET =
  "sheet-top fixed left-1/2 flex w-[420px] max-w-[94vw] -translate-x-1/2 flex-col gap-3 rounded-b-md border border-t-0 border-ui-border bg-ui-panel px-4 py-3 text-ui-foreground shadow-lg";

/** 처음 고를 범위: 이름 바꾸기는 마지막 `.` 앞까지(`.gitignore` 처럼 점으로 시작하면 전부). */
export function stemRange(name: string): [number, number] {
  const dot = name.lastIndexOf(".");
  return [0, dot > 0 ? dot : name.length];
}

function NameDialog({
  question,
  onAnswer,
}: {
  question: NameQuestion;
  onAnswer(name: string | undefined): void;
}) {
  const T = useFileTreeStrings().ops;
  useKeymapPopup(true);
  const [name, setName] = useState(question.initial);
  // 처음에는 빈 이름을 탓하지 않는다 — 친 뒤에 지웠을 때부터.
  const [touched, setTouched] = useState(false);
  const problem = question.validate(name);
  const submit = () => {
    setTouched(true);
    if (!problem) onAnswer(name);
  };
  return (
    <Dialog.Root open onOpenChange={(open) => !open && onAnswer(undefined)}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content data-testid="file-name-dialog" className={SHEET}>
          <Dialog.Title className="text-sm font-semibold">{question.title}</Dialog.Title>
          {question.hint ? (
            <Dialog.Description className="text-ui-muted-foreground">
              {question.hint}
            </Dialog.Description>
          ) : (
            <Dialog.Description className="sr-only">{question.title}</Dialog.Description>
          )}
          <input
            aria-label={T.name.label}
            aria-invalid={touched && problem !== undefined}
            autoFocus
            value={name}
            spellCheck={false}
            onFocus={(e) => {
              if (!question.selectStem) return;
              const [from, to] = stemRange(question.initial);
              e.currentTarget.setSelectionRange(from, to);
            }}
            onChange={(e) => {
              setName(e.target.value);
              setTouched(true);
            }}
            onKeyDown={(e) => {
              if (e.key !== "Enter" || isComposingKey(e.nativeEvent)) return;
              e.preventDefault();
              submit();
            }}
            className="rounded-sm border border-ui-border bg-transparent px-2 py-1 outline-none focus:border-ui-accent"
          />
          {touched && problem && (
            <p role="alert" className="text-[12px] text-ui-accent">
              {problem}
            </p>
          )}
          <div className="flex justify-end gap-2">
            <button type="button" className={BUTTON} onClick={() => onAnswer(undefined)}>
              {T.cancel}
            </button>
            <button
              type="button"
              disabled={touched && problem !== undefined}
              className={`${BUTTON} border-ui-accent`}
              onClick={submit}
            >
              {T.ok}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function ConfirmDialog({
  question,
  onAnswer,
}: {
  question: ConfirmQuestion;
  onAnswer(ok: boolean): void;
}) {
  const T = useFileTreeStrings().ops;
  useKeymapPopup(true);
  return (
    <Dialog.Root open onOpenChange={(open) => !open && onAnswer(false)}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content data-testid="file-confirm-dialog" className={SHEET}>
          <Dialog.Title className="text-sm font-semibold">{question.title}</Dialog.Title>
          <Dialog.Description className="whitespace-pre-line text-ui-muted-foreground">
            {question.body}
          </Dialog.Description>
          {question.warning && (
            <p role="alert" className="whitespace-pre-line text-ui-accent">
              {question.warning}
            </p>
          )}
          <div className="flex justify-end gap-2">
            <button
              type="button"
              autoFocus={question.danger}
              className={BUTTON}
              onClick={() => onAnswer(false)}
            >
              {T.cancel}
            </button>
            <button
              type="button"
              autoFocus={!question.danger}
              className={`${BUTTON} border-ui-accent ${question.danger ? "text-ui-accent" : ""}`}
              onClick={() => onAnswer(true)}
            >
              {question.confirm}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

type Asked =
  | { kind: "name"; question: NameQuestion; answer(name: string | undefined): void }
  | { kind: "confirm"; question: ConfirmQuestion; answer(ok: boolean): void };

/** 이름 묻기 · 확인을 묻는 함수와 그 대화상자. `element` 를 그린다. 한 번에 하나만 묻는다 — 떠 있는 동안의 질문은 곧바로 취소다. */
export function useFileDialogs(): {
  askName(question: NameQuestion): Promise<string | undefined>;
  confirm(question: ConfirmQuestion): Promise<boolean>;
  element: ReactNode;
} {
  const [asked, setAsked] = useState<Asked>();
  const open = useRef(false);
  const ask = useCallback(
    <T,>(make: (answer: (value: T) => void) => Asked, cancelled: T) =>
      new Promise<T>((resolve) => {
        if (open.current) return resolve(cancelled);
        open.current = true;
        setAsked(
          make((value) => {
            open.current = false;
            setAsked(undefined);
            resolve(value);
          }),
        );
      }),
    [],
  );
  const askName = useCallback(
    (question: NameQuestion) =>
      ask<string | undefined>((answer) => ({ kind: "name", question, answer }), undefined),
    [ask],
  );
  const confirm = useCallback(
    (question: ConfirmQuestion) =>
      ask<boolean>((answer) => ({ kind: "confirm", question, answer }), false),
    [ask],
  );
  const element =
    asked?.kind === "name" ? (
      <NameDialog question={asked.question} onAnswer={asked.answer} />
    ) : asked?.kind === "confirm" ? (
      <ConfirmDialog question={asked.question} onAnswer={asked.answer} />
    ) : undefined;
  return { askName, confirm, element };
}
