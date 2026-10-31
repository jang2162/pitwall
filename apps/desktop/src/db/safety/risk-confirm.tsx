// 위험 쿼리 확인 다이얼로그(14-m3-plan.md M3-19 ②, 결정 M3-D20): 등급 · 문장 목록(문장마다 등급과 까닭) · 연결 이름을 보이고
// 사람의 확인을 `DbConfirmed { grade, statementsHash, name? }` 로 돌려준다. 부르는 쪽(콘솔 M3-15 · 그리드 편집 M3-17 · Redis 콘솔
// M3-18)은 그것을 실행 RPC 의 `confirmed` 에 그대로 싣는다.
//
// - 바탕은 M2-20 의 위험 작업 확인(git/dialogs/git-dialogs.tsx 의 `ConfirmDialog`)과 같다: 위쪽 시트, 바깥 · Esc 는 취소, 떠 있는
//   동안 창의 키맵은 쉰다, **처음 포커스는 [취소]**(Enter 한 번에 실행되지 않게 — 등급과 상관없이), 한글 조합 중의 Enter 는 조합을
//   끝낼 뿐이고 조합 중의 Esc 는 조합만 취소한다(친 이름을 잃지 않게).
// - 제목은 실제 등급을 말한다. 파괴 등급이라도 그것이 문장을 읽지 못해서(`unparsed` · `tooLarge` · `tooDeep`)라면 「되돌릴 수
//   없다」 고 단정하지 않고 「분석하지 못해 파괴로 다룬다」 고 말한다.
// - 파괴 등급 · 프로덕션 연결(`nameRequired`)은 연결 이름을 그대로 쳐야 [실행] 이 켜진다. **강제는 Host 가 한다**(M3-05 관문이
//   이름 · 해시 · 등급을 다시 본다) — 이 다이얼로그는 그 요청을 보여 주고 친 이름을 실어 보낼 뿐이다.
// - `unparsed` 문장은 「분석하지 못한 문장」 으로 따로 표시한다(무엇을 하는지 모르는 문장 — fail-closed, 결정 M3-D5).
// - 등급을 TS 에서 매기지 않는다(결정 M3-D5 — 분류기는 Host 하나). `confirmationFor` 는 Host 가 이미 매긴 분류(`db.sql.classify`)에
//   정책(결정 M3-D20)을 얹어 실행 전에 미리 물을지만 정한다 — 그래도 Host 가 `3001` 을 주면 그것을 다시 묻는다. read-only 연결은
//   미리 판정하지 않는다: 분류에는 Host 가 함께 보는 것(세션 설정 · 트랜잭션 제어 문장)이 없어 화면이 Host 보다 느슨하거나 엄격해질
//   수 있다 — 그대로 보내 Host 가 주는 거절(`readOnlyConnection` 과 거절한 문장, messages.tsx 의 `DbRefusalMessage`)을 보인다.
// - Redis 명령(M3-18 · M3-22 — `db.kv.*` 의 `3001` 은 `data` 가 `DbKvConfirmRequired`)은 같은 다이얼로그에 `KvConfirmQuestion`
//   으로 묻는다: 문장 목록 대신 서버에 갈 명령 한 줄(argv)을 보이고 까닭 · effects 는 없다. 인자는 글이거나 `{ hex }`(UTF-8 이 아닌
//   바이트)라 둘을 갈라 보인다 — 글 `"0xff"` 와 바이트 `0xff` 가 같아 보이지 않게. 처음 포커스 · 이름 치기 · 돌려주는 값은 SQL 과
//   같다.

import * as Dialog from "@radix-ui/react-dialog";
import type {
  DbConfirmed,
  DbConfirmRequired,
  DbKvConfirmRequired,
  DbSqlClassification,
  DbStatementClass,
  RiskGrade,
  KvKey,
  RiskReason,
} from "@pitwall/rpc-client";
import { TriangleAlert } from "lucide-react";
import { type ReactNode, useCallback, useEffect, useRef, useState } from "react";

import { BUTTON, SHEET } from "../../git/dialogs/git-dialogs";
import { useKeymapPopup } from "../../keymap/keymap-provider";
import { isComposingKey, keepOpenWhileComposing } from "../../ui/ime";
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

/** Redis 명령에서 보이는 인자 수(이름 포함). 넘는 것은 「그리고 인자 n개 더」(해시는 모든 인자의 것이다). */
export const KV_ARGS_SHOWN = 50;

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

/** Redis 명령의 확인(`db.kv.*` 의 `3001`). */
export interface KvConfirmQuestion {
  /** `3001` 의 `data`. */
  kv: DbKvConfirmRequired;
  connection: RiskConfirmConnection;
  /** 다이얼로그 위에 한 줄로 덧붙일 안내(대상이 있는 이름 바꾸기의 「대상 키를 덮어쓴다」). */
  note?: string;
}

