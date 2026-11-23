// MCP 승인 다이얼로그(16-m5-plan M5-14, 10-ui.md 「다이얼로그 · 팝업 목록」 의 MCP 행). 요청 에이전트 · 툴 · 등급 · 바뀔 것을
// 보이고 [거절] [이번만 허용] [이 세션 동안 허용] 으로 답한다. 무엇을 띄울지 · 답을 보내는 것은 approvals-host.tsx 다.
//
// - 다른 머신을 겨눈 요청(`machine` — 18-m7-plan M7-18)은 「머신」 줄에 그 머신의 이름을 보인다. 이름은 Desktop 의 별명이다
//   (`useMachineName` — Relay 가 댄 이름이 아니다, M7-P19). 그 머신의 `write` · `exec` 에는 [이 세션 동안 <머신> 허용](`machine`,
//   CLI 면 창이 열려 있는 동안 CLI 에) 단추가 붙는다 — Host 가 `answers` 에 실을 때만.
// - 셋째 단추: CLI 요청이면 「창이 열려 있는 동안 CLI 허용」(`window`, 결정 M5-D5), 아니면 「이 세션 동안 허용」(`session`).
//   `dangerous` 와 기억하지 않는 계획(`open_with_upsert` — `rules.remember` 가 `never`, 결정 M5-D31)에는 없다. Host 가 요청에
//   실은 `answers` 에 없는 답도 그리지 않는다 — 둘 다 맞아야 보인다.
// - **처음 포커스는 [거절]**(신뢰 확인 · 위험 작업 확인과 같은 규칙) — 뜬 순간 친 Enter · Space 가 허용이 되지 않게. `Esc` 는
//   다이얼로그만 닫는다(요청은 「에이전트 활동」 의 「승인 대기」 에 남는다). 바깥을 눌러도 닫히지 않는다.
// - **허용 단추의 잠금**(M5-14 리뷰 B1): 요청이 보이기 시작하면(다이얼로그가 뜨거나 다른 요청으로 바뀌거나 목록 · 알림으로 다시
//   열면 — 요청마다 새로 그린다) 허용 계열 단추(이번만 · 세션 · CLI)를 `ARM_DELAY_MS` 동안 잠근다. 잠긴 동안 창에 키 · 포인터가
//   눌리면 다시 센다 — 에디터에서 치던 `Tab` … `Enter`, 더블클릭의 둘째 누름, 앞 요청이 닫히는 순간의 클릭이 새 요청의 허용에
//   닿지 않는다. 포인터로 누른 허용은 잠금이 풀린 뒤에 그 단추에서 내려간 누름이어야 한다(뜨기 전 · 잠긴 동안의 마우스 다운이 풀린
//   뒤의 떼기로 이어지지 않게). [거절] 은 잠그지 않는다. 잠금 시간은 브라우저 권한 창 · Firefox 의 설치 확인
//   (`security.dialog_enable_delay`)과 같은 꼴이다.
// - 에이전트가 보낸 글(클라이언트 이름 · 요약 · 경로 · 바뀔 글 · 문장)은 테두리 친 칸 안에만 그리고 넓은 규칙(agent-text.ts)으로
//   거른다 — 한 줄 자리는 줄바꿈도 표지가 되고, 여러 줄 자리는 높이를 묶어 그 안에서 굴린다. 단추 줄은 굴리는 칸 밖(아래)에
//   늘 있다. 그래서 긴 글 · 줄바꿈 · 단추처럼 꾸민 글이 다이얼로그의 칸이나 단추처럼 보이지 않고 단추를 화면 밖으로 밀지 못한다.
// - SQL(`PlanChange` 의 `sql`): 연결 표지(프로덕션 · 읽기 전용), 파일이면 경로(여럿이면 차례대로 — 실행 구성의 `sql` 종류,
//   17-m6-plan M6-09 ⑥, 문장마다 그 파일), 트랜잭션 방식과 「실패해도 앞 파일은 커밋된다」(`per_file`) · 「실패해도 앞 문장은
//   남는다」(`none`), 「되돌릴 수 없다」, 문장마다 등급. 조회가 아닌
//   문장은 수와 상관없이 모두 보이고 넘는 조회 문장만 접는다(접은 수를 보인다). 파괴 등급 문장이
//   하나라도 있으면 그 연결 이름을 쳐야 허용 단추가 켜진다(위험 쿼리 확인과 같은 부품 — db/safety/name-confirm.tsx).
// - 남은 시한과 「n개 중 k」 를 보인다. 시한이 지나면 approvals.ts 가 요청을 빼 다이얼로그가 내려간다. 시계가 1초마다 다이얼로그를
//   다시 그려도 바뀔 것 목록(`Changes`)은 다시 그리지 않는다.

