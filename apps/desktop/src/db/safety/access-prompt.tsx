// DB 접근 묻기(14-m3-plan.md M3-13 — 메인 판정으로 이 행이 주인, §2 「공유 부품의 주인」): 연결을 여는 RPC(탐색기의 `db.schema.*`,
// 콘솔의 `db.session.open`, Redis 의 `db.kv.*`)가 `3101`(비밀 필요) · `3102`(확인 필요)로 답하면 사람에게 묻고 Host 에 넣은 뒤
// 부른 쪽이 다시 부르게 한다(결정 M3-D12 · M3-02 행).
//
// - `3101`: 까닭(없음 · 대상 바뀜 · 키체인 못 씀)과 Host 가 준 대상(이전 → 지금)을 보이고 비밀번호를 받아 `db.secret.set` 에
//   넣는다 — 대상은 Host 가 준 `target` 그대로 돌려준다(그사이 설정이 바뀌었으면 Host 가 conflict). 비밀번호 칸은 `type=password` ·
//   자동 완성 끔이고, 값은 그 RPC 에만 간다 — 로그 · 알림 · 화면 어디에도 남기지 않고 다이얼로그가 닫히면 사라진다.
// - `3102`: Host 가 준 요청(`DbApprovalRequest` — 환경 변수 이름 · 루트 밖 파일의 실제 경로 · SSH 목적지 · 인증서 파일과 대상, 전에
//   확인한 것)을 그대로 보이고, [확인] 이면 그 요청을 그대로(`previous` 만 빼고) `db.connection.approve` 에 돌려준다. 화면이 대상을
//   고쳐 쓰지 않는다.
// - 처음 포커스는 [취소] 다(M2-20 · M3-19 의 확인과 같다 — Enter 한 번으로 비밀이 나가거나 확인되지 않게). 바깥 · Esc 는 취소, 한글
//   조합 중의 Esc 는 조합만 취소한다. 떠 있는 동안 창의 키맵은 쉰다.
// - 그 밖의 오류는 묻지 않는다(`resolve` 가 거짓). 미신뢰 프로젝트는 Host 가 3101 · 3102 전에 신뢰 거절(`untrusted`)을 주므로 이
//   다이얼로그가 뜨지 않는다.
// - 한 번에 하나만 묻는다 — 떠 있는 동안 온 다른 요청은 묻지 않고 거짓이다.

import * as Dialog from "@radix-ui/react-dialog";
import {
  type DbApproval,
  type DbApprovalRequest,
  type DbSecretKey,
  type DbSecretTarget,
  RpcCallError,
  type RpcClient,
  type SecretMissing,
} from "@pitwall/rpc-client";
import { KeyRound, ShieldQuestion } from "lucide-react";
import { type ReactNode, useCallback, useRef, useState } from "react";

import { BUTTON, SHEET } from "../../git/dialogs/git-dialogs";
import { useKeymapPopup } from "../../keymap/keymap-provider";
import { keepOpenWhileComposing } from "../../ui/ime";
import { type DbAccessStrings, useDbAccessStrings } from "./access-strings";
import { DB_ERROR_CODE } from "./messages";

/** `3101` 의 `data`(crates/pw-db/src/trust.rs `code::SECRET_REQUIRED`). */
export interface DbSecretRequired {
  connectionId: string | null;
  key: DbSecretKey;
  reason: SecretMissing;
  target: DbSecretTarget;
  previous: DbSecretTarget | null;
}

export type DbAccessNeed =
  | { kind: "secret"; data: DbSecretRequired }
  | { kind: "approval"; request: DbApprovalRequest };

/** 오류가 `3101` · `3102` 면 물을 것. 아니면 `undefined`. */
export function dbAccessOf(error: unknown): DbAccessNeed | undefined {
  if (!(error instanceof RpcCallError)) return undefined;
  const data = error.data;
  if (!data || typeof data !== "object" || Array.isArray(data)) return undefined;
  const d = data as Record<string, unknown>;
  if (error.code === DB_ERROR_CODE.SECRET_REQUIRED && d.target && typeof d.reason === "string")
    return { kind: "secret", data: d as unknown as DbSecretRequired };
  if (
    error.code === DB_ERROR_CODE.APPROVAL_REQUIRED &&
    d.request &&
    typeof (d.request as { kind?: unknown }).kind === "string"
  )
    return { kind: "approval", request: d.request as DbApprovalRequest };
  return undefined;
}