/** 다이얼로그가 받는 질문 — SQL 문장들 또는 Redis 명령 하나. */
export type ConfirmQuestion = RiskConfirmQuestion | KvConfirmQuestion;

function isKvQuestion(question: ConfirmQuestion): question is KvConfirmQuestion {
  return "kv" in question;
}

/** 두 꼴이 함께 가진 칸(등급 · 해시 · 이름 요구 · 앞 확인을 받지 않은 까닭). */
function headOf(question: ConfirmQuestion): DbKvConfirmRequired | DbConfirmRequired {
  return isKvQuestion(question) ? question.kv : question.request;
}

/**
 * 연결 정책(결정 M3-D20, crates/pw-db/src/gate.rs `judge` 와 같은 규칙)으로 본 분류의 처분: 확인 없이 보낸다(`undefined`), 확인을
 * 받는다(`DbConfirmRequired` — `rejected` 는 `null`). 판정의 정본은 Host 다 — 이것은 실행 전에 미리 묻기 위한 것이고, Host 가
 * `3001` 을 주면 그 `data` 로 다시 묻는다. read-only 연결은 늘 `undefined` 다 — Host 는 거기서 확인을 묻지 않고 돌리거나 거절하며,
 * 거절은 Host 가 준 까닭으로 보인다(머리 주석).
 */
