// SQL 콘솔 문서(14-m3-plan.md M3-15, 10-ui.md 「에디터 영역 문서 타입」): 위 막대(연결 · DB/스키마 고르기 · [연결] · ▶ 실행 ⌘↩ ·
// ■ 취소 ⌘F2 · 실행 계획 · TX 자동/수동 + [커밋] [롤백] · 프로덕션 · read-only 표지), CodeMirror(SQL 강조), 그 아래 결과(문장마다
// 탭 하나 — M3-16 의 `ResultView`, 실행 계획은 들여쓴 트리). 콘솔과 결과 사이의 경계는 끌어 옮긴다.
//
// 세션 · 실행 · 확인의 순서는 console-store.ts 머리 주석. 여기는 그 상태를 그리고 창에 잇는다:
// - 콘솔 글은 콘솔 파일(`project.console.read` · `write`, D36 — 미저장 개념 없음)이다. 편집을 `SAVE_DELAY_MS` 모아 쓰고, 탭이 닫힐
//   때 마저 쓴다(비었으면 지운다).
// - 상태바 `db` 항목(M3-19)에 이 탭의 세션을 둔다(`useDbTabSession`). 연결 표지 색이 콘솔 테두리에 든다(프로덕션은 빨강 2px).
// - 닫기 전에 묻기: 작업 공간의 `guardClose` 에 등록한다 — 열린 트랜잭션이 있으면 창의 질문(`consoleWindow.askTransaction`)으로
//   [커밋] [롤백] [취소] 를 묻는다(결정 M3-D21).
// - DB · 스키마 목록은 세션을 연 뒤에만 읽는다 — 목록을 읽는 것도 연결을 여는 일이라 사람이 [연결] 을 누르기 전에는 하지 않는다
//   (결정 M3-D13). 트랜잭션이 열려 있으면 연결 · DB · 스키마 고르기를 끈다.

import type { DbSessionChanged, DbSessionMode, ExplainPlan, RpcClient } from "@pitwall/rpc-client";
import { Play, Square } from "lucide-react";
import {
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
  useCallback,
  useEffect,
  useMemo,
  useReducer,
  useRef,
  useState,
} from "react";
import { useStore } from "zustand";
import type { EditorView } from "@codemirror/view";

import { useEditorWorkspace } from "../../editor/editor-workspace";
import type { SyntaxSource } from "../../editor/syntax-sessions";
import { useGitDialogs } from "../../git/dialogs/git-dialogs";
import { useNotify } from "../../notifications/notifications";
import { useActiveWorktree, activeWorktree } from "../../open-with/active-worktree";
import { useHost, useRpcQuery } from "../../rpc/host";
import { ResultView } from "../grid/result-view";
import type { StatementResult } from "../grid/query-stream";
import { ConnectionMark, TlsMark, connectionFrameStyle } from "../safety/connection-mark";
import {
  DB_SESSION_CHANGED_TOPIC,
  type DbActiveSession,
  useDbTabSession,
} from "../safety/db-status";
import { useDbAccessPrompt } from "../safety/access-prompt";
import { DbRefusalMessage, dbNoticeText, dbRefusal } from "../safety/messages";
import { useRiskConfirm } from "../safety/risk-confirm";
import { useDbSafetyStrings } from "../safety/strings";
import { ConsoleEditor } from "./console-editor";
import {
  type ConsoleSpec,
  type ConsoleStore,
  createConsoleStore,
  hasOpenTransaction,
  isFailed,
} from "./console-store";
import { consoleWindow, registerConsole, rememberConnection } from "./console-window";
import { type PlanNode, planTree } from "./plan-tree";
import { statementsToRun } from "./run-scope";
import { type DbConsoleStrings, useDbConsoleStrings } from "./strings";

/** 편집을 모아 쓰는 시간. */
export const SAVE_DELAY_MS = 300;

const BUTTON =
  "flex h-[22px] items-center gap-1 rounded-sm border border-ui-border px-2 text-[12px] hover:bg-ui-hover disabled:opacity-50 disabled:hover:bg-transparent";
const SELECT =
  "h-[22px] max-w-[180px] rounded-sm border border-ui-border bg-ui-panel px-1 text-[12px] disabled:opacity-50";