import * as Dialog from "@radix-ui/react-dialog";
import type {
  McpApprovalAnswer,
  McpApprovalRequest,
  PlanChange,
  SqlStatement,
  ToolPlan,
} from "@pitwall/rpc-client";
import { Lock, TriangleAlert } from "lucide-react";
import { memo, type ReactNode, useEffect, useMemo, useRef, useState } from "react";

import { useMachineName } from "../checkout/machine-mark";
import { ConnectionMark } from "../db/safety/connection-mark";
import { ConnectionNameField } from "../db/safety/name-confirm";
import { useDbSafetyStrings } from "../db/safety/strings";
import { BUTTON } from "../git/dialogs/git-dialogs";
import { DiffView } from "../git/diff/diff-view";
import { useKeymapPopup } from "../keymap/keymap-provider";
import { keepOpenWhileComposing } from "../ui/ime";
import { agentBlock, agentLine, BLOCK_CHARS, hasHidden } from "./agent-text";
import { planDiff } from "./plan-diff";
import { type McpStrings, useMcpStrings } from "./strings";

type Strings = McpStrings["approval"];

/** 허용 단추를 잠그는 시간(밀리초). 잠긴 동안의 키 · 포인터 누름은 이 시간을 처음부터 다시 센다. */
export const ARM_DELAY_MS = 1000;
/** 조회 문장을 보이는 수. 조회가 아닌 문장(쓰기 · 파괴 등)은 수와 상관없이 모두 보이고, 넘는 조회 문장만 접는다. */
export const STATEMENTS_SHOWN = 50;
/** 문장 하나의 원문을 보일 글자 상한. */
export const STATEMENT_CHARS = 2000;
/** 요약 · 경로의 글자 상한. */
const SUMMARY_CHARS = 500;

const AGENT_BOX =
  "rounded-sm border border-ui-border bg-ui-background px-2 py-1 font-mono text-[12px] select-text";

/** 셋째 단추의 답 — 없으면 `undefined`. */
export function thirdAnswer(request: McpApprovalRequest): McpApprovalAnswer | undefined {
  if (request.tier === "dangerous" || request.plan.rules.remember.kind === "never")
    return undefined;
  const answer: McpApprovalAnswer = request.cli ? "window" : "session";
  return request.answers.includes(answer) ? answer : undefined;
}

/**
 * 머신 id 의 Desktop 이름(별명 — `useMachineName`). 다른 머신을 겨눈 요청에서만 그린다 — 이름을 읽는 쿼리가 이 머신 요청에 돌지 않게.
 */
export function MachineName({
  machine,
  children = (name) => name,
}: {
  machine: string;
  children?: (name: string) => ReactNode;
}) {
  return <>{children(useMachineName()(machine))}</>;
}

/** 쳐야 하는 연결 이름: 파괴 등급 문장이 든 SQL 변경의 연결(겹치지 않게, 나온 순서). */
export function namesRequired(plan: ToolPlan): string[] {
  const names: string[] = [];
  for (const change of plan.changes) {
    if (change.kind !== "sql") continue;
    if (!change.statements.some((s) => s.grade === "destructive")) continue;
    if (!names.includes(change.connection.name)) names.push(change.connection.name);
  }
  return names;
}

