// SQL 콘솔 탭 하나의 세션 · 실행(14-m3-plan.md M3-15). 화면(console-document.tsx)과 떨어진 상태 기계라 창 시험이 RPC 순서를 본다.
//
// - **연결은 사람이 누를 때만**(결정 M3-D13): 탭이 처음 그려질 때는 `db.session.resume { projectId, tabId }` 만 부른다 — 새로고침 ·
//   웹뷰 재시작 뒤 Host 가 보존한 열린 트랜잭션 세션이 있으면 그대로 잇고(새 연결을 열지 않는다), 없으면 연결하지 않은 채다.
//   [연결] 이 `db.session.open { …, tabId }` 를 부른다 — `tabId` 는 탭을 복원해도 같다. `3101` · `3102` 면 DB 접근 묻기(M3-13)로
//   비밀 · 확인을 받아 넣고 다시 연다.
// - **실행**: `db.sql.classify { sessionId, statements }` 로 등급을 미리 받아 확인이 필요하면(M3-19 `confirmationFor` — 정책의 정본은
//   Host) 위험 쿼리 확인을 띄우고, 받은 `DbConfirmed`(statementsHash · 친 이름)를 `confirmed` 로 실어 `db.query.execute` 를 부른다.
//   Host 가 그래도 `3001` 을 주면(미리 본 뒤 세션 상태가 바뀜) 그 `data` 로 한 번 더 묻는다. [취소] 면 실행 RPC 를 부르지 않는다.
// - **취소**(■, ⌘F2): `db.query.cancel { executionId }`. `3002`(SQLite 수동 트랜잭션의 쓰기 — 취소가 트랜잭션 전체를 되돌린다)면
//   확인을 받고 `confirmRollback` 으로 다시 부른다.
// - **실행 계획**: `db.query.explain` — 확인은 실행과 같다(관문이 같다).
// - **세션 상태**는 `db.sessionChanged` 를 따른다. `failed`(오류 뒤 롤백만 되는 트랜잭션)면 [커밋] 을 끈다. 끊김 · 보존 롤백은
//   세션을 잃은 것으로 둔다 — 다시 붙는 것은 사람이 [연결] 을 누를 때다.
// - **닫기 전에 묻기**(결정 M3-D21): 열린 트랜잭션(`inTransaction` · `failed` · 설정이 바뀐 세션의 열린 트랜잭션)이 있으면
//   `needsCloseAnswer()` 가 참이다. 묻는 것은 화면의 몫이고 답(`commit` · `rollback`)을 `settleClose` 가 한다.

import type {
  DbConfirmed,
  DbConfirmRequired,
  DbSessionChanged,
  DbSessionInfo,
  DbSessionMode,
  DbSessionNotice,
  DbSessionOpened,
  ExplainPlan,
  RpcClient,
} from "@pitwall/rpc-client";
import { RpcCallError } from "@pitwall/rpc-client";
import { createStore, type StoreApi } from "zustand/vanilla";

import { QueryRun } from "../grid/query-stream";
import { DB_ERROR_CODE, confirmRequiredOf } from "../safety/messages";
import {
  confirmationFor,
  type RiskConfirmAnswer,
  type RiskConfirmQuestion,
} from "../safety/risk-confirm";

/** 콘솔 탭의 저장되는 것(탭을 복원해도 같다). */
export interface ConsoleSpec {
  /** 세션을 보존 · 재개하는 탭 id(`db.session.open { tabId }` · `db.session.resume`). */
  tabId: string;
  /** 콘솔 파일(`project.console.*` 의 `consoleId`). */
  consoleId: string;
  connectionId?: string | null;
  /** 고른 DB(없으면 연결 설정의 것). */
  database?: string | null;
  /** 고른 스키마(없으면 서버 기본 — 스키마가 있는 드라이버만). */
  schema?: string | null;
}