export interface ConsoleDocumentProps {
  /** 탭(dockview 패널) id. */
  panelId: string;
  spec: ConsoleSpec;
  /** 저장되는 것이 바뀌었다(탭 params 에 둔다). */
  onSpec(spec: ConsoleSpec): void;
  /** 탭 이름을 바꾼다(연결 이름). */
  onTitle?(title: string): void;
}

/**
 * 콘솔 파일을 읽고, 편집을 모아 쓴다. 탭이 닫힐 때 `flush` 로 마저 쓴다.
 *
 * - **읽기가 성공한 뒤에만 쓰고 지운다.** 읽기 전(느린 읽기 중 탭 닫기)이나 읽지 못했을 때(16 MiB 초과 · UTF-8 아님 · 읽기 오류)
 *   쓰면 그 파일을 빈 글이나 친 글로 덮고, 닫으면 빈 글로 보고 지운다 — 저장된 콘솔이 사라진다. 읽지 못했으면 편집기를 읽기 전용으로
 *   두고(`readFailed`) [다시 읽기](`reload`)를 준다.
 * - **쓰기는 하나씩**: 앞 쓰기가 끝나기 전에는 다음을 보내지 않고 마지막 글만 남긴다 — Host 의 원자적 쓰기가 늦게 끝나면 옛 글이
 *   새 글 뒤에 이름을 바꿔 사람의 글이 사라진다. 지우기도 기다리던 쓰기가 끝난 뒤에 보낸다.
 */
function useConsoleFile(
  client: RpcClient | undefined,
  projectId: string | undefined,
  consoleId: string,
) {
  const [initial, setInitial] = useState<string>();
  const [readFailed, setReadFailed] = useState<string>();
  /** 다시 읽은 횟수 — 편집기를 새로 만드는 열쇠. */
  const [generation, setGeneration] = useState(0);
  const [error, setError] = useState<string>();
  const pending = useRef<{ text: string; timer: ReturnType<typeof setTimeout> }>(undefined);
  /** 읽기가 성공했다(그 뒤에만 쓰고 지운다). */
  const loaded = useRef(false);
  const queue = useRef<{ running?: Promise<void>; next?: string }>({});
  const latest = useRef({ client, projectId });
  latest.current = { client, projectId };
  useEffect(() => {
    if (!client || !projectId || initial !== undefined || readFailed !== undefined) return;
    let live = true;
    client.call("project.console.read", { projectId, consoleId }).then(
      (read) => {
        if (!live) return;
        loaded.current = true;
        setInitial(read.text ?? "");
      },
      (e: unknown) => {
        if (!live) return;
        setReadFailed(e instanceof Error ? e.message : String(e));
        setInitial("");
      },
    );
    return () => {
      live = false;
    };
  }, [client, projectId, consoleId, initial, readFailed]);
  const reload = useCallback(() => {
    setReadFailed(undefined);
    setInitial(undefined);
    setGeneration((n) => n + 1);
  }, []);
  const write = useCallback(
    (text: string): Promise<void> => {
      const q = queue.current;
      q.next = text;
      if (q.running) return q.running;
      const drain = async () => {
        while (q.next !== undefined) {
          const next = q.next;
          q.next = undefined;
          const { client, projectId } = latest.current;
          if (!client || !projectId || !loaded.current) return;
          await client.call("project.console.write", { projectId, consoleId, text: next }).then(
            () => setError(undefined),
            (e: unknown) => setError(e instanceof Error ? e.message : String(e)),
          );
        }
      };
      q.running = drain().finally(() => {
        q.running = undefined;
      });
      return q.running;
    },
    [consoleId],
  );
  const changed = useCallback(
    (text: string) => {
      if (!loaded.current) return;
      if (pending.current) clearTimeout(pending.current.timer);
      const timer = setTimeout(() => {
        pending.current = undefined;
        void write(text);
      }, SAVE_DELAY_MS);
      pending.current = { text, timer };
    },
    [write],
  );
  /** 기다리던 쓰기를 곧바로 한다. 글이 비었고 `removeEmpty` 면 파일을 지운다(닫은 빈 콘솔이 쌓이지 않게). */
  const flush = useCallback(
    async (current: string | undefined, removeEmpty: boolean) => {
      const waiting = pending.current;
      if (waiting) clearTimeout(waiting.timer);
      pending.current = undefined;
      // 읽기 전 · 읽지 못한 파일은 그대로 둔다.
      if (!loaded.current) return;
      const text = waiting?.text ?? current;
      const { client, projectId } = latest.current;
      if (removeEmpty && text !== undefined && text.trim() === "" && client && projectId) {
        await queue.current.running;
        await client
          .call("project.console.delete", { projectId, consoleId })
          .catch(() => undefined);
        return;
      }
      if (waiting) await write(waiting.text);
    },
    [consoleId, write],
  );
  // 창이 내려갈 때(새로고침 · 닫기) 기다리던 쓰기를 마저 보낸다.
  useEffect(() => {
    const out = () => void flush(undefined, false);
    window.addEventListener("pagehide", out);
    return () => {
      window.removeEventListener("pagehide", out);
      out();
    };
  }, [flush]);
  return { initial, readFailed, generation, reload, error, changed, flush };
}