/**
 * 허용 단추의 잠금. 사람이 그 요청을 볼 수 있게 된 순간부터 `delayMs` 를 센다(M5-14 재리뷰 G1):
 * - 컴포넌트가 요청마다 새로 그려지므로(approvals-host.tsx 의 `key`) 뜰 때 · 다른 요청으로 바뀔 때 잠긴다. `epoch` 가 바뀌면(목록 ·
 *   알림으로 다시 엶 — 이미 떠 있던 요청이어도) 다시 잠근다.
 * - 창이 앞에 없으면(`blur` · 숨김) 잠근 채 세지 않는다 — 뒤에 있는 동안 시간이 흘러 풀리지 않는다. 창이 앞으로 오면(`focus` ·
 *   보임) 처음부터 센다.
 * - 세는 동안 창에 키 · 포인터가 눌리면 처음부터 다시 센다(입력 칸 안의 키는 빼고 — 이름 칸에 치는 글자는 이 다이얼로그에 뜻을 둔
 *   것이다).
 * `delayMs` 가 0 이하면 늘 풀려 있다.
 */
export function useArmed(delayMs: number, epoch = 0): boolean {
  const [armed, setArmed] = useState(delayMs <= 0);
  useEffect(() => {
    if (delayMs <= 0) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let done = false;
    const inFront = () => document.visibilityState !== "hidden" && document.hasFocus();
    const lock = () => {
      done = false;
      clearTimeout(timer);
      timer = undefined;
      setArmed(false);
    };
    const count = () => {
      clearTimeout(timer);
      timer = setTimeout(() => {
        done = true;
        setArmed(true);
      }, delayMs);
    };
    const pressed = (event: Event) => {
      if (done || event.target instanceof HTMLInputElement) return;
      if (timer !== undefined) count();
    };
    // 요소의 blur · focus 는 거품이 없어 창의 (capture 아닌) 듣기까지 오지 않는다 — 그래도 요소에서 온 것은 거른다. 창 자체가
    // 포커스를 잃고 얻을 때만이다.
    const fromElement = (event: Event) => event.target instanceof Element;
    const away = (event: Event) => {
      if (!fromElement(event)) lock();
    };
    const back = (event: Event) => {
      if (fromElement(event)) return;
      lock();
      if (inFront()) count();
    };
    const visibility = () => {
      lock();
      if (inFront()) count();
    };
    lock();
    if (inFront()) count();
    window.addEventListener("keydown", pressed, true);
    window.addEventListener("pointerdown", pressed, true);
    window.addEventListener("mousedown", pressed, true);
    window.addEventListener("blur", away);
    window.addEventListener("focus", back);
    document.addEventListener("visibilitychange", visibility);
    return () => {
      clearTimeout(timer);
      window.removeEventListener("keydown", pressed, true);
      window.removeEventListener("pointerdown", pressed, true);
      window.removeEventListener("mousedown", pressed, true);
      window.removeEventListener("blur", away);
      window.removeEventListener("focus", back);
      document.removeEventListener("visibilitychange", visibility);
    };
  }, [delayMs, epoch]);
  return armed;
}

/**
 * 목록에 보일 문장(원래 순서): 조회가 아닌 문장은 모두, 조회 문장은 앞에서부터 `STATEMENTS_SHOWN` 개까지. 파괴 · 쓰기 문장이 목록
 * 밖에 숨어 「이름을 쳐라」 만 보이는 일이 없게(M5-14 리뷰 F1) — 접히는 것은 조회 문장뿐이다.
 */
export function shownStatements(statements: readonly SqlStatement[]): SqlStatement[] {
  let reads = 0;
  return statements.filter((s) => s.grade !== "read" || ++reads <= STATEMENTS_SHOWN);
}

/** `m:ss`. */
export function clock(ms: number): string {
  const total = Math.max(0, Math.ceil(ms / 1000));
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
}

