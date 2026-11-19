// 첫 실행 확인 대화상자(17-m6-plan M6-07 ⑥ — 규칙은 run-confirm.ts · crates/pw-run/src/runs/confirm.rs).
//
// - 실행될 내용을 **가리지 않고** 보인다(M6-13 리뷰 B1 판정): 명령(argv)과 정의 본문(npm script)은 Host 가 준 그대로다. 가린 것은
//   env 의 값뿐이고 Host 가 가려서 준다(`pw_run::config::shown` — 승인 화면과 같은 부품). 명령에 비밀처럼 보이는 것이 있으면 경고만
//   단다.
// - 사슬(`before` · 복합 구성 — 17-m6-plan M6-21)이면 띄울 단계를 모두 차례대로 보인다(`steps` · `order`). `sql` 단계는 연결과
//   파일만 — 그 문장은 이 확인 뒤의 위험 확인이 보인다.
// - 처음 포커스는 [취소]다. Esc · 바깥 누르기도 취소다. 떠 있는 동안 창의 키맵은 쉰다(창 팝업 규칙).

import type { RunConfirmRequired, RunShown } from "@pitwall/rpc-client";
import * as Dialog from "@radix-ui/react-dialog";
import { TriangleAlert } from "lucide-react";
import { type ReactNode, useCallback, useEffect, useRef, useState } from "react";

import { useKeymapPopup } from "../keymap/keymap-provider";
import { setRunConfirmAsker } from "./run-confirm";
import { useRunStrings } from "./strings";

const BUTTON = "rounded-sm border border-ui-border px-3 py-1 hover:bg-ui-hover";
const CODE =
  "max-h-40 overflow-auto rounded-sm border border-ui-border bg-ui-background px-2 py-1 font-mono text-[12px] break-all whitespace-pre-wrap select-text";