/** `QueryRun` 이 바뀔 때마다 다시 그린다. */
function useRunVersion(store: ConsoleStore): void {
  const query = useStore(store, (s) => s.run?.query);
  const [, bump] = useReducer((n: number) => n + 1, 0);
  useEffect(() => query?.subscribe(bump), [query]);
}

export function ConsoleDocument(props: ConsoleDocumentProps) {
  // 창의 콘솔 기능(console-feature.tsx)이 프로젝트를 둔 뒤에 그린다.
  const projectId = useStore(consoleWindow, (s) => s.projectId);
  if (projectId === undefined) return <div data-testid="db-console" className="h-full" />;
  return <ConsoleBody {...props} projectId={projectId} />;
}

function ConsoleBody({
  panelId,
  spec,
  onSpec,
  onTitle,
  projectId,
}: ConsoleDocumentProps & { projectId: string }) {
  const T = useDbConsoleStrings();
  const S = useDbSafetyStrings();
  const { client } = useHost();
  const worktree = useActiveWorktree();
  const workspace = useEditorWorkspace();
  const notify = useNotify();
  const risk = useRiskConfirm();
  const dialogs = useGitDialogs();
  const access = useDbAccessPrompt();
  const root = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView>(undefined);
  const latest = useRef({
    client,
    projectId,
    onSpec,
    risk,
    dialogs,
    T,
    S,
    color: undefined as string | null | undefined,
    resolveAccess: (_error: unknown, _connectionId: string) => Promise.resolve(false),
  });
  latest.current = { ...latest.current, client, projectId, onSpec, risk, dialogs, T, S };

  const [store] = useState(() =>
    createConsoleStore(spec, {
      client: () => latest.current.client,
      projectId,
      worktree: activeWorktree,
      confirmRisk: (question) => latest.current.risk.confirm(question),
      confirmCancelRollback: () =>
        latest.current.dialogs.confirm({ ...latest.current.S.cancelRollback, danger: true }),
      onSpec: (next) => latest.current.onSpec(next),
      color: () => latest.current.color,
      resolveAccess: (error, connectionId) => latest.current.resolveAccess(error, connectionId),
    }),
  );
  const state = useStore(store);
  useRunVersion(store);
  const { opened, run } = state;
  const session = opened?.session;
  const file = useConsoleFile(client, projectId, spec.consoleId);
  // 콘솔 편집기의 구문 세션이 쓰는 Host 연결 — 연결이 바뀔 때만 새로(편집기가 세션을 다시 연다).
  const syntax = useMemo<SyntaxSource | undefined>(
    () =>
      client && {
        open: (params) => client.call("syntax.open", params),
        edit: (params) => client.call("syntax.edit", params),
        highlights: (params) => client.call("syntax.highlights", params),
        close: (params) => client.call("syntax.close", params),
      },
    [client],
  );
  const { flush } = file;

  // 연결 목록(설정 읽기 — 연결을 열지 않는다). 표지 색 · 이름 · 고르기에 쓴다.
  const list = useRpcQuery("db.connection.list", { projectId, worktree: worktree ?? null });
  const connections = useMemo(() => list.data?.connections ?? [], [list.data]);
  const entry = connections.find((c) => c.id === state.spec.connectionId);
  const color = entry?.config?.color ?? null;
  latest.current.color = color;
  // 넘기는 연결은 그 RPC 에 실은 것이다(렌더 때의 고른 연결이 아니다 — 그사이 바뀌었을 수 있다).
  latest.current.resolveAccess = (error, connectionId) => {
    if (!client) return Promise.resolve(false);
    const named = connections.find((c) => c.id === connectionId)?.config?.name;
    return access.resolve(error, {
      calls: client,
      projectId,
      worktree: activeWorktree() ?? null,
      connectionId,
      connectionName: named ?? connectionId,
    });
  };
  /** `connectionId` 로 연결을 여는 RPC 를 부르고, `3101` · `3102` 면 묻고 넣은 뒤 한 번 더 부른다. */
  const withAccess = async <R,>(
    connectionId: string,
    call: () => Promise<R> | undefined,
  ): Promise<R | undefined> => {
    try {
      return await call();
    } catch (e) {
      if (await latest.current.resolveAccess(e, connectionId)) return call();
      throw e;
    }
  };
  const title = opened?.name ?? entry?.config?.name;
  useEffect(() => {
    if (title) onTitle?.(T.tabTitle(title));
  }, [title, onTitle, T]);

  // 탭이 그려지면: Host 가 보존한 세션만 잇는다(새 연결은 열지 않는다 — 결정 M3-D13).
  useEffect(() => {
    if (client && projectId) void store.resume();
  }, [client, projectId, store]);

  // 이 프로젝트의 세션 상태 · 알림.
  useEffect(() => {
    if (!client || !projectId) return;
    const opened = client.subscribe({ projectId, topics: [DB_SESSION_CHANGED_TOPIC] }, (event) =>
      store.applySessionChanged(event.payload as unknown as DbSessionChanged),
    );
    opened.catch(() => {});
    return () => {
      opened.then((s) => s.unsubscribe()).catch(() => {});
    };
  }, [client, projectId, store]);

  // 상태바 `db` 항목(M3-19): 세션이 바뀔 때만 새 객체.
  const active = useMemo<DbActiveSession | undefined>(
    () =>
      opened && {
        session: opened.session,
        name: opened.name,
        production: opened.production,
        readOnly: opened.readOnly,
        color,
      },
    [opened, color],
  );
  useDbTabSession(panelId, active);

  // 연결하면 그 연결을 「마지막 연결」 로 기억한다(⌘⇧L 이 그 연결의 새 콘솔을 연다).
  useEffect(() => {
    if (opened && projectId) rememberConnection(projectId, opened.session.connectionId);
  }, [opened, projectId]);

  // 닫기 전에 묻기(결정 M3-D21).
  useEffect(
    () =>
      workspace.guardClose({
        panelId,
        pending: () => store.needsCloseAnswer(),
        settle: async () => {
          const ask = consoleWindow.getState().askTransaction;
          const now = store.getState().opened;
          if (!ask || !now) return false;
          const answer = await ask({
            connection: {
              name: now.name,
              production: now.production,
              readOnly: now.readOnly,
              color: latest.current.color,
            },
            failed: isFailed(now.session),
          });
          return store.settleClose(answer);
        },
      }),
    [workspace, panelId, store],
  );

  // 실행 범위(선택 · 현재 문장)를 정해 돌린다.
  const scope = useCallback(async (): Promise<
    { sql: string; statements: number[] } | undefined
  > => {
    const view = viewRef.current;
    const now = store.getState().opened;
    const c = latest.current.client;
    const dialect = now?.driver.dialect;
    if (!view || !c || !dialect) return undefined;
    const sql = view.state.doc.toString();
    const { from, to } = view.state.selection.main;
    const statements = await statementsToRun(
      (method, params) => c.call(method, params),
      dialect,
      sql,
      { from, to },
    );
    return { sql, statements };
  }, [store]);

  const runNow = useCallback(
    async (all: boolean) => {
      const view = viewRef.current;
      if (!view || !store.getState().opened) return;
      if (all) return store.run(view.state.doc.toString(), undefined);
      const chosen = await scope().catch((e: unknown) => {
        notify({ level: "error", title: T.title, body: String(e) });
        return undefined;
      });
      if (!chosen) return;
      if (chosen.statements.length === 0) {
        notify({ level: "info", title: T.error.nothingToRun });
        return;
      }
      await store.run(chosen.sql, chosen.statements);
    },
    [store, scope, notify, T],
  );

  const explainNow = useCallback(async () => {
    if (!store.getState().opened) return;
    const chosen = await scope().catch(() => undefined);
    if (!chosen) return;
    if (chosen.statements.length !== 1) {
      notify({ level: "info", title: T.error.explainOne });
      return;
    }
    await store.explain(chosen.sql, chosen.statements);
  }, [store, scope, notify, T]);

  // 창의 액션이 부르는 자리.
  useEffect(
    () =>
      registerConsole({
        panelId,
        root: () => root.current,
        run: (all) => void runNow(all),
        cancel: () => void store.cancel(),
        explain: () => void explainNow(),
        canRun: () => {
          const s = store.getState();
          return s.opened !== undefined && !s.run?.running && !s.busy;
        },
        canCancel: () => store.getState().run?.running === true,
        dispose: () => {
          void flush(viewRef.current?.state.doc.toString(), true);
          void store.dispose();
        },
      }),
    [panelId, runNow, explainNow, store, flush],
  );

  const txOpen = hasOpenTransaction(session);
  const failed = isFailed(session);
  const running = run?.running === true;
  const sqlCapable = opened?.driver.capabilities.sql !== false;
  const capabilities = opened?.driver.capabilities;
  const disconnected = session?.state.kind === "disconnected";
  const choosingLocked = txOpen || running || state.busy;

  return (
    <div
      ref={root}
      // 포커스를 받을 수 있게 — 편집기 밖(위 막대 · 결과의 빈 곳)을 눌러도 콘솔의 키(⌘↩ · ⌘F2)가 산다.
      tabIndex={-1}
      data-testid="db-console"
      data-keymap-context="dbConsoleFocus"
      data-session={session?.sessionId}
      className="flex h-full min-h-0 flex-col bg-editor-background text-ui-foreground"
      style={opened ? connectionFrameStyle({ ...opened, color }) : undefined}
    >
      <div
        data-testid="db-console-bar"
        className="flex flex-wrap items-center gap-1 border-b border-ui-border bg-ui-panel px-2 py-1 text-[12px]"
      >
        <select
          aria-label={T.bar.connection}
          data-testid="db-console-connection"
          className={SELECT}
          value={state.spec.connectionId ?? ""}
          disabled={choosingLocked}
          title={choosingLocked && txOpen ? T.bar.chooseLocked : undefined}
          onChange={(e) =>
            void store.choose({
              connectionId: e.target.value || null,
              database: null,
              schema: null,
            })
          }
        >
          <option value="">{T.bar.noConnection}</option>
          {connections.map((c) => (
            <option key={c.id} value={c.id} disabled={!c.config}>
              {c.config?.name ?? c.id}
            </option>
          ))}
        </select>
        {opened && capabilities?.multipleDatabases && (
          <NamePicker
            label={T.bar.database}
            placeholder={T.bar.databaseDefault}
            value={state.spec.database ?? null}
            disabled={choosingLocked}
            load={() =>
              withAccess(opened.session.connectionId, () =>
                latest.current.client?.call("db.schema.databases", {
                  projectId,
                  worktree: activeWorktree() ?? null,
                  connectionId: opened.session.connectionId,
                }),
              ).then((r) => r?.items.map((i) => i.name) ?? [])
            }
            onPick={(database) =>
              void store.choose({ connectionId: state.spec.connectionId, database, schema: null })
            }
          />
        )}
        {opened && capabilities?.schemas && (
          <NamePicker
            label={T.bar.schema}
            placeholder={T.bar.schemaDefault}
            value={state.spec.schema ?? null}
            disabled={choosingLocked}
            load={() =>
              withAccess(opened.session.connectionId, () =>
                latest.current.client?.call("db.schema.schemas", {
                  projectId,
                  worktree: activeWorktree() ?? null,
                  connectionId: opened.session.connectionId,
                  database: state.spec.database ?? null,
                }),
              ).then((r) => r?.items.map((i) => i.name) ?? [])
            }
            onPick={(schema) =>
              void store.choose({
                connectionId: state.spec.connectionId,
                database: state.spec.database ?? null,
                schema,
              })
            }
          />
        )}
        {opened && !disconnected ? (
          <button
            type="button"
            className={BUTTON}
            data-testid="db-console-disconnect"
            disabled={choosingLocked}
            title={txOpen ? T.bar.chooseLocked : undefined}
            onClick={() => void store.disconnect()}
          >
            {T.bar.disconnect}
          </button>
        ) : (
          <button
            type="button"
            className={BUTTON}
            data-testid="db-console-connect"
            disabled={!state.spec.connectionId || state.busy || !state.resumed}
            onClick={() => {
              if (disconnected) void store.disconnect().then(() => store.connect());
              else void store.connect();
            }}
          >
            {T.bar.connect}
          </button>
        )}
        <span className="mx-1 h-4 border-l border-ui-border" aria-hidden />
        <button
          type="button"
          className={BUTTON}
          data-testid="db-console-run"
          title={T.bar.runHint}
          disabled={!opened || disconnected || running || state.busy || !sqlCapable}
          onClick={() => void runNow(false)}
        >
          <Play size={11} aria-hidden />
          {T.bar.run}
        </button>
        <button
          type="button"
          className={BUTTON}
          data-testid="db-console-cancel"
          title={T.bar.cancelHint}
          disabled={!running}
          onClick={() => void store.cancel()}
        >
          <Square size={11} aria-hidden />
          {T.bar.cancel}
        </button>
        <button
          type="button"
          className={BUTTON}
          data-testid="db-console-explain"
          disabled={!opened || disconnected || running || state.busy || !capabilities?.explain}
          onClick={() => void explainNow()}
        >
          {T.bar.explain}
        </button>
        <span className="mx-1 h-4 border-l border-ui-border" aria-hidden />
        <label className="flex items-center gap-1">
          <span>{T.bar.mode}</span>
          <select
            aria-label={T.bar.mode}
            data-testid="db-console-mode"
            className={SELECT}
            value={session?.mode ?? "auto"}
            disabled={!opened || disconnected || running || !capabilities?.transactions}
            onChange={(e) => void store.setMode(e.target.value as DbSessionMode)}
          >
            <option value="auto">{S.status.mode.auto}</option>
            <option value="manual">{S.status.mode.manual}</option>
          </select>
        </label>
        <button
          type="button"
          className={BUTTON}
          data-testid="db-console-commit"
          disabled={!txOpen || failed || running || state.busy}
          onClick={() => void store.commit()}
        >
          {T.bar.commit}
        </button>
        <button
          type="button"
          className={BUTTON}
          data-testid="db-console-rollback"
          disabled={!txOpen || running || state.busy}
          onClick={() => void store.rollback()}
        >
          {T.bar.rollback}
        </button>
        {opened && (
          <span className="ml-1 flex items-center gap-1">
            <ConnectionMark
              name={opened.name}
              production={opened.production}
              readOnly={opened.readOnly}
              color={color}
            />
            <TlsMark tls={opened.server.tls} />
          </span>
        )}
      </div>
      <ConsoleNotices
        store={store}
        T={T}
        fileError={file.error}
        readFailed={file.readFailed}
        onReload={file.reload}
      />
      <ConsoleSplit
        editor={
          <ConsoleEditor
            key={file.generation}
            panelId={panelId}
            consoleId={spec.consoleId}
            initial={file.initial}
            readOnly={file.readFailed !== undefined}
            onChange={file.changed}
            syntax={syntax}
            onView={(view) => {
              viewRef.current = view;
            }}
          />
        }
        results={<ConsoleResults store={store} T={T} dialect={opened?.driver.dialect ?? null} />}
        label={T.results.resize}
      />
      {risk.element}
      {dialogs.element}
      {access.element}
    </div>
  );
}

