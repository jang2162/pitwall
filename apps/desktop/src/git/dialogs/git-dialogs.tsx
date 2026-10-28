// 위험 작업 다이얼로그 부품(13-m2-plan.md M2-20, 결정 M2-D6 ①): 확인(위험 표시 · 잃을 변경 목록 · git 원문)과 브랜치 이름 묻기(만들기 ·
// 이름 바꾸기, 「만든 뒤 체크아웃」). 무엇을 묻고 답대로 무엇을 부를지는 부르는 쪽(branch-ops · log-ops · undo · 푸시)이 정하고 여기는
// 묻기만 한다. 바깥을 누르거나 Esc 로 닫으면 취소다. 떠 있는 동안 창의 키맵은 쉰다. 한글 조합 중의 Enter 는 조합을 끝낼 뿐이다.
//
// - 잃을 변경은 `git.resetPreview` 의 목록 그대로(경로순)를 30줄까지 보이고 나머지는 수로 적는다. 지문이 `null` 이면(잃을 폴더 아래가
//   너무 많아 Host 가 셈하지 않았다) 「확인할 수 없음」 을 보이고 확인 단추를 끈다 — 그 reset 은 할 수 없다.
// - 위험(`danger`)이면 머리에 위험 표시를 두고 처음 포커스는 취소다(Enter 한 번에 덮지 않게).

import * as Dialog from "@radix-ui/react-dialog";
import type { GitLostChanges, GitStatusEntry } from "@pitwall/rpc-client";
import { TriangleAlert } from "lucide-react";
import { type ReactNode, useCallback, useRef, useState } from "react";

import { useKeymapPopup } from "../../keymap/keymap-provider";
import { isComposingKey } from "../../ui/ime";
import { type GitDialogStrings, useGitDialogStrings } from "./strings";

export const BUTTON =
  "rounded-sm border border-ui-border px-3 py-1 hover:bg-ui-hover disabled:opacity-50";
export const SHEET =
  "sheet-top fixed left-1/2 flex max-h-[80vh] w-[480px] max-w-[94vw] -translate-x-1/2 flex-col gap-3 rounded-b-md border border-t-0 border-ui-border bg-ui-panel px-4 py-3 text-ui-foreground shadow-lg";

/** 목록에 보이는 잃을 항목 수. 넘는 것은 「그리고 n개 더」. */
export const LOST_SHOWN = 30;

/** 확인 하나. */
export interface GitConfirm {
  title: string;
  body?: string;
  /** 본문 아래 경고(함께 사라지는 것 — 저장하지 않은 머지 탭 등). */
  warning?: string;
  /** 그대로 보일 원문(git 의 stderr 등). */
  detail?: string;
  /** 잃을 변경(`git.resetPreview`). `fingerprint` 가 `null` 이면 확인할 수 없다 — 확인 단추가 꺼진다. */
  lost?: { changes: GitLostChanges; fingerprint: string | null };
  confirm: string;
  /** 되돌리기 어려운 작업(위험 표시, 처음 포커스는 취소). */
  danger?: boolean;
}

/** 브랜치 이름 묻기. */
export interface BranchNameQuestion {
  title: string;
  hint?: string;
  initial: string;
  confirm: string;
  /** 이미 있는 로컬 브랜치 이름. */
  taken: ReadonlySet<string>;
  /** 「만든 뒤 체크아웃」 칸의 처음 값. 없으면 칸이 없다. */
  checkout?: boolean;
}

export interface BranchNameAnswer {
  name: string;
  checkout: boolean;
}

function entryLine(entry: GitStatusEntry): string {
  return entry.origPath ? `${entry.origPath} → ${entry.path}` : entry.path;
}

/** 잃을 변경의 줄(사라지는 변경 → 덮이는 추적 안 된 것). */
export function lostEntries(lost: GitLostChanges): string[] {
  return [...lost.changes.map(entryLine), ...lost.overwritten];
}

/**
 * 브랜치 이름으로 쓸 수 없으면 그 이유(git `check-ref-format --branch` 의 흔한 규칙만 — 나머지는 Host 가 같은 규칙으로 다시 보고
 * git 이 원문으로 거절한다).
 */