/** argv 한 칸을 사람이 읽을 꼴로(공백 · 따옴표가 있으면 작은따옴표로). 보이기만 한다 — 띄우는 것은 Host 의 argv 그대로다. */
export function shownArg(arg: string): string {
  if (arg !== "" && !/[\s'"\\$`]/.test(arg)) return arg;
  return `'${arg.replace(/'/g, `'\\''`)}'`;
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="grid grid-cols-[96px_1fr] items-start gap-2">
      <span className="text-ui-muted-foreground">{label}</span>
      <div className="min-w-0">{children}</div>
    </div>
  );
}

/** 띄울 구성 하나의 내용. */
function StepRows({ shown }: { shown: RunShown }) {
  const T = useRunStrings().confirm;
  return (
    <div className="flex flex-col gap-2 text-[12px]">
      <Row label={T.kind}>
        <span className="font-mono">{shown.kind}</span>
      </Row>
      <Row label={T.command}>
        <pre data-testid="run-confirm-command" className={CODE}>
          {shown.argv.map(shownArg).join(" ")}
        </pre>
      </Row>
      {shown.definition !== null && (
        <Row label={T.definition}>
          {shown.definitionSource !== null && (
            <p
              data-testid="run-confirm-definition-source"
              className="mb-0.5 font-mono text-[11px] break-all text-ui-muted-foreground"
            >
              {shown.definitionSource}
            </p>
          )}
          <pre data-testid="run-confirm-definition" className={CODE}>
            {shown.definition}
          </pre>
        </Row>
      )}
      <Row label={T.cwd}>
        <span data-testid="run-confirm-cwd" className="font-mono">
          {shown.cwd}
        </span>
      </Row>
      {shown.profile !== null && (
        <Row label={T.profile}>
          <span className="font-mono">{shown.profile}</span>
        </Row>
      )}
      <Row label={T.env}>
        {shown.env.length === 0 ? (
          <span className="text-ui-muted-foreground">{T.envNone}</span>
        ) : (
          <ul data-testid="run-confirm-env" className="flex flex-col font-mono">
            {shown.env.map((v) => (
              <li key={v.name} data-masked={v.masked} className="break-all">
                {v.name}={v.value}
                {v.masked && <span className="sr-only"> ({T.masked})</span>}
              </li>
            ))}
          </ul>
        )}
      </Row>
      {shown.envFiles.length > 0 && (
        <Row label={T.envFiles}>
          <ul className="flex flex-col font-mono">
            {shown.envFiles.map((f) => (
              <li key={f.path}>
                {f.path} <span className="text-ui-muted-foreground">sha256 {f.sha256}</span>
              </li>
            ))}
          </ul>
        </Row>
      )}
    </div>
  );
}

export function RunConfirmDialog({
  shown,
  steps = [],
  order = null,
  onAnswer,
}: {
  shown: RunShown;
  /** 사슬(`before` · 복합 구성)이면 띄울 단계 모두(띄우는 차례). */
  steps?: RunShown[];
  order?: string | null;
  onAnswer(run: boolean): void;
}) {
  const T = useRunStrings().confirm;
  useKeymapPopup(true);
  return (
    <Dialog.Root open onOpenChange={(open) => !open && onAnswer(false)}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content
          data-testid="run-confirm-dialog"
          className="sheet-top fixed left-1/2 flex max-h-[80vh] w-[560px] max-w-[94vw] -translate-x-1/2 flex-col gap-3 overflow-y-auto rounded-b-md border border-t-0 border-ui-border bg-ui-panel px-4 py-3 text-ui-foreground shadow-lg"
        >
          <Dialog.Title className="text-sm font-semibold">{T.title(shown.name)}</Dialog.Title>
          <Dialog.Description className="text-ui-muted-foreground">{T.body}</Dialog.Description>
          {shown.secretLooking && (
            <p
              role="alert"
              data-testid="run-confirm-secret"
              className="flex items-start gap-2 text-[12px] text-ui-accent"
            >
              <TriangleAlert size={14} aria-hidden className="mt-0.5 flex-none" />
              {T.secretWarning}
            </p>
          )}
          {steps.length > 0 ? (
            <>
              {order !== null && (
                <p data-testid="run-confirm-order" className="text-[12px]">
                  <span className="text-ui-muted-foreground">{T.order} </span>
                  <span className="font-mono">{order}</span>
                </p>
              )}
              <ol data-testid="run-confirm-steps" className="flex flex-col gap-3">
                {steps.map((step, i) => (
                  <li key={`${i}-${step.key}`} data-step={step.key} className="flex flex-col gap-1">
                    <p className="text-[12px] font-semibold">
                      {i + 1}. {step.name}{" "}
                      <span className="font-mono text-ui-muted-foreground">({step.key})</span>
                    </p>
                    <StepRows shown={step} />
                  </li>
                ))}
              </ol>
            </>
          ) : (
            <StepRows shown={shown} />
          )}
          {!shown.recordable && steps.length === 0 && (
            <p className="text-[12px] text-ui-muted-foreground">{T.notRecorded}</p>
          )}
          <div className="flex justify-end gap-2">
            <button type="button" autoFocus className={BUTTON} onClick={() => onAnswer(false)}>
              {T.cancel}
            </button>
            <button
              type="button"
              className={`${BUTTON} border-ui-accent`}
              onClick={() => onAnswer(true)}
            >
              {T.run}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/**
 * 이 창의 첫 실행 확인을 묻는 쪽을 두고 그 대화상자를 그린다. 한 번에 하나만 묻는다 — 떠 있는 동안 온 다른 질문은 취소로 답한다
 * (같은 클릭이 두 번 띄우지 않게).
 */
export function RunConfirmHost() {
  const [question, setQuestion] = useState<{
    data: RunConfirmRequired;
    answer(run: boolean): void;
  }>();
  // 떠 있는 질문의 답. 이 컴포넌트가 사라지면 취소로 끝낸다 — 기다리는 쪽이 멈추지 않게.
  const open = useRef<(run: boolean) => void>(undefined);
  const ask = useCallback(
    (data: RunConfirmRequired) =>
      new Promise<boolean>((resolve) => {
        if (open.current) {
          resolve(false);
          return;
        }
        const answer = (run: boolean) => {
          if (open.current !== answer) return;
          open.current = undefined;
          setQuestion(undefined);
          resolve(run);
        };
        open.current = answer;
        setQuestion({ data, answer });
      }),
    [],
  );
  useEffect(() => {
    const unset = setRunConfirmAsker(ask);
    return () => {
      unset();
      open.current?.(false);
    };
  }, [ask]);
  return question ? (
    <RunConfirmDialog
      shown={question.data.shown}
      steps={question.data.steps}
      order={question.data.order}
      onAnswer={question.answer}
    />
  ) : null;
}