/** 위 막대 아래 한 줄: 세션 상태(연결 안 함 · 실패한 트랜잭션 · 끊김 · 설정 바뀜), 세션 알림, 마지막 오류. */
function ConsoleNotices({
  store,
  T,
  fileError,
  readFailed,
  onReload,
}: {
  store: ConsoleStore;
  T: DbConsoleStrings;
  fileError: string | undefined;
  /** 콘솔 파일을 읽지 못한 까닭(편집기는 읽기 전용이다). */
  readFailed: string | undefined;
  onReload(): void;
}) {
  const S = useDbSafetyStrings();
  const { opened, error, notice, resumed } = useStore(store);
  const kind = opened?.session.state.kind;
  const lines: { key: string; text: string; warning?: boolean }[] = [];
  if (readFailed !== undefined)
    lines.push({ key: "read", text: T.error.load(readFailed), warning: true });
  if (resumed && !opened) lines.push({ key: "idle", text: T.bar.notConnected });
  if (kind === "failed") lines.push({ key: "failed", text: T.bar.failed, warning: true });
  if (kind === "disconnected") lines.push({ key: "lost", text: T.bar.disconnected, warning: true });
  if (kind === "configChanged")
    lines.push({ key: "config", text: T.bar.configChanged, warning: true });
  if (notice) lines.push({ key: "notice", text: dbNoticeText(notice, S.notice), warning: true });
  if (fileError) lines.push({ key: "file", text: T.error.save(fileError), warning: true });
  const refused = error !== undefined && dbRefusal(error) !== undefined;
  if (error !== undefined && !refused)
    lines.push({
      key: "error",
      text: T.error.other(error instanceof Error ? error.message : String(error)),
      warning: true,
    });
  if (lines.length === 0 && !refused) return null;
  return (
    <div
      data-testid="db-console-notices"
      className="flex flex-col gap-0.5 border-b border-ui-border bg-ui-panel px-2 py-1 text-[12px]"
    >
      {lines.map((line) => (
        <p
          key={line.key}
          data-notice={line.key}
          role={line.warning ? "alert" : undefined}
          className={line.warning ? "text-db-production" : "text-ui-muted-foreground"}
        >
          {line.text}
          {line.key === "read" && (
            <button
              type="button"
              data-testid="db-console-reload"
              className={`${BUTTON} ml-2 inline-flex`}
              onClick={onReload}
            >
              {T.error.reload}
            </button>
          )}
        </p>
      ))}
      {refused && <DbRefusalMessage error={error} />}
    </div>
  );
}