export function branchNameProblem(
  T: GitDialogStrings["branch"],
  name: string,
  taken: ReadonlySet<string>,
): string | undefined {
  if (name.trim() === "") return T.empty;
  if (
    /[\s~^:?*[\\]/.test(name) ||
    [...name].some((c) => c.charCodeAt(0) < 0x20 || c.charCodeAt(0) === 0x7f) ||
    name.startsWith("-") ||
    name.startsWith("/") ||
    name.endsWith("/") ||
    name.endsWith(".") ||
    name.endsWith(".lock") ||
    name.includes("..") ||
    name.includes("//") ||
    name.includes("@{") ||
    name === "@" ||
    name === "HEAD"
  )
    return T.invalid;
  if (taken.has(name)) return T.taken;
  return undefined;
}

/** 잃을 변경 목록. 너무 많으면 앞 [`LOST_SHOWN`] 줄과 수. */
export function LostList({
  lost,
  T,
}: {
  lost: NonNullable<GitConfirm["lost"]>;
  T: GitDialogStrings;
}) {
  if (lost.fingerprint === null)
    return (
      <p role="alert" data-testid="git-lost-unverifiable" className="text-ui-accent">
        {T.lost.unverifiable}
      </p>
    );
  const lines = lostEntries(lost.changes);
  if (lines.length === 0) return <p className="text-ui-muted-foreground">{T.lost.nothing}</p>;
  return (
    <div className="flex min-h-0 flex-col gap-1">
      <p>{T.lost.count(lines.length)}</p>
      <ul
        data-testid="git-lost-list"
        className="min-h-0 overflow-auto rounded-sm border border-ui-border px-2 py-1 font-mono text-[12px]"
      >
        {lines.slice(0, LOST_SHOWN).map((line) => (
          <li key={line} className="truncate" title={line}>
            {line}
          </li>
        ))}
        {lines.length > LOST_SHOWN && (
          <li className="text-ui-muted-foreground">{T.lost.more(lines.length - LOST_SHOWN)}</li>
        )}
      </ul>
    </div>
  );
}

export function ConfirmDialog({
  question,
  onAnswer,
}: {
  question: GitConfirm;
  onAnswer(ok: boolean): void;
}) {
  const T = useGitDialogStrings();
  useKeymapPopup(true);
  const blocked = question.lost?.fingerprint === null;
  return (
    <Dialog.Root open onOpenChange={(open) => !open && onAnswer(false)}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content
          data-testid="git-confirm-dialog"
          data-danger={question.danger ? "true" : undefined}
          className={SHEET}
        >
          <Dialog.Title className="flex items-center gap-2 text-sm font-semibold">
            {question.danger && (
              <TriangleAlert size={16} aria-label={T.common.danger} className="text-ui-accent" />
            )}
            {question.title}
          </Dialog.Title>
          <Dialog.Description className={question.body ? "whitespace-pre-line" : "sr-only"}>
            {question.body ?? question.title}
          </Dialog.Description>
          {question.warning && (
            <p role="alert" className="whitespace-pre-line text-ui-accent">
              {question.warning}
            </p>
          )}
          {question.detail && (
            <pre
              data-testid="git-confirm-detail"
              className="max-h-40 overflow-auto rounded-sm border border-ui-border px-2 py-1 text-[12px] whitespace-pre-wrap"
            >
              {question.detail}
            </pre>
          )}
          {question.lost && <LostList lost={question.lost} T={T} />}
          <div className="flex justify-end gap-2">
            <button
              type="button"
              autoFocus={question.danger || blocked}
              className={BUTTON}
              onClick={() => onAnswer(false)}
            >
              {T.common.cancel}
            </button>
            <button
              type="button"
              autoFocus={!question.danger && !blocked}
              disabled={blocked}
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

function BranchNameDialog({
  question,
  onAnswer,
}: {
  question: BranchNameQuestion;
  onAnswer(answer: BranchNameAnswer | undefined): void;
}) {
  const T = useGitDialogStrings();
  useKeymapPopup(true);
  const [name, setName] = useState(question.initial);
  const [checkout, setCheckout] = useState(question.checkout ?? false);
  // 처음에는 빈 이름을 탓하지 않는다 — 친 뒤에 지웠을 때부터.
  const [touched, setTouched] = useState(false);
  const problem = branchNameProblem(T.branch, name, question.taken);
  const submit = () => {
    setTouched(true);
    if (!problem) onAnswer({ name, checkout });
  };
  return (
    <Dialog.Root open onOpenChange={(open) => !open && onAnswer(undefined)}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content data-testid="git-branch-dialog" className={SHEET}>
          <Dialog.Title className="text-sm font-semibold">{question.title}</Dialog.Title>
          <Dialog.Description className={question.hint ? "text-ui-muted-foreground" : "sr-only"}>
            {question.hint ?? question.title}
          </Dialog.Description>
          <input
            aria-label={T.branch.name}
            aria-invalid={touched && problem !== undefined}
            autoFocus
            value={name}
            spellCheck={false}
            autoComplete="off"
            onFocus={(e) => e.currentTarget.select()}
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
          {question.checkout !== undefined && (
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={checkout}
                onChange={(e) => setCheckout(e.target.checked)}
              />
              {T.branch.checkout}
            </label>
          )}
          <div className="flex justify-end gap-2">
            <button type="button" className={BUTTON} onClick={() => onAnswer(undefined)}>
              {T.common.cancel}
            </button>
            <button
              type="button"
              disabled={touched && problem !== undefined}
              className={`${BUTTON} border-ui-accent`}
              onClick={submit}
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
  | { kind: "confirm"; question: GitConfirm; answer(ok: boolean): void }
  | {
      kind: "branch";
      question: BranchNameQuestion;
      answer(value: BranchNameAnswer | undefined): void;
    };

export interface GitDialogs {
  confirm(question: GitConfirm): Promise<boolean>;
  askBranch(question: BranchNameQuestion): Promise<BranchNameAnswer | undefined>;
}

/** 확인 · 브랜치 이름을 묻는 함수와 그 다이얼로그. `element` 를 그린다. 한 번에 하나만 묻는다 — 떠 있는 동안의 질문은 곧바로 취소다. */
export function useGitDialogs(): GitDialogs & { element: ReactNode } {
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
  const confirm = useCallback(
    (question: GitConfirm) =>
      ask<boolean>((answer) => ({ kind: "confirm", question, answer }), false),
    [ask],
  );
  const askBranch = useCallback(
    (question: BranchNameQuestion) =>
      ask<BranchNameAnswer | undefined>(
        (answer) => ({ kind: "branch", question, answer }),
        undefined,
      ),
    [ask],
  );
  const element =
    asked?.kind === "confirm" ? (
      <ConfirmDialog question={asked.question} onAnswer={asked.answer} />
    ) : asked?.kind === "branch" ? (
      <BranchNameDialog question={asked.question} onAnswer={asked.answer} />
    ) : undefined;
  return { confirm, askBranch, element };
}