/** 요청을 확인으로(`previous` 만 뺀다 — 나머지는 Host 가 준 그대로). */
export function approvalOf(request: DbApprovalRequest): DbApproval {
  switch (request.kind) {
    case "env":
      return { kind: "env", variable: request.variable, target: request.target };
    case "file":
      return { kind: "file", realPath: request.realPath };
    case "tunnel":
      return { kind: "tunnel", hostRef: request.hostRef, target: request.target };
    case "certFiles":
      return { kind: "certFiles", files: request.files, target: request.target };
  }
}

/** 물을 연결. */
export interface DbAccessTarget {
  /** Host 호출(`RpcClient`). */
  calls: Pick<RpcClient, "call">;
  projectId: string;
  worktree?: string | null;
  connectionId: string;
  /** 다이얼로그 제목의 연결 이름. */
  connectionName: string;
}

interface Asked {
  need: DbAccessNeed;
  target: DbAccessTarget;
  answer(done: boolean): void;
}

export interface DbAccessPrompt {
  /**
   * `error` 가 `3101` · `3102` 면 사람에게 묻고 Host 에 넣는다. 넣었으면 참 — 부른 쪽이 그 RPC 를 다시 부른다. 취소 · 다른 오류 ·
   * 이미 떠 있으면 거짓(묻지 않는다).
   */
  resolve(error: unknown, target: DbAccessTarget): Promise<boolean>;
  /** 다이얼로그. 부른 컴포넌트가 그린다. */
  element: ReactNode;
}

export function useDbAccessPrompt(): DbAccessPrompt {
  const [asked, setAsked] = useState<Asked>();
  const open = useRef(false);
  const resolve = useCallback((error: unknown, target: DbAccessTarget) => {
    const need = dbAccessOf(error);
    if (!need || open.current) return Promise.resolve(false);
    open.current = true;
    return new Promise<boolean>((done) =>
      setAsked({
        need,
        target,
        answer: (value) => {
          open.current = false;
          setAsked(undefined);
          done(value);
        },
      }),
    );
  }, []);
  const element = asked ? <AccessDialog key={asked.need.kind} asked={asked} /> : undefined;
  return { resolve, element };
}

function targetRows(target: DbSecretTarget, T: DbAccessStrings): [string, string][] {
  const v = (x: string | number | null) => (x === null ? T.none : String(x));
  const rows: [string, string][] = [
    [T.field.driver, target.driver],
    [T.field.host, v(target.host)],
    [T.field.port, v(target.port)],
    [T.field.user, v(target.user)],
    [T.field.tunnel, v(target.tunnel)],
    [T.field.sslmode, target.sslmode],
  ];
  if (target.sslrootcert) rows.push([T.field.sslrootcert, target.sslrootcert]);
  if (target.sslcert) rows.push([T.field.sslcert, target.sslcert]);
  if (target.sslkey) rows.push([T.field.sslkey, target.sslkey]);
  return rows;
}

function Rows({ rows, testId }: { rows: [string, string][]; testId: string }) {
  return (
    <dl data-testid={testId} className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 text-[12px]">
      {rows.map(([label, value]) => (
        <div key={label} className="contents">
          <dt className="text-ui-muted-foreground">{label}</dt>
          <dd className="font-mono break-all select-text">{value}</dd>
        </div>
      ))}
    </dl>
  );
}

/** 요청이 보이는 것(확인할 값과 대상). */
function requestRows(request: DbApprovalRequest, T: DbAccessStrings): [string, string][] {
  switch (request.kind) {
    case "env":
      return [[T.field.variable, request.variable], ...targetRows(request.target, T)];
    case "file":
      return [[T.field.path, request.realPath]];
    case "tunnel":
      return [[T.field.destination, request.hostRef], ...targetRows(request.target, T)];
    case "certFiles": {
      const files: [string, string][] = [];
      if (request.files.sslrootcert) files.push([T.field.sslrootcert, request.files.sslrootcert]);
      if (request.files.sslcert) files.push([T.field.sslcert, request.files.sslcert]);
      if (request.files.sslkey) files.push([T.field.sslkey, request.files.sslkey]);
      return [...files, ...targetRows(request.target, T)];
    }
  }
}

