// 상태바 항목 `db`(14-m3-plan.md M3-19 ①, 결정 M3-D21): 지금 탭의 연결(색 · 이름 · 프로덕션 · 읽기 전용 라벨), 커밋 방식,
// 트랜잭션 상태 — 열린 시간(초마다), `failed`(롤백만 된다), 끊김, 설정 바뀜. `db.sessionChanged`(프로젝트 범위 이벤트)를 따른다.
//
// 이음매 — 「지금 탭」 은 콘솔 · 테이블 데이터 · Redis 탭(M3-15 · M3-17 · M3-18)이 정한다: 탭이 앞에 오면 `setActiveDbSession`
// 으로 그 세션(`db.session.open` · `resume` 의 답)을 두고, 탭이 뒤로 가거나 닫히면 `undefined` 로 비운다. 창마다 웹뷰가 따로라
// 모듈 상태로 둔다(runtime-status.tsx 와 같다). 항목은 이 창의 프로젝트에 `db.sessionChanged` 를 구독해 같은 세션의 상태를 바꾼다
// — 탭 쪽도 받은 이벤트를 `applyDbSessionChanged` 로 넣어도 된다(같은 일을 두 번 해도 결과가 같다).

import type { DbSessionChanged, DbSessionInfo, DbSessionState } from "@pitwall/rpc-client";
import { Database } from "lucide-react";
import { useEffect, useState } from "react";
import { createStore, useStore } from "zustand";

import { useHost } from "../../rpc/host";
import { STATUS_SLOTS, useStatusBarItem } from "../../status-bar/status-bar";
import { ConnectionMark, connectionColor } from "./connection-mark";
import { type DbSafetyStrings, useDbSafetyStrings } from "./strings";

/** `db.sessionChanged` 토픽(crates/pw-db/src/session.rs `TOPIC_SESSION_CHANGED`). */
export const DB_SESSION_CHANGED_TOPIC = "db.sessionChanged";

/** 지금 탭의 세션. 연결 쪽 필드는 `DbSessionOpened` 의 같은 이름 · 연결 설정의 `color`. */
export interface DbActiveSession {
  session: DbSessionInfo;
  name: string;
  production: boolean;
  readOnly: boolean;
  color?: string | null;
}

interface DbStatusState {
  /** 이 창의 프로젝트(`DbStatus` 가 둔다). */
  projectId: string | undefined;
  active: DbActiveSession | undefined;
}

/** 이 창의 상태바 `db` 항목이 보는 것. 시험 · 다른 행은 아래 함수로만 바꾼다. */
export const dbStatusStore = createStore<DbStatusState>()(() => ({
  projectId: undefined,
  active: undefined,
}));

/** 지금 탭의 세션을 둔다(탭이 앞에 올 때) · 비운다(`undefined` — 탭이 뒤로 가거나 닫힐 때). */
export function setActiveDbSession(active: DbActiveSession | undefined) {
  dbStatusStore.setState({ active });
}

/** `db.sessionChanged` 를 넣는다. 지금 탭의 세션이 아니면 아무것도 바꾸지 않는다. */
export function applyDbSessionChanged(event: DbSessionChanged) {
  const { active } = dbStatusStore.getState();
  if (!active || active.session.sessionId !== event.session.sessionId) return;
  dbStatusStore.setState({ active: { ...active, session: event.session } });
}

/** 지난 시간(밀리초)을 `m:ss` · `h:mm:ss` 로. 음수(시계 차이)는 0 으로 본다. */
export function formatElapsed(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const s = total % 60;
  const m = Math.floor(total / 60) % 60;
  const h = Math.floor(total / 3600);
  const ss = String(s).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}

/** 트랜잭션이 열린 시각. 없으면 `undefined`. */
export function transactionSince(state: DbSessionState): number | undefined {
  switch (state.kind) {
    case "inTransaction":
    case "failed":
      return state.since;
    case "configChanged":
      return state.transaction?.since;
    case "idle":
    case "disconnected":
      return undefined;
  }
}

/** 세션 상태의 글(상태바). 그릴 것이 없으면(`idle`) `undefined`. */
export function describeSessionState(
  state: DbSessionState,
  now: number,
  T: DbSafetyStrings["status"],
): { kind: DbSessionState["kind"]; text: string; warning: boolean } | undefined {
  const elapsed = (since: number) => formatElapsed(now - since);
  switch (state.kind) {
    case "idle":
      return undefined;
    case "inTransaction":
      return { kind: state.kind, text: T.transaction(elapsed(state.since)), warning: false };
    case "failed":
      return { kind: state.kind, text: T.failed(elapsed(state.since)), warning: true };
    case "disconnected":
      return { kind: state.kind, text: T.disconnected, warning: true };
    case "configChanged": {
      const tx = state.transaction;
      const text = tx
        ? `${T.configChanged} · ${(tx.failed ? T.failed : T.transaction)(elapsed(tx.since))}`
        : T.configChanged;
      return { kind: state.kind, text, warning: true };
    }
  }
}

/** 열린 트랜잭션이 있는 동안 1초마다 다시 그린다. */
function useNow(ticking: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!ticking) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [ticking]);
  return now;
}

/** 상태바 항목: 지금 탭의 DB 세션. 지금 탭이 DB 탭이 아니면 그리지 않는다. */
export function DbStatusItem() {
  const T = useDbSafetyStrings();
  const active = useStore(dbStatusStore, (s) => s.active);
  const since = active ? transactionSince(active.session.state) : undefined;
  const now = useNow(since !== undefined);
  if (!active) return null;
  const { session } = active;
  const mode = T.status.mode[session.mode];
  const state = describeSessionState(session.state, now, T.status);
  const color = connectionColor(active);
  return (
    <span
      data-testid="status-db"
      data-state={session.state.kind}
      data-mode={session.mode}
      title={T.status.hint(active.name, mode)}
      className="flex items-center gap-1 rounded-sm px-1"
      // 연결 색을 항목 아래 띠로(10-ui.md 「탭/콘솔 테두리 · 상태바까지 색」).
      style={color ? { boxShadow: `inset 0 -2px 0 ${color}` } : undefined}
    >
      <Database size={11} aria-hidden />
      <ConnectionMark
        name={active.name}
        production={active.production}
        readOnly={active.readOnly}
        color={active.color}
      />
      <span className="text-ui-muted-foreground">· {mode}</span>
      {state && (
        <span
          data-testid="status-db-transaction"
          data-warning={state.warning || undefined}
          className={state.warning ? "text-db-production" : undefined}
        >
          · {state.text}
        </span>
      )}
    </span>
  );
}

/** 이 창의 프로젝트로 `db` 항목을 두고 그 프로젝트의 `db.sessionChanged` 를 따른다. `StatusBarProvider` 안에 둔다. */
export function DbStatus({ projectId }: { projectId: string }) {
  const { client } = useHost();
  useEffect(() => {
    dbStatusStore.setState({ projectId });
    return () => dbStatusStore.setState({ projectId: undefined, active: undefined });
  }, [projectId]);
  useEffect(() => {
    if (!client) return;
    const opened = client.subscribe({ projectId, topics: [DB_SESSION_CHANGED_TOPIC] }, (event) =>
      applyDbSessionChanged(event.payload as unknown as DbSessionChanged),
    );
    opened.catch((e: unknown) => console.warn("[db] cannot subscribe to db.sessionChanged", e));
    return () => {
      opened.then((s) => s.unsubscribe()).catch(() => {});
    };
  }, [client, projectId]);
  useStatusBarItem({ id: "db", ...STATUS_SLOTS.db, component: DbStatusItem });
  return null;
}
