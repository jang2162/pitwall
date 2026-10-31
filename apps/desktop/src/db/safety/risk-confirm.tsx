// 위험 쿼리 확인 다이얼로그(14-m3-plan.md M3-19 ②, 결정 M3-D20): 등급 · 문장 목록(문장마다 등급과 까닭) · 연결 이름을 보이고
// 사람의 확인을 `DbConfirmed { grade, statementsHash, name? }` 로 돌려준다. 부르는 쪽(콘솔 M3-15 · 그리드 편집 M3-17 · Redis 콘솔
// M3-18)은 그것을 실행 RPC 의 `confirmed` 에 그대로 싣는다.
//
// - 바탕은 M2-20 의 위험 작업 확인(git/dialogs/git-dialogs.tsx 의 `ConfirmDialog`)과 같다: 위쪽 시트, 바깥 · Esc 는 취소, 떠 있는
//   동안 창의 키맵은 쉰다, **처음 포커스는 [취소]**(Enter 한 번에 실행되지 않게 — 등급과 상관없이), 한글 조합 중의 Enter 는 조합을
//   끝낼 뿐이다.
// - 파괴 등급 · 프로덕션 연결(`nameRequired`)은 연결 이름을 그대로 쳐야 [실행] 이 켜진다. **강제는 Host 가 한다**(M3-05 관문이
//   이름 · 해시 · 등급을 다시 본다) — 이 다이얼로그는 그 요청을 보여 주고 친 이름을 실어 보낼 뿐이다.
// - `unparsed` 문장은 「분석하지 못한 문장」 으로 따로 표시한다(무엇을 하는지 모르는 문장 — fail-closed, 결정 M3-D5).
// - 등급을 TS 에서 매기지 않는다(결정 M3-D5 — 분류기는 Host 하나). `confirmationFor` 는 Host 가 이미 매긴 분류(`db.sql.classify`)에
//   정책(결정 M3-D20)을 얹어 실행 전에 미리 물을지만 정한다 — 그래도 Host 가 `3001` 을 주면 그것을 다시 묻는다.

import * as Dialog from "@radix-ui/react-dialog";
import type {
  DbConfirmed,
  DbConfirmRequired,
  DbSqlClassification,
  DbStatementClass,
  RiskGrade,
} from "@pitwall/rpc-client";
import { TriangleAlert } from "lucide-react";
import { type ReactNode, useCallback, useRef, useState } from "react";

import { BUTTON, SHEET } from "../../git/dialogs/git-dialogs";
import { useKeymapPopup } from "../../keymap/keymap-provider";
import { isComposingKey } from "../../ui/ime";
import { ConnectionMark, type DbConnectionMarkInput } from "./connection-mark";
import { type DbSafetyStrings, useDbSafetyStrings } from "./strings";

/** 위험도 순서(crates/pw-db-sql 의 `RiskGrade` 의 `Ord`). */
export const RISK_GRADES: readonly RiskGrade[] = [
  "read",
  "write",
  "unboundedWrite",
  "ddl",
  "destructive",
];

export const gradeRank = (grade: RiskGrade): number => RISK_GRADES.indexOf(grade);

/** 목록에 보이는 문장 수. 넘는 것은 「그리고 n개 더」(등급 · 해시는 모든 문장의 것이다). */
export const STATEMENTS_SHOWN = 50;

/** 문장 원문을 보일 때의 글자 상한. */
export const STATEMENT_TEXT_SHOWN = 400;

/** 확인하는 연결. */
export interface RiskConfirmConnection extends DbConnectionMarkInput {
  name: string;
}

export interface RiskConfirmQuestion {
  /** `3001` 의 `data`(또는 `confirmationFor` 가 만든 것). */
  request: DbConfirmRequired;
  connection: RiskConfirmConnection;
  /** 실행할 SQL 전체 — 문장 범위(`range`, UTF-16)로 원문을 잘라 보인다. 없으면 원문 없이 등급 · 까닭만. */
  sql?: string;
}

/**
 * 연결 정책(결정 M3-D20, crates/pw-db/src/gate.rs `judge` 와 같은 규칙)으로 본 분류의 처분: 확인 없이 돈다(`undefined`), 확인을
 * 받는다(`DbConfirmRequired` — `rejected` 는 `null`), read-only 연결이라 거절된다(`"refused"`). 판정의 정본은 Host 다 — 이것은
 * 실행 전에 미리 묻기 위한 것이고, Host 가 `3001` 을 주면 그 `data` 로 다시 묻는다.
 */