export interface ConsoleDeps {
  client(): RpcClient | undefined;
  projectId: string;
  /** 활성 워크트리의 꼭대기(없으면 메인 체크아웃). 연결 설정을 그 워크트리에서 읽는다. */
  worktree(): string | undefined;
  /** 위험 쿼리 확인(M3-19 `useRiskConfirm().confirm`). */
  confirmRisk(question: RiskConfirmQuestion): Promise<RiskConfirmAnswer>;
  /** `3002` — 「취소하면 트랜잭션 전체가 롤백된다」 를 묻는다. 계속하면 `true`. */
  confirmCancelRollback(): Promise<boolean>;
  /** 저장되는 것이 바뀌었다(연결 · DB · 스키마). */
  onSpec(spec: ConsoleSpec): void;
  /** 연결 표지 색(연결 설정의 `color`) — 확인 다이얼로그에 싣는다. */
  color?(): string | null | undefined;
  /**
   * 연결을 여는 RPC 가 `3101`(비밀 필요) · `3102`(확인 필요)로 답했을 때 사람에게 묻고 Host 에 넣는다(M3-13 `useDbAccessPrompt`).
   * 넣었으면 `true` — 그 RPC 를 다시 부른다. `connectionId` 는 그 RPC 에 실은 것(다르면 묻지 않는다). 탭이 사라지면 `false` 로 끝난다.
   */
  resolveAccess?(error: unknown, connectionId: string): Promise<boolean>;
}

/** 실행 하나: 그 결과(문장마다의 버퍼)와 도는 중인지. */
export interface ConsoleRun {
  query: QueryRun;
  running: boolean;
}

export interface ConsoleState {
  spec: ConsoleSpec;
  /** 연 세션(`db.session.open` · `resume` 의 답 — 세션 상태는 `db.sessionChanged` 를 따른다). 없으면 연결하지 않은 채다. */
  opened?: DbSessionOpened;
  /** 탭이 처음 그려진 뒤 `db.session.resume` 을 마쳤다(그 전에는 [연결] 을 끈다). */
  resumed: boolean;
  /** 세션을 여는 · 닫는 · 커밋하는 중. */
  busy: boolean;
  /** 마지막 오류(거절 · DB 오류 · 연결 실패) — 위 막대에 보인다. 다음 동작이 지운다. */
  error?: unknown;
  /** 마지막 세션 알림(롤백됨 · 끊김 등). */
  notice?: DbSessionNotice;
  run?: ConsoleRun;
  /** 결과 영역이 보이는 것: 문장의 결과 탭(`run.query.results` 의 순서) 또는 실행 계획. */
  shown: { kind: "result"; index: number } | { kind: "plan" };
  plan?: ExplainPlan;
}

/** 세션에 열린 트랜잭션이 있다(닫기 전에 [커밋] [롤백] [취소] 를 묻는다 — 결정 M3-D21). */
export function hasOpenTransaction(session: DbSessionInfo | undefined): boolean {
  if (!session) return false;
  const { state } = session;
  return (
    state.kind === "inTransaction" ||
    state.kind === "failed" ||
    (state.kind === "configChanged" && state.transaction !== null)
  );
}

/** 세션이 오류 뒤 롤백만 되는 트랜잭션이다 — [커밋] 을 끈다. */
export function isFailed(session: DbSessionInfo | undefined): boolean {
  return session?.state.kind === "failed";
}

/** 세션을 잃었다(끊김) — 실행 · 커밋을 받지 않는다. 다시 붙는 것은 [연결]. */
function isGone(session: DbSessionInfo): boolean {
  return session.state.kind === "disconnected";
}

/** 오류에 Host 가 실어 준 세션 상태(`3003` 의 `data.session`). */
function sessionOf(error: unknown): DbSessionInfo | undefined {
  if (!(error instanceof RpcCallError) || error.code !== DB_ERROR_CODE.DB_ERROR) return undefined;
  const data = error.data;
  if (!data || typeof data !== "object" || Array.isArray(data)) return undefined;
  const session = (data as Record<string, unknown>).session;
  return session && typeof session === "object" ? (session as DbSessionInfo) : undefined;
}