/** 한 줄 자리의 에이전트 글. 보이지 않는 글자가 있었으면 표지를 단다. */
function AgentLine({
  text,
  max,
  T,
  testId,
}: {
  text: string;
  max?: number;
  T: Strings;
  testId: string;
}) {
  const hidden = hasHidden(text);
  return (
    <span className="flex min-w-0 flex-wrap items-baseline gap-1">
      <span data-testid={testId} className={`${AGENT_BOX} min-w-0 break-all`}>
        {agentLine(text, max)}
      </span>
      {hidden && (
        <span data-testid={`${testId}-invisible`} className="text-[11px] text-ui-accent">
          {T.invisible}
        </span>
      )}
    </span>
  );
}

/** 여러 줄 자리의 에이전트 글(높이를 묶고 그 안에서 굴린다). */
function AgentBlock({
  text,
  max,
  T,
  testId,
  height = "max-h-40",
}: {
  text: string;
  max?: number;
  T: Strings;
  testId: string;
  height?: string;
}) {
  const shown = agentBlock(text, max);
  return (
    <div className="flex flex-col gap-0.5">
      <pre
        data-testid={testId}
        className={`${AGENT_BOX} ${height} overflow-auto break-all whitespace-pre-wrap`}
      >
        {shown.text}
      </pre>
      {shown.cut && (
        <span className="text-[11px] text-ui-muted-foreground">
          {T.textCut(max ?? BLOCK_CHARS)}
        </span>
      )}
    </div>
  );
}

function FileChange({
  change,
  T,
  phrases,
}: {
  change: Extract<PlanChange, { kind: "file" }>;
  T: Strings;
  phrases: Record<string, string>;
}) {
  const content = useMemo(
    () => planDiff(change.path, change.before, change.after),
    [change.path, change.before, change.after],
  );
  const kind = change.before === null ? "added" : change.after === null ? "deleted" : "modified";
  return (
    <div data-testid="mcp-approval-file" data-status={kind} className="flex flex-col gap-1">
      <span className="flex items-baseline gap-2">
        <span className="flex-none text-[12px] font-semibold">{T.file[kind]}</span>
        <AgentLine text={change.path} max={SUMMARY_CHARS} T={T} testId="mcp-approval-path" />
      </span>
      <div className="h-56 overflow-hidden rounded-sm border border-ui-border">
        <DiffView content={content} mode="unified" phrases={phrases} />
      </div>
    </div>
  );
}

/** 문장의 이름: 파일 여럿 · 파일 하나의 계획이면 그 파일과 줄(실행 구성의 `sql` 종류), 글이면 번호와 줄. */
function statementLabel(
  statement: SqlStatement,
  files: Extract<PlanChange, { kind: "sql" }>["files"],
  T: Strings,
): string {
  const file = statement.file === null ? undefined : files[statement.file];
  return file
    ? T.sql.statementIn(statement.index + 1, agentLine(file.path), statement.line)
    : T.sql.statement(statement.index + 1, statement.line);
}