export function confirmationFor(
  classification: DbSqlClassification,
  connection: { production?: boolean | null; readOnly?: boolean | null },
): DbConfirmRequired | "refused" | undefined {
  const { grade, effects } = classification;
  if (connection.readOnly)
    return classification.statements.some((s) => s.grade !== "read") ? "refused" : undefined;
  const production = connection.production === true;
  const needs =
    gradeRank(grade) >= gradeRank("unboundedWrite") ||
    (production && gradeRank(grade) >= gradeRank("write")) ||
    effects.length > 0;
  if (!needs) return undefined;
  return {
    grade,
    statements: classification.statements,
    statementsHash: classification.statementsHash,
    effects,
    nameRequired: production && grade === "destructive",
    rejected: null,
  };
}

function rejectedText(rejected: string, T: DbSafetyStrings["confirm"]["rejected"]): string {
  switch (rejected) {
    case "hashMismatch":
    case "gradeTooLow":
    case "nameMismatch":
      return T[rejected];
    default:
      return T.other;
  }
}

function statementText(sql: string | undefined, statement: DbStatementClass): string | undefined {
  if (sql === undefined) return undefined;
  const text = sql.slice(statement.range.from, statement.range.to);
  return text.length > STATEMENT_TEXT_SHOWN ? `${text.slice(0, STATEMENT_TEXT_SHOWN)}…` : text;
}

function StatementList({
  request,
  sql,
  T,
}: {
  request: DbConfirmRequired;
  sql: string | undefined;
  T: DbSafetyStrings;
}) {
  const shown = request.statements.slice(0, STATEMENTS_SHOWN);
  const hidden = request.statements.length - shown.length;
  return (
    <div className="flex min-h-0 flex-col gap-1">
      <p>{T.confirm.statements(request.statements.length)}</p>
      <ol
        data-testid="db-confirm-statements"
        className="flex min-h-0 flex-col gap-2 overflow-auto rounded-sm border border-ui-border px-2 py-1"
      >
        {shown.map((statement) => {
          const unparsed = statement.reasons.includes("unparsed");
          const text = statementText(sql, statement);
          return (
            <li
              key={statement.index}
              data-testid="db-confirm-statement"
              data-grade={statement.grade}
              data-unparsed={unparsed ? "true" : undefined}
              className="flex flex-col gap-0.5"
            >
              <span className="flex flex-wrap items-center gap-2 text-[12px]">
                <span className="text-ui-muted-foreground">
                  {T.confirm.statement(statement.index + 1)}
                </span>
                <span data-testid="db-confirm-statement-grade" className="font-semibold">
                  {T.grade[statement.grade]}
                </span>
                {unparsed && (
                  <span data-testid="db-confirm-unparsed" className="font-semibold text-ui-accent">
                    {T.confirm.unparsed}
                  </span>
                )}
              </span>
              {text !== undefined && (
                <pre className="max-h-24 overflow-auto font-mono text-[12px] whitespace-pre-wrap select-text">
                  {text}
                </pre>
              )}
              <ul className="text-[12px] text-ui-muted-foreground">
                {statement.reasons.map((reason) => (
                  <li key={reason} data-reason={reason}>
                    {T.reason[reason]}
                  </li>
                ))}
              </ul>
            </li>
          );
        })}
        {hidden > 0 && <li className="text-ui-muted-foreground">{T.confirm.more(hidden)}</li>}
      </ol>
    </div>
  );
}