export function confirmationFor(
  classification: DbSqlClassification,
  connection: { production?: boolean | null; readOnly?: boolean | null },
): DbConfirmRequired | undefined {
  const { grade, effects } = classification;
  if (connection.readOnly) return undefined;
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

/** 문장을 읽지 못해 파괴로 다룬 까닭 — 그 문장이 무엇을 하는지 모른다(crates/pw-db-sql 의 `RiskReason`). */
const UNREAD_REASONS: readonly RiskReason[] = ["unparsed", "tooLarge", "tooDeep"];

/**
 * 다이얼로그 제목의 종류: 가장 높은 등급, 단 파괴 등급의 문장이 모두 읽지 못한 문장(까닭이 `UNREAD_REASONS` 뿐)이면 `"unread"`.
 * 읽고 파괴로 판정한 문장(`drop` 등)이 하나라도 있으면 `"destructive"` 다.
 */
export function confirmTitleKind(request: DbConfirmRequired): RiskGrade | "unread" {
  if (request.grade !== "destructive") return request.grade;
  const destructive = request.statements.filter((s) => s.grade === "destructive");
  const unread =
    destructive.length > 0 &&
    destructive.every(
      (s) => s.reasons.length > 0 && s.reasons.every((r) => UNREAD_REASONS.includes(r)),
    );
  return unread ? "unread" : "destructive";
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

/** 인자 하나(보이기용). 글은 따옴표 안에, 바이트는 hex 와 「바이트」 표지로. */
function KvArg({ arg, T }: { arg: KvKey; T: DbSafetyStrings }) {
  if (typeof arg === "string") {
    const text = arg.length > STATEMENT_TEXT_SHOWN ? `${arg.slice(0, STATEMENT_TEXT_SHOWN)}…` : arg;
    return (
      <span data-testid="db-confirm-kv-arg" data-arg="text" className="break-all">
        {JSON.stringify(text)}
      </span>
    );
  }
  const hex =
    arg.hex.length > STATEMENT_TEXT_SHOWN ? `${arg.hex.slice(0, STATEMENT_TEXT_SHOWN)}…` : arg.hex;
  return (
    <span
      data-testid="db-confirm-kv-arg"
      data-arg="bytes"
      className="rounded-sm border border-ui-border px-1 break-all"
    >
      {hex}
      <span className="ml-1 text-[11px] text-ui-muted-foreground">{T.confirm.kv.bytes}</span>
    </span>
  );
}

/** 서버에 갈 명령 한 줄. */
function KvCommandLine({ argv, T }: { argv: readonly KvKey[]; T: DbSafetyStrings }) {
  const shown = argv.slice(0, KV_ARGS_SHOWN);
  const hidden = argv.length - shown.length;
  return (
    <div className="flex min-h-0 flex-col gap-1">
      <p>{T.confirm.kv.command}</p>
      <p
        data-testid="db-confirm-kv-command"
        className="flex max-h-48 flex-wrap gap-x-2 gap-y-1 overflow-auto rounded-sm border border-ui-border px-2 py-1 font-mono text-[12px] select-text"
      >
        {shown.map((arg, i) => (
          // 인자는 자리가 곧 뜻이다(같은 글이 두 번 올 수 있다).
          <KvArg key={i} arg={arg} T={T} />
        ))}
        {hidden > 0 && (
          <span className="text-ui-muted-foreground">{T.confirm.kv.more(hidden)}</span>
        )}
      </p>
    </div>
  );
}

/** 다이얼로그 하나. `onAnswer(undefined)` 는 취소다. */
export function RiskConfirmDialog({
  question,
  onAnswer,
}: {
  question: ConfirmQuestion;
  onAnswer(confirmed: DbConfirmed | undefined): void;
}) {
  const T = useDbSafetyStrings();
  useKeymapPopup(true);
  const { connection } = question;
  const request = headOf(question);
  const kv = isKvQuestion(question) ? question : undefined;
  const sql = kv ? undefined : (question as RiskConfirmQuestion);
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
  // Redis 명령에는 「읽지 못한 문장」 이 없다 — 등급 그대로.
  const titleKind = sql ? confirmTitleKind(sql.request) : request.grade;
  return (
    <Dialog.Root open onOpenChange={(open) => !open && onAnswer(undefined)}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content
          data-testid="db-risk-confirm"
          data-grade={request.grade}
          className={SHEET}
          onEscapeKeyDown={keepOpenWhileComposing}
          onOpenAutoFocus={(e) => {
            // 처음 포커스는 [취소] — Radix 가 첫 포커스 가능한 것(이름 칸)을 고르지 않게 직접 둔다.
            e.preventDefault();
            (e.currentTarget as HTMLElement | null)
              ?.querySelector<HTMLButtonElement>("[data-action=cancel]")
              ?.focus();
          }}
        >
          <Dialog.Title
            data-title={titleKind}
            className="flex items-center gap-2 text-sm font-semibold"
          >
            {risky && <TriangleAlert size={16} aria-hidden className="flex-none text-ui-accent" />}
            {titleKind === "unread" ? T.confirm.titleUnread : T.confirm.title[titleKind]}
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
          {kv?.note !== undefined && (
            <p role="alert" data-testid="db-confirm-note" className="text-ui-accent">
              {kv.note}
            </p>
          )}
          {(sql?.request.effects ?? []).map((effect) => (
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
          {sql ? (
            <StatementList request={sql.request} sql={sql.sql} T={T} />
          ) : (
            <KvCommandLine argv={kv!.kv.argv} T={T} />
          )}
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
 * `confirm` 의 답: 사람이 [실행] 한 확인(`DbConfirmed`), 취소(`undefined` — [취소] · Esc · 바깥, 또는 묻는 동안 이 훅의 주인이
 * 사라졌다), 다른 질문이 떠 있어 묻지 않았다(`"busy"`).
 */
export type RiskConfirmAnswer = DbConfirmed | undefined | "busy";

/** 떠 있는 질문과 같은 요청인가 — 같은 연결의 같은 문장 · 명령(해시) · 등급 · 이름 요구. */
function sameRequest(a: ConfirmQuestion, b: ConfirmQuestion): boolean {
  const x = headOf(a);
  const y = headOf(b);
  return (
    a.connection.name === b.connection.name &&
    x.statementsHash === y.statementsHash &&
    x.grade === y.grade &&
    x.nameRequired === y.nameRequired
  );
}

interface Asked {
  question: ConfirmQuestion;
  promise: Promise<DbConfirmed | undefined>;
  answer(value: DbConfirmed | undefined): void;
}

/**
 * 위험 쿼리 확인을 묻는 함수와 그 다이얼로그. `element` 를 그린다. 한 번에 하나만 묻는다 — 떠 있는 동안 같은 요청을 다시 물으면
 * (실행 키를 두 번 누름) 떠 있는 질문의 답을 함께 받고, 다른 요청은 묻지 않고 `"busy"` 를 받는다. 훅의 주인이 언마운트되면 떠 있던
 * 질문은 취소(`undefined`)로 끝난다 — 기다리는 쪽이 영영 멈추지 않게.
 */
export function useRiskConfirm(): {
  confirm(question: ConfirmQuestion): Promise<RiskConfirmAnswer>;
  element: ReactNode;
} {
  const [asked, setAsked] = useState<Asked>();
  const current = useRef<Asked>(undefined);
  useEffect(() => () => current.current?.answer(undefined), []);
  const confirm = useCallback((question: ConfirmQuestion): Promise<RiskConfirmAnswer> => {
    const open = current.current;
    if (open) return sameRequest(open.question, question) ? open.promise : Promise.resolve("busy");
    const entry = { question } as Asked;
    entry.promise = new Promise<DbConfirmed | undefined>((resolve) => {
      entry.answer = (value) => {
        if (current.current !== entry) return;
        current.current = undefined;
        setAsked(undefined);
        resolve(value);
      };
    });
    current.current = entry;
    setAsked(entry);
    return entry.promise;
  }, []);
  const element = asked ? (
    <RiskConfirmDialog question={asked.question} onAnswer={asked.answer} />
  ) : undefined;
  return { confirm, element };
}
