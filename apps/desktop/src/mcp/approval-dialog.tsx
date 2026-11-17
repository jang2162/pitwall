// MCP 승인 다이얼로그(16-m5-plan M5-14, 10-ui.md 「다이얼로그 · 팝업 목록」 의 MCP 행). 요청 에이전트 · 툴 · 등급 · 바뀔 것을
// 보이고 [거절] [이번만 허용] [이 세션 동안 허용] 으로 답한다. 무엇을 띄울지 · 답을 보내는 것은 approvals-host.tsx 다.
//
// - 셋째 단추: CLI 요청이면 「창이 열려 있는 동안 CLI 허용」(`window`, 결정 M5-D5), 아니면 「이 세션 동안 허용」(`session`).
//   `dangerous` 와 기억하지 않는 계획(`open_with_upsert` — `rules.remember` 가 `never`, 결정 M5-D31)에는 없다. Host 가 요청에
//   실은 `answers` 에 없는 답도 그리지 않는다 — 둘 다 맞아야 보인다.
// - **처음 포커스는 [거절]**(신뢰 확인 · 위험 작업 확인과 같은 규칙) — 뜬 순간 친 Enter · Space 가 허용이 되지 않게. `Esc` 는
//   다이얼로그만 닫는다(요청은 「에이전트 활동」 의 「승인 대기」 에 남는다). 바깥을 눌러도 닫히지 않는다.
// - 에이전트가 보낸 글(클라이언트 이름 · 요약 · 경로 · 바뀔 글 · 문장)은 테두리 친 칸 안에만 그리고 넓은 규칙(agent-text.ts)으로
//   거른다 — 한 줄 자리는 줄바꿈도 표지가 되고, 여러 줄 자리는 높이를 묶어 그 안에서 굴린다. 단추 줄은 굴리는 칸 밖(아래)에
//   늘 있다. 그래서 긴 글 · 줄바꿈 · 단추처럼 꾸민 글이 다이얼로그의 칸이나 단추처럼 보이지 않고 단추를 화면 밖으로 밀지 못한다.
// - SQL(`PlanChange` 의 `sql`): 연결 표지(프로덕션 · 읽기 전용), 파일이면 경로, 「되돌릴 수 없다」, 문장마다 등급. 파괴 등급 문장이
//   하나라도 있으면 그 연결 이름을 쳐야 허용 단추가 켜진다(위험 쿼리 확인과 같은 부품 — db/safety/name-confirm.tsx).
// - 남은 시한과 「n개 중 k」 를 보인다. 시한이 지나면 approvals.ts 가 요청을 빼 다이얼로그가 내려간다.

import * as Dialog from "@radix-ui/react-dialog";
import type {
  McpApprovalAnswer,
  McpApprovalRequest,
  PlanChange,
  SqlStatement,
  ToolPlan,
} from "@pitwall/rpc-client";
import { TriangleAlert } from "lucide-react";
import { type ReactNode, useMemo, useState } from "react";

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

/** 문장 목록에 보이는 문장 수. 넘는 것은 「그리고 n개 더」(승인은 모든 문장에 닿는다). */
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

function SqlChange({ change, T }: { change: Extract<PlanChange, { kind: "sql" }>; T: Strings }) {
  const grade = useDbSafetyStrings().grade;
  const shown = change.statements.slice(0, STATEMENTS_SHOWN);
  const hidden = change.statements.length - shown.length;
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
      {change.file && (
        <span className="flex items-baseline gap-2">
          <span className="flex-none text-[12px] font-semibold">{T.sql.file}</span>
          <AgentLine
            text={change.file.path}
            max={SUMMARY_CHARS}
            T={T}
            testId="mcp-approval-sql-file"
          />
        </span>
      )}
      {change.irreversible && (
        <p role="alert" data-testid="mcp-approval-irreversible" className="text-ui-accent">
          {T.sql.irreversible}
        </p>
      )}
      <p className="text-[12px]">{T.sql.statements(change.statements.length)}</p>
      <ol className="flex flex-col gap-2">
        {shown.map((statement: SqlStatement) => (
          <li
            key={statement.index}
            data-testid="mcp-approval-statement"
            data-grade={statement.grade}
            className="flex flex-col gap-0.5"
          >
            <span className="flex items-center gap-2 text-[12px]">
              <span className="text-ui-muted-foreground">
                {T.sql.statement(statement.index + 1, statement.line)}
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
        {hidden > 0 && <li className="text-ui-muted-foreground">{T.sql.more(hidden)}</li>}
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
}

export function ApprovalDialog({
  request,
  index,
  total,
  now,
  onAnswer,
  onDismiss,
}: ApprovalDialogProps) {
  const T = useMcpStrings().approval;
  useKeymapPopup(true);
  const { plan } = request;
  const third = thirdAnswer(request);
  const required = useMemo(() => namesRequired(plan), [plan]);
  const [typed, setTyped] = useState<Record<string, string>>({});
  const namesOk = required.every((name) => typed[name] === name);
  const allow = (answer: McpApprovalAnswer) => {
    if (namesOk) onAnswer(answer);
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
              {plan.changes.length === 0 ? (
                <p className="text-[12px] text-ui-muted-foreground">{T.noChanges}</p>
              ) : (
                <section className="flex flex-col gap-2">
                  <h3 className="text-[12px] font-semibold">{T.changes(plan.changes.length)}</h3>
                  {plan.changes.map((change, i) => (
                    // 변경은 자리가 곧 순서다.
                    <Change key={i} change={change} T={T} phrases={phrases} />
                  ))}
                </section>
              )}
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
                disabled={!namesOk}
                className={`${BUTTON} border-ui-accent`}
                onClick={() => allow("once")}
              >
                {T.once}
              </button>
            )}
            {third !== undefined && (
              <button
                type="button"
                data-answer={third}
                disabled={!namesOk}
                className={BUTTON}
                onClick={() => allow(third)}
              >
                {third === "window" ? T.window : T.session}
              </button>
            )}
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