/** 다이얼로그 하나. `onAnswer(undefined)` 는 취소다. */
export function RiskConfirmDialog({
  question,
  onAnswer,
}: {
  question: RiskConfirmQuestion;
  onAnswer(confirmed: DbConfirmed | undefined): void;
}) {
  const T = useDbSafetyStrings();
  useKeymapPopup(true);
  const { request, connection, sql } = question;
  const [typed, setTyped] = useState("");
  const nameOk = !request.nameRequired || typed === connection.name;
  const run = () => {
    if (!nameOk) return;
    onAnswer({
      grade: request.grade,
      statementsHash: request.statementsHash,
      name: request.nameRequired ? typed : null,
    });
  };
  const risky = gradeRank(request.grade) >= gradeRank("unboundedWrite");
  return (
    <Dialog.Root open onOpenChange={(open) => !open && onAnswer(undefined)}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content
          data-testid="db-risk-confirm"
          data-grade={request.grade}
          className={SHEET}
          onOpenAutoFocus={(e) => {
            // 처음 포커스는 [취소] — Radix 가 첫 포커스 가능한 것(이름 칸)을 고르지 않게 직접 둔다.
            e.preventDefault();
            (e.currentTarget as HTMLElement | null)
              ?.querySelector<HTMLButtonElement>("[data-action=cancel]")
              ?.focus();
          }}
        >
          <Dialog.Title className="flex items-center gap-2 text-sm font-semibold">
            {risky && <TriangleAlert size={16} aria-hidden className="flex-none text-ui-accent" />}
            {T.confirm.title[request.grade]}
          </Dialog.Title>
          <Dialog.Description asChild>
            <div className="flex flex-col gap-1">
              <span className="flex items-center gap-2">
                <span>{T.confirm.grade(T.grade[request.grade])}</span>
                <span aria-hidden className="text-ui-muted-foreground">
                  ·
                </span>
                <ConnectionMark
                  name={connection.name}
                  production={connection.production}
                  readOnly={connection.readOnly}
                  color={connection.color}
                />
              </span>
              {connection.production && (
                <span data-testid="db-confirm-production" className="text-db-production">
                  {T.confirm.production(connection.name)}
                </span>
              )}
            </div>
          </Dialog.Description>
          {request.rejected !== null && (
            <p role="alert" data-testid="db-confirm-rejected" className="text-ui-accent">
              {rejectedText(request.rejected, T.confirm.rejected)}
            </p>
          )}
          {request.effects.map((effect) => (
            <p
              key={effect}
              role="alert"
              data-testid="db-confirm-effect"
              data-effect={effect}
              className="text-ui-accent"
            >
              {T.effect[effect]}
            </p>
          ))}
          <StatementList request={request} sql={sql} T={T} />
          {request.nameRequired && (
            <label className="flex flex-col gap-1">
              <span>{T.confirm.namePrompt(connection.name)}</span>
              <input
                data-testid="db-confirm-name"
                aria-label={T.confirm.name}
                aria-invalid={typed !== "" && !nameOk}
                value={typed}
                spellCheck={false}
                autoComplete="off"
                onChange={(e) => setTyped(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key !== "Enter" || isComposingKey(e.nativeEvent)) return;
                  e.preventDefault();
                  run();
                }}
                className="rounded-sm border border-ui-border bg-transparent px-2 py-1 font-mono outline-none select-text focus:border-ui-accent"
              />
              {typed !== "" && !nameOk && (
                <span className="text-[12px] text-ui-accent">{T.confirm.nameMismatch}</span>
              )}
            </label>
          )}
          <div className="flex justify-end gap-2">
            <button
              type="button"
              data-action="cancel"
              className={BUTTON}
              onClick={() => onAnswer(undefined)}
            >
              {T.confirm.cancel}
            </button>
            <button
              type="button"
              data-action="run"
              disabled={!nameOk}
              className={`${BUTTON} border-ui-accent ${risky ? "text-ui-accent" : ""}`}
              onClick={run}
            >
              {T.confirm.run}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/**
 * 위험 쿼리 확인을 묻는 함수와 그 다이얼로그. `element` 를 그린다. 한 번에 하나만 묻는다 — 떠 있는 동안의 질문은 곧바로 취소
 * (`undefined`)다(M2-20 의 `useGitDialogs` 와 같다).
 */
export function useRiskConfirm(): {
  confirm(question: RiskConfirmQuestion): Promise<DbConfirmed | undefined>;
  element: ReactNode;
} {
  const [asked, setAsked] = useState<{
    question: RiskConfirmQuestion;
    answer(value: DbConfirmed | undefined): void;
  }>();
  const open = useRef(false);
  const confirm = useCallback(
    (question: RiskConfirmQuestion) =>
      new Promise<DbConfirmed | undefined>((resolve) => {
        if (open.current) return resolve(undefined);
        open.current = true;
        setAsked({
          question,
          answer: (value) => {
            open.current = false;
            setAsked(undefined);
            resolve(value);
          },
        });
      }),
    [],
  );
  const element = asked ? (
    <RiskConfirmDialog question={asked.question} onAnswer={asked.answer} />
  ) : undefined;
  return { confirm, element };
}