function SqlChange({ change, T }: { change: Extract<PlanChange, { kind: "sql" }>; T: Strings }) {
  const grade = useDbSafetyStrings().grade;
  const shown = shownStatements(change.statements);
  const folded = change.statements.length - shown.length;
  return (
    <div data-testid="mcp-approval-sql" className="flex flex-col gap-1">
      <span className="flex flex-wrap items-center gap-2">
        <span className="text-[12px] font-semibold">{T.sql.connection}</span>
        <ConnectionMark
          name={agentLine(change.connection.name)}
          production={change.connection.production}
          readOnly={change.connection.readOnly}
        />
      </span>
      {change.files.map((file, at) => (
        <span key={at} className="flex items-baseline gap-2">
          <span className="flex-none text-[12px] font-semibold">{T.sql.file}</span>
          <AgentLine text={file.path} max={SUMMARY_CHARS} T={T} testId="mcp-approval-sql-file" />
        </span>
      ))}
      {change.transaction && (
        <p data-testid="mcp-approval-sql-transaction" className="text-[12px]">
          {T.sql.transaction[change.transaction]}
        </p>
      )}
      {change.keepsOnFailure && change.transaction && change.transaction !== "all" && (
        <p role="alert" data-testid="mcp-approval-keeps-on-failure" className="text-ui-accent">
          {T.sql.keepsOnFailure[change.transaction]}
        </p>
      )}
      {change.irreversible && (
        <p role="alert" data-testid="mcp-approval-irreversible" className="text-ui-accent">
          {T.sql.irreversible}
        </p>
      )}
      <p className="text-[12px]">{T.sql.statements(change.statements.length)}</p>
      <ol className="flex flex-col gap-2">
        {shown.map((statement: SqlStatement, at: number) => (
          <li
            key={at}
            data-testid="mcp-approval-statement"
            data-grade={statement.grade}
            className="flex flex-col gap-0.5"
          >
            <span className="flex items-center gap-2 text-[12px]">
              <span className="text-ui-muted-foreground">
                {statementLabel(statement, change.files, T)}
              </span>
              <span
                data-testid="mcp-approval-statement-grade"
                className={`font-semibold ${statement.grade === "destructive" ? "text-ui-accent" : ""}`}
              >
                {grade[statement.grade]}
              </span>
            </span>
            <AgentBlock
              text={statement.text}
              max={STATEMENT_CHARS}
              T={T}
              testId="mcp-approval-statement-text"
              height="max-h-24"
            />
          </li>
        ))}
        {folded > 0 && (
          <li data-testid="mcp-approval-folded" className="text-ui-muted-foreground">
            {T.sql.foldedReads(folded)}
          </li>
        )}
      </ol>
    </div>
  );
}

function Change({
  change,
  T,
  phrases,
}: {
  change: PlanChange;
  T: Strings;
  phrases: Record<string, string>;
}): ReactNode {
  switch (change.kind) {
    case "file":
      return <FileChange change={change} T={T} phrases={phrases} />;
    case "text":
      return (
        <div data-testid="mcp-approval-text" className="flex flex-col gap-1">
          <AgentLine text={change.label} T={T} testId="mcp-approval-text-label" />
          <AgentBlock text={change.text} T={T} testId="mcp-approval-text-body" />
        </div>
      );
    case "sql":
      return <SqlChange change={change} T={T} />;
    default: {
      // 새 갈래를 더하는 행이 여기를 채운다 — 빠뜨리면 타입 검사가 실패한다.
      const unknown: never = change;
      return unknown;
    }
  }
}

/**
 * 바뀔 것 목록. 남은 시한 시계(1초마다)와 잠금이 다이얼로그를 다시 그려도 이 목록은 다시 그리지 않는다(M5-14 재리뷰 G3 — 문장이
 * 수천 개면 한 번 그리는 데 수백 ms 다). 계획 · 문자열이 바뀔 때만 그린다.
 */
const Changes = memo(function Changes({
  changes,
  T,
  phrases,
}: {
  changes: readonly PlanChange[];
  T: Strings;
  phrases: Record<string, string>;
}) {
  if (changes.length === 0)
    return <p className="text-[12px] text-ui-muted-foreground">{T.noChanges}</p>;
  return (
    <section className="flex flex-col gap-2">
      <h3 className="text-[12px] font-semibold">{T.changes(changes.length)}</h3>
      {changes.map((change, i) => (
        // 변경은 자리가 곧 순서다.
        <Change key={i} change={change} T={T} phrases={phrases} />
      ))}
    </section>
  );
});

export interface ApprovalDialogProps {
  request: McpApprovalRequest;
  /** 이 창의 대기 가운데 이 요청의 차례(1 기준)와 전체 수. */
  index: number;
  total: number;
  /** 지금(Unix 밀리초) — 남은 시한. */
  now: number;
  onAnswer(answer: McpApprovalAnswer): void;
  /** `Esc` — 다이얼로그만 닫는다. */
  onDismiss(): void;
  /** 허용 단추를 잠그는 시간(기본 `ARM_DELAY_MS`). 시험만 바꾼다. */
  armDelayMs?: number;
  /** 다시 연 횟수 — 바뀌면 허용 단추를 다시 잠근다(approvals.ts 의 `revealed`). */
  epoch?: number;
}