/** 이름 고르기(DB · 스키마): 처음 펼칠 때 목록을 읽는다(세션을 연 뒤에만 그려진다). */
function NamePicker({
  label,
  placeholder,
  value,
  disabled,
  load,
  onPick,
}: {
  label: string;
  placeholder: string;
  value: string | null;
  disabled: boolean;
  load(): Promise<string[]> | undefined;
  onPick(name: string | null): void;
}) {
  const [names, setNames] = useState<string[]>();
  const loadRef = useRef(load);
  loadRef.current = load;
  const ensure = () => {
    if (names !== undefined) return;
    setNames([]);
    void loadRef.current()?.then(setNames, () => setNames([]));
  };
  const shown = names ?? [];
  const options = value !== null && !shown.includes(value) ? [value, ...shown] : shown;
  return (
    <select
      aria-label={label}
      className={SELECT}
      value={value ?? ""}
      disabled={disabled}
      onFocus={ensure}
      onPointerDown={ensure}
      onChange={(e) => onPick(e.target.value || null)}
    >
      <option value="">{placeholder}</option>
      {options.map((name) => (
        <option key={name} value={name}>
          {name}
        </option>
      ))}
    </select>
  );
}

/** 콘솔(위)과 결과(아래). 경계를 끌어 높이를 바꾼다(콘솔 몫 15~85%). */
function ConsoleSplit({
  editor,
  results,
  label,
}: {
  editor: ReactNode;
  results: ReactNode;
  label: string;
}) {
  const [share, setShare] = useState(0.5);
  const box = useRef<HTMLDivElement>(null);
  const drag = (e: ReactPointerEvent<HTMLDivElement>) => {
    const el = box.current;
    if (!el) return;
    e.currentTarget.setPointerCapture?.(e.pointerId);
    const move = (ev: PointerEvent) => {
      const rect = el.getBoundingClientRect();
      if (rect.height <= 0) return;
      setShare(Math.min(0.85, Math.max(0.15, (ev.clientY - rect.top) / rect.height)));
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };
  return (
    <div ref={box} className="flex min-h-0 flex-1 flex-col">
      <div className="min-h-0" style={{ flex: `${share} 1 0` }}>
        {editor}
      </div>
      <div
        role="separator"
        aria-orientation="horizontal"
        aria-label={label}
        className="h-[4px] shrink-0 cursor-row-resize border-y border-ui-border bg-ui-panel"
        onPointerDown={drag}
      />
      <div className="min-h-0" style={{ flex: `${1 - share} 1 0` }}>
        {results}
      </div>
    </div>
  );
}

/** 결과: 문장마다 탭 하나 + 실행 계획. */
function ConsoleResults({
  store,
  T,
  dialect,
}: {
  store: ConsoleStore;
  T: DbConsoleStrings;
  dialect: "postgres" | "mysql" | "sqlite" | null;
}) {
  const { run, shown, plan } = useStore(store);
  const { client } = useHost();
  const notify = useNotify();
  const results: StatementResult[] = run?.query.results ?? [];
  const current =
    shown.kind === "result" ? (results[shown.index] ?? results[results.length - 1]) : undefined;
  const tabs = (
    <div role="tablist" aria-label={T.results.label} className="flex flex-wrap gap-0.5">
      {results.map((r, i) => (
        <button
          key={r.index}
          type="button"
          role="tab"
          data-testid="db-console-result-tab"
          data-statement={r.index}
          aria-selected={current === r}
          className={`rounded-sm px-2 text-[12px] ${current === r ? "bg-ui-active" : "hover:bg-ui-hover"}`}
          onClick={() => store.show({ kind: "result", index: i })}
        >
          {T.results.tab(r.index + 1)}
        </button>
      ))}
      {plan && (
        <button
          type="button"
          role="tab"
          data-testid="db-console-plan-tab"
          aria-selected={shown.kind === "plan"}
          className={`rounded-sm px-2 text-[12px] ${shown.kind === "plan" ? "bg-ui-active" : "hover:bg-ui-hover"}`}
          onClick={() => store.show({ kind: "plan" })}
        >
          {T.results.plan}
        </button>
      )}
    </div>
  );
  if (shown.kind === "plan" && plan)
    return (
      <div className="flex h-full min-h-0 flex-col">
        <div className="border-b border-ui-border bg-ui-panel px-2 py-0.5">{tabs}</div>
        <PlanView plan={plan} T={T} />
      </div>
    );
  if (!current)
    return (
      <div className="flex h-full min-h-0 flex-col">
        {(results.length > 0 || plan) && (
          <div className="border-b border-ui-border bg-ui-panel px-2 py-0.5">{tabs}</div>
        )}
        <p
          data-testid="db-console-results-empty"
          className="p-3 text-[12px] text-ui-muted-foreground"
        >
          {T.results.empty}
        </p>
      </div>
    );
  return (
    <div className="flex h-full min-h-0 flex-col" data-testid="db-console-results">
      <ResultView
        buffer={current.buffer}
        notify={notify}
        toolbar={tabs}
        {...(client && dialect && { insert: { client, dialect, table: null } })}
      />
    </div>
  );
}

function PlanView({ plan, T }: { plan: ExplainPlan; T: DbConsoleStrings }) {
  const nodes = useMemo(() => planTree(plan), [plan]);
  if (nodes.length === 0)
    return <p className="p-3 text-[12px] text-ui-muted-foreground">{T.results.planEmpty}</p>;
  return (
    <div
      data-testid="db-console-plan"
      role="tree"
      className="min-h-0 flex-1 overflow-auto p-2 font-mono text-[12px]"
    >
      {nodes.map((node, i) => (
        <PlanRow key={i} node={node} depth={0} T={T} />
      ))}
    </div>
  );
}

function PlanRow({ node, depth, T }: { node: PlanNode; depth: number; T: DbConsoleStrings }) {
  return (
    <>
      <div
        role="treeitem"
        aria-level={depth + 1}
        className="flex gap-3 whitespace-pre"
        style={{ paddingLeft: depth * 16 }}
      >
        <span data-plan-label>{node.label}</span>
        {node.cost !== undefined && (
          <span className="text-ui-muted-foreground">
            {T.results.planCost} {node.cost}
          </span>
        )}
        {node.rows !== undefined && (
          <span className="text-ui-muted-foreground">
            {T.results.planRows} {node.rows}
          </span>
        )}
        {node.detail !== undefined && (
          <span className="text-ui-muted-foreground">{node.detail}</span>
        )}
      </div>
      {node.children.map((child, i) => (
        <PlanRow key={i} node={child} depth={depth + 1} T={T} />
      ))}
    </>
  );
}