export type ConsoleStore = StoreApi<ConsoleState> & ReturnType<typeof consoleActions>;

/** [연결] 한 번에 물어 넣고 다시 여는 최대 수. */
const ACCESS_RETRIES = 4;

export function createConsoleStore(spec: ConsoleSpec, deps: ConsoleDeps): ConsoleStore {
  const store = createStore<ConsoleState>()(() => ({
    spec,
    resumed: false,
    busy: false,
    shown: { kind: "result", index: 0 },
  }));
  return Object.assign(store, consoleActions(store, deps));
}

function consoleActions(store: StoreApi<ConsoleState>, deps: ConsoleDeps) {
  const get = store.getState;
  const set = store.setState;
  const client = (): RpcClient => {
    const c = deps.client();
    if (!c) throw new Error("the window is not connected to its Host");
    return c;
  };
  /** 지금 실행의 취소 신호(스트림을 그만 읽는다). */
  let abort: AbortController | undefined;
  /** 탭이 닫혔다 — 그 뒤에 돌아온 세션은 곧바로 닫는다. */
  let disposed = false;

  const connection = (opened: DbSessionOpened): RiskConfirmQuestion["connection"] => ({
    name: opened.name,
    production: opened.production,
    readOnly: opened.readOnly,
    color: deps.color?.() ?? null,
  });

  const setSession = (session: DbSessionInfo) => {
    const opened = get().opened;
    if (opened && opened.session.sessionId === session.sessionId)
      set({ opened: { ...opened, session } });
  };

  const updateSpec = (change: Partial<ConsoleSpec>) => {
    const next = { ...get().spec, ...change };
    set({ spec: next });
    deps.onSpec(next);
  };

  /** 확인이 필요하면 묻는다. 계속하면 확인(없으면 `null`), 취소면 `undefined`. */
  async function ask(
    opened: DbSessionOpened,
    request: DbConfirmRequired | undefined,
    sql: string,
  ): Promise<DbConfirmed | null | undefined> {
    if (!request) return null;
    const answer = await deps.confirmRisk({ request, connection: connection(opened), sql });
    return answer === undefined || answer === "busy" ? undefined : answer;
  }

  /** 미리 분류해 확인을 받고(`classify`), `call` 이 `3001` 을 주면 그 `data` 로 한 번 더 묻는다. */
  async function confirmed<T>(
    opened: DbSessionOpened,
    sql: string,
    statements: number[] | undefined,
    call: (confirmed: DbConfirmed | null) => Promise<T>,
  ): Promise<{ ok: true; value: T } | { ok: false }> {
    const classification = await client().call("db.sql.classify", {
      projectId: deps.projectId,
      worktree: deps.worktree() ?? null,
      sessionId: opened.session.sessionId,
      sql,
      statements: statements ?? null,
    });
    let answer = await ask(opened, confirmationFor(classification, opened), sql);
    for (let attempt = 0; ; attempt++) {
      if (answer === undefined) return { ok: false };
      try {
        return { ok: true, value: await call(answer) };
      } catch (e) {
        const request = confirmRequiredOf(e);
        if (!request || attempt > 0) throw e;
        answer = await ask(opened, request, sql);
      }
    }
  }

  const actions = {
    /** 탭이 처음 그려질 때: Host 가 보존한 세션을 찾아 잇는다. 새 연결은 열지 않는다(결정 M3-D13). */
    async resume() {
      if (get().resumed) return;
      try {
        const found = await client().call("db.session.resume", {
          projectId: deps.projectId,
          tabId: get().spec.tabId,
        });
        // 돌아오기 전에 탭이 닫혔으면 보존 세션을 아무도 닫지 않는다 — 곧바로 닫는다(사람이 탭을 닫았다. 열린 트랜잭션은
        // DB 가 롤백한다).
        if (found && disposed) {
          await client()
            .call("db.session.close", { sessionId: found.session.sessionId })
            .catch(() => undefined);
          return;
        }
        if (found && !get().opened) set({ opened: found });
      } catch (e) {
        set({ error: e });
      } finally {
        set({ resumed: true });
      }
    },

    /** [연결]: 고른 연결 · DB · 스키마로 세션을 연다. */
    async connect() {
      const { spec, opened, busy } = get();
      if (opened || busy || !spec.connectionId) return;
      set({ busy: true, error: undefined, notice: undefined });
      const params = {
        projectId: deps.projectId,
        worktree: deps.worktree() ?? null,
        connectionId: spec.connectionId,
        database: spec.database ?? null,
        schema: spec.schema ?? null,
        tabId: spec.tabId,
      };
      try {
        // 묻고 넣은 뒤 다시 연다. 한 연결이 여럿을 차례로 물을 수 있다(터널 확인 → 비밀번호) — 끝없이 돌지 않게 수를 둔다.
        for (let attempt = 0; ; attempt++) {
          try {
            const next = await client().call("db.session.open", params);
            // [연결] 이 도는 중에 탭이 닫혔으면 아무도 닫지 않을 세션이 창이 닫힐 때까지 남는다 — 곧바로 닫는다.
            if (disposed) {
              await client()
                .call("db.session.close", { sessionId: next.session.sessionId })
                .catch(() => undefined);
              return;
            }
            set({ opened: next });
            return;
          } catch (e) {
            if (
              disposed ||
              attempt >= ACCESS_RETRIES ||
              !(await deps.resolveAccess?.(e, params.connectionId))
            )
              throw e;
          }
        }
      } catch (e) {
        set({ error: e });
      } finally {
        set({ busy: false });
      }
    },

    /** 세션을 닫는다(열린 트랜잭션은 DB 가 롤백한다 — 묻는 것은 부르는 쪽). */
    async disconnect() {
      const opened = get().opened;
      if (!opened) return;
      abort?.abort();
      set({ opened: undefined, notice: undefined });
      try {
        await client().call("db.session.close", { sessionId: opened.session.sessionId });
      } catch (e) {
        set({ error: e });
      }
    },

    /**
     * 연결 · DB · 스키마를 바꾼다. 세션이 열려 있으면 닫고 새것으로 다시 연다 — 열린 트랜잭션이 있으면 바꾸지 않는다(화면이 고르기를
     * 끈다).
     */
    async choose(change: Pick<ConsoleSpec, "connectionId" | "database" | "schema">) {
      const opened = get().opened;
      if (hasOpenTransaction(opened?.session) || get().run?.running) return;
      updateSpec(change);
      if (!opened) return;
      await actions.disconnect();
      await actions.connect();
    },

    async setMode(mode: DbSessionMode) {
      const opened = get().opened;
      if (!opened || opened.session.mode === mode) return;
      set({ error: undefined });
      try {
        setSession(
          await client().call("db.session.setMode", { sessionId: opened.session.sessionId, mode }),
        );
      } catch (e) {
        set({ error: e });
      }
    },

    async commit(): Promise<boolean> {
      return actions.finish("db.session.commit");
    },

    async rollback(): Promise<boolean> {
      return actions.finish("db.session.rollback");
    },

    /** [커밋] · [롤백]. 끝났으면 `true`. 실패하면(커밋이 롤백됨 · 이미 끝난 트랜잭션) Host 가 준 세션 상태로 맞추고 오류를 보인다. */
    async finish(method: "db.session.commit" | "db.session.rollback"): Promise<boolean> {
      const opened = get().opened;
      if (!opened) return true;
      set({ busy: true, error: undefined });
      try {
        setSession(await client().call(method, { sessionId: opened.session.sessionId }));
        return true;
      } catch (e) {
        const session = sessionOf(e);
        if (session) setSession(session);
        set({ error: e });
        return false;
      } finally {
        set({ busy: false });
      }
    },

    /** 문장들을 돌린다(`statements` 가 없으면 전부 — 「전체 실행」). 돌릴 문장이 없으면 아무것도 하지 않는다. */
    async run(sql: string, statements: number[] | undefined) {
      const opened = get().opened;
      if (!opened || get().run?.running || isGone(opened.session)) return;
      if (statements && statements.length === 0) return;
      set({ error: undefined, notice: undefined });
      try {
        await confirmed(opened, sql, statements, async (answer) => {
          const query = new QueryRun();
          const controller = new AbortController();
          abort = controller;
          set({ run: { query, running: true }, shown: { kind: "result", index: 0 } });
          try {
            await query.run(
              client(),
              {
                sessionId: opened.session.sessionId,
                sql,
                statements: statements ?? null,
                confirmed: answer,
              },
              controller.signal,
            );
          } finally {
            if (abort === controller) abort = undefined;
            set((s) => (s.run?.query === query ? { run: { query, running: false } } : s));
          }
        });
      } catch (e) {
        set({ error: e });
      }
    },

    /** ■ 취소(⌘F2). 돌고 있지 않으면 아무것도 하지 않는다. */
    async cancel() {
      const run = get().run;
      const executionId = run?.running ? run.query.executionId : undefined;
      if (!executionId) return;
      try {
        await client().call("db.query.cancel", { executionId });
      } catch (e) {
        if (!(e instanceof RpcCallError) || e.code !== DB_ERROR_CODE.CANCEL_ROLLS_BACK) {
          set({ error: e });
          return;
        }
        if (!(await deps.confirmCancelRollback())) return;
        try {
          await client().call("db.query.cancel", { executionId, confirmRollback: true });
        } catch (again) {
          set({ error: again });
        }
      }
    },

    /** 실행 계획(문장 하나). */
    async explain(sql: string, statements: number[]) {
      const opened = get().opened;
      if (!opened || get().run?.running || statements.length !== 1) return;
      set({ error: undefined, busy: true });
      try {
        const result = await confirmed(opened, sql, statements, (answer) =>
          client().call("db.query.explain", {
            sessionId: opened.session.sessionId,
            sql,
            statements,
            confirmed: answer,
          }),
        );
        if (result.ok) set({ plan: result.value, shown: { kind: "plan" } });
      } catch (e) {
        set({ error: e });
      } finally {
        set({ busy: false });
      }
    },

    show(shown: ConsoleState["shown"]) {
      set({ shown });
    },

    /** `db.sessionChanged`. 이 탭의 세션 것만 받는다. */
    applySessionChanged(event: DbSessionChanged) {
      const opened = get().opened;
      if (!opened || opened.session.sessionId !== event.session.sessionId) return;
      if (event.notice?.kind === "unattendedRolledBack") {
        // Host 가 롤백하고 세션을 닫았다 — 그 세션은 없다.
        set({ opened: undefined, notice: event.notice });
        return;
      }
      set({
        opened: { ...opened, session: event.session },
        ...(event.notice && { notice: event.notice }),
      });
    },

    /** 닫기 전에 물을 것이 있다(열린 트랜잭션). */
    needsCloseAnswer(): boolean {
      return hasOpenTransaction(get().opened?.session);
    },

    /** 닫기 질문의 답을 한다. 닫아도 되면 `true` — 커밋이 실패하면 닫지 않는다(오류가 위 막대에 보인다). */
    async settleClose(answer: "commit" | "rollback" | "cancel"): Promise<boolean> {
      if (answer === "cancel") return false;
      return answer === "commit" ? actions.commit() : actions.rollback();
    },

    /** 탭이 닫혔다: 세션을 닫는다(답을 받은 뒤라 열린 트랜잭션은 없다 — 남았으면 DB 가 롤백한다). [연결] · 재개(`resume`)가 도는 중이면 돌아온 세션을 곧바로 닫는다. */
    async dispose() {
      disposed = true;
      abort?.abort();
      await actions.disconnect();
    },
  };
  return actions;
}