export function ApprovalDialog({
  request,
  index,
  total,
  now,
  onAnswer,
  onDismiss,
  armDelayMs = ARM_DELAY_MS,
  epoch = 0,
}: ApprovalDialogProps) {
  const T = useMcpStrings().approval;
  useKeymapPopup(true);
  const { plan } = request;
  const third = thirdAnswer(request);
  const machine = request.machine ?? undefined;
  const required = useMemo(() => namesRequired(plan), [plan]);
  const [typed, setTyped] = useState<Record<string, string>>({});
  const namesOk = required.every((name) => typed[name] === name);
  const armed = useArmed(armDelayMs, epoch);
  /** 잠금이 풀린 뒤 포인터가 내려간 허용 단추 — 포인터로 누른 허용은 이 단추여야 받는다. */
  const pressed = useRef<McpApprovalAnswer | undefined>(undefined);
  const press = (answer: McpApprovalAnswer) => {
    pressed.current = armed ? answer : undefined;
  };
  const allow = (answer: McpApprovalAnswer, event: { detail: number }) => {
    if (!armed || !namesOk) return;
    // `detail` 이 0 이면 키보드(Enter · Space)나 프로그램의 누름이다. 포인터 누름은 풀린 뒤에 이 단추에서 시작했어야 한다.
    if (event.detail > 0 && pressed.current !== answer) return;
    onAnswer(answer);
  };
  const phrases = useMemo(() => ({ "$ unchanged lines": T.unchangedLines }), [T]);
  const dangerous = request.tier === "dangerous";
  return (
    <Dialog.Root open onOpenChange={(open) => !open && onDismiss()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content
          data-testid="mcp-approval-dialog"
          data-request={request.id}
          data-tier={request.tier}
          className="sheet-top fixed left-1/2 flex max-h-[85vh] w-[640px] max-w-[94vw] -translate-x-1/2 flex-col gap-3 rounded-b-md border border-t-0 border-ui-border bg-ui-panel px-4 py-3 text-ui-foreground shadow-lg"
          onEscapeKeyDown={keepOpenWhileComposing}
          onPointerDownOutside={(e) => e.preventDefault()}
          onInteractOutside={(e) => e.preventDefault()}
          onOpenAutoFocus={(e) => {
            // 처음 포커스는 [거절] — Radix 가 첫 포커스 가능한 것(이름 칸 · diff)을 고르지 않게 직접 둔다.
            e.preventDefault();
            (e.currentTarget as HTMLElement | null)
              ?.querySelector<HTMLButtonElement>("[data-answer=deny]")
              ?.focus();
          }}
        >
          <Dialog.Title className="flex items-center gap-2 text-sm font-semibold">
            {dangerous && (
              <TriangleAlert size={16} aria-hidden className="flex-none text-ui-accent" />
            )}
            {T.title}
          </Dialog.Title>
          {request.notThisProject && (
            <p role="alert" data-testid="mcp-approval-not-this-project" className="text-ui-accent">
              {T.notThisProject}
            </p>
          )}
          <Dialog.Description asChild>
            <div className="flex min-h-0 flex-1 flex-col gap-2 overflow-auto">
              <p className="text-[12px] text-ui-muted-foreground">{T.agentText}</p>
              <dl className="grid grid-cols-[auto_minmax(0,1fr)] items-baseline gap-x-3 gap-y-1">
                <dt className="text-[12px] text-ui-muted-foreground">{T.agent}</dt>
                <dd className="flex min-w-0 flex-col gap-0.5">
                  {request.client === null ? (
                    <span data-testid="mcp-approval-client">{T.unknownAgent}</span>
                  ) : (
                    <AgentLine text={request.client} T={T} testId="mcp-approval-client" />
                  )}
                  {request.cli && (
                    <span data-testid="mcp-approval-cli" className="text-[12px]">
                      {T.cli}
                    </span>
                  )}
                </dd>
                <dt className="text-[12px] text-ui-muted-foreground">{T.tool}</dt>
                <dd>
                  <AgentLine text={request.tool} T={T} testId="mcp-approval-tool" />
                </dd>
                {machine !== undefined && (
                  <>
                    <dt className="text-[12px] text-ui-muted-foreground">{T.machine}</dt>
                    <dd className="flex min-w-0 flex-col gap-0.5">
                      <span
                        data-testid="mcp-approval-machine"
                        data-machine={machine}
                        className="truncate font-semibold"
                      >
                        <MachineName machine={machine} />
                      </span>
                      <span className="text-[12px] text-ui-muted-foreground">{T.machineNote}</span>
                    </dd>
                  </>
                )}
                <dt className="text-[12px] text-ui-muted-foreground">{T.tierLabel}</dt>
                <dd
                  data-testid="mcp-approval-tier"
                  className={`font-semibold ${dangerous ? "text-ui-accent" : ""}`}
                >
                  {T.tier[request.tier]}
                </dd>
                {plan.summary !== "" && (
                  <>
                    <dt className="text-[12px] text-ui-muted-foreground">{T.summary}</dt>
                    <dd>
                      <AgentLine
                        text={plan.summary}
                        max={SUMMARY_CHARS}
                        T={T}
                        testId="mcp-approval-summary"
                      />
                    </dd>
                  </>
                )}
              </dl>
              <Changes changes={plan.changes} T={T} phrases={phrases} />
            </div>
          </Dialog.Description>
          {required.map((name) => (
            <ConnectionNameField
              key={name}
              name={name}
              typed={typed[name] ?? ""}
              onTyped={(value) => setTyped((t) => ({ ...t, [name]: value }))}
              onSubmit={() => undefined}
              testId="mcp-approval-name"
            />
          ))}
          <div className="flex flex-none items-center gap-2 border-t border-ui-border pt-2">
            <span
              data-testid="mcp-approval-remaining"
              className="text-[12px] text-ui-muted-foreground"
            >
              {T.remaining(clock(request.expiresAt - now))}
            </span>
            <span
              data-testid="mcp-approval-position"
              className="text-[12px] text-ui-muted-foreground"
            >
              {T.position(index, total)}
            </span>
            <span className="flex-1" />
            <button
              type="button"
              data-answer="deny"
              className={BUTTON}
              onClick={() => onAnswer("deny")}
            >
              {T.deny}
            </button>
            {request.answers.includes("once") && (
              <button
                type="button"
                data-answer="once"
                data-locked={armed ? undefined : "true"}
                disabled={!armed || !namesOk}
                title={armed ? undefined : T.locked}
                className={`${BUTTON} inline-flex items-center gap-1 border-ui-accent`}
                onPointerDown={() => press("once")}
                onClick={(e) => allow("once", e)}
              >
                {!armed && <Lock size={12} aria-hidden data-testid="mcp-approval-lock" />}
                {T.once}
              </button>
            )}
            {third !== undefined && (
              <button
                type="button"
                data-answer={third}
                data-locked={armed ? undefined : "true"}
                disabled={!armed || !namesOk}
                title={armed ? undefined : T.locked}
                className={`${BUTTON} inline-flex items-center gap-1`}
                onPointerDown={() => press(third)}
                onClick={(e) => allow(third, e)}
              >
                {!armed && <Lock size={12} aria-hidden data-testid="mcp-approval-lock" />}
                {third === "window" ? T.window : T.session}
              </button>
            )}
            {machine !== undefined && request.answers.includes("machine") && (
              <button
                type="button"
                data-answer="machine"
                data-locked={armed ? undefined : "true"}
                disabled={!armed || !namesOk}
                title={armed ? undefined : T.locked}
                className={`${BUTTON} inline-flex items-center gap-1`}
                onPointerDown={() => press("machine")}
                onClick={(e) => allow("machine", e)}
              >
                {!armed && <Lock size={12} aria-hidden data-testid="mcp-approval-lock" />}
                <MachineName machine={machine}>
                  {(name) => (request.cli ? T.allowMachineCli(name) : T.allowMachine(name))}
                </MachineName>
              </button>
            )}
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