function previousRows(
  request: DbApprovalRequest,
  T: DbAccessStrings,
): [string, string][] | undefined {
  switch (request.kind) {
    case "env":
      return request.previous
        ? [[T.field.variable, request.previous.variable], ...targetRows(request.previous.target, T)]
        : undefined;
    case "tunnel":
      return request.previous
        ? [
            [T.field.destination, request.previous.hostRef],
            ...targetRows(request.previous.target, T),
          ]
        : undefined;
    case "certFiles":
      return request.previous ? targetRows(request.previous.target, T) : undefined;
    case "file":
      return undefined;
  }
}

function AccessDialog({ asked }: { asked: Asked }) {
  const T = useDbAccessStrings();
  useKeymapPopup(true);
  const { need, target } = asked;
  const [secret, setSecret] = useState("");
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState<string>();

  const submit = async () => {
    setBusy(true);
    setFailed(undefined);
    try {
      const base = {
        projectId: target.projectId,
        worktree: target.worktree ?? null,
        connectionId: target.connectionId,
      };
      if (need.kind === "secret")
        await target.calls.call("db.secret.set", {
          ...base,
          key: need.data.key,
          target: need.data.target,
          value: secret,
        });
      else
        await target.calls.call("db.connection.approve", {
          ...base,
          approval: approvalOf(need.request),
        });
      asked.answer(true);
    } catch (e) {
      // 오류 글에 비밀번호가 들어가지 않는다(Host 는 값을 돌려주지 않는다).
      setFailed(e instanceof Error ? e.message : String(e));
      setBusy(false);
    }
  };

  const title =
    need.kind === "secret"
      ? T.secret.title(target.connectionName)
      : T.approve.title[need.request.kind];
  const Icon = need.kind === "secret" ? KeyRound : ShieldQuestion;
  const previous =
    need.kind === "secret"
      ? need.data.previous && targetRows(need.data.previous, T)
      : previousRows(need.request, T);
  const now =
    need.kind === "secret" ? targetRows(need.data.target, T) : requestRows(need.request, T);

  return (
    <Dialog.Root open onOpenChange={(o) => !o && asked.answer(false)}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content
          data-testid="db-access-prompt"
          data-kind={need.kind === "secret" ? "secret" : need.request.kind}
          className={SHEET}
          onEscapeKeyDown={keepOpenWhileComposing}
          onOpenAutoFocus={(e) => {
            // 처음 포커스는 [취소] — Radix 가 첫 포커스 가능한 것(비밀번호 칸)을 고르지 않게 직접 둔다.
            e.preventDefault();
            (e.currentTarget as HTMLElement | null)
              ?.querySelector<HTMLButtonElement>("[data-action=cancel]")
              ?.focus();
          }}
        >
          <Dialog.Title className="flex items-center gap-2 text-sm font-semibold">
            <Icon size={16} aria-hidden className="flex-none text-ui-accent" />
            {title}
          </Dialog.Title>
          <Dialog.Description asChild>
            <p className="text-[12px] text-ui-muted-foreground">
              {need.kind === "secret" ? T.secret.reason[need.data.reason] : T.approve.hint}
            </p>
          </Dialog.Description>
          {previous && (
            <div className="flex flex-col gap-1">
              <span className="text-[12px] font-semibold">{T.previous}</span>
              <Rows rows={previous} testId="db-access-previous" />
              <span className="text-[12px] font-semibold">{T.now}</span>
            </div>
          )}
          <Rows rows={now} testId="db-access-now" />
          {need.kind === "secret" && (
            <label className="flex flex-col gap-1 text-[12px]">
              {T.secret.label}
              <input
                type="password"
                autoComplete="off"
                spellCheck={false}
                data-testid="db-access-secret"
                value={secret}
                onChange={(e) => setSecret(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && !e.nativeEvent.isComposing && !busy) void submit();
                }}
                className="rounded-sm border border-ui-border bg-transparent px-2 py-1 outline-none focus:border-ui-accent"
              />
            </label>
          )}
          {failed !== undefined && (
            <p role="alert" data-testid="db-access-failed" className="text-[12px] text-ui-accent">
              {T.failed(failed)}
            </p>
          )}
          <div className="flex justify-end gap-2">
            <button
              type="button"
              data-action="cancel"
              className={BUTTON}
              onClick={() => asked.answer(false)}
            >
              {T.cancel}
            </button>
            <button
              type="button"
              data-action="run"
              disabled={busy}
              className={`${BUTTON} border-ui-accent`}
              onClick={() => void submit()}
            >
              {need.kind === "secret" ? T.secret.save : T.approve.run}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
