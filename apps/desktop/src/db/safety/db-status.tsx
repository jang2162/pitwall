// 상태바 항목 `db`(14-m3-plan.md M3-19 ①, 결정 M3-D21): 지금 탭의 연결(색 · 이름 · 프로덕션 · 읽기 전용 라벨), 커밋 방식,
// 트랜잭션 상태 — 열린 시간(초마다), `failed`(롤백만 된다), 끊김, 설정 바뀜. `db.sessionChanged`(프로젝트 범위 이벤트)를 따른다.
//
// 이음매 — 세션은 탭이 제 것을 제 id(dockview 패널 id)로 둔다: 콘솔 · 테이블 데이터 · Redis 탭(M3-15 · M3-17 · M3-18)은
// `useDbTabSession(panelId, active)` 로 세션(`db.session.open` · `resume` 의 답)을 두고, 세션을 닫으면 `undefined` 를 넘긴다 —
// 탭이 닫히면(언마운트) 저절로 빠진다. 「지금 탭」 은 창이 정한다: `DbStatus` 가 에디터 영역의 지금 탭
// (`workspace.activeTab()` — 툴 윈도우가 앞이면 마지막에 쓴 에디터 탭)을 따라 그 id 의 세션만 보인다. 그래서 탭끼리 쓰는 순서와
// 상관없이 앞에 온 탭의 세션이 보이고, 뒤로 간 · 닫힌 탭의 세션은 보이지 않는다. 창마다 웹뷰가 따로라 모듈 상태로 둔다
// (runtime-status.tsx 와 같다). 항목은 이 창의 프로젝트에 `db.sessionChanged` 를 구독해 같은 세션의 상태를 바꾼다 — 탭 쪽도
// 받은 이벤트를 `applyDbSessionChanged` 로 넣어도 된다(같은 일을 두 번 해도 결과가 같다).

import type { DbSessionChanged, DbSessionInfo, DbSessionState } from "@pitwall/rpc-client";
import { Database } from "lucide-react";
import type { DockviewApi } from "dockview-react";
import { useEffect, useState } from "react";
import { createStore, useStore } from "zustand";

import type { EditorWorkspace } from "../../editor/editor-workspace";
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
  /** 탭(dockview 패널 id)마다 그 탭이 둔 세션. 탭만 제 id 를 바꾼다. */
  tabs: Readonly<Record<string, DbActiveSession>>;
  /** 에디터 영역의 지금 탭(패널 id). `DbStatus` 가 창의 도킹 영역을 따라 둔다. */
  frontTabId: string | undefined;
}

/** 이 창의 상태바 `db` 항목이 보는 것. 시험 · 다른 행은 아래 함수로만 바꾼다. */
export const dbStatusStore = createStore<DbStatusState>()(() => ({
  projectId: undefined,
  tabs: {},
  frontTabId: undefined,
}));

/** 탭 `tabId` 의 세션을 둔다 · 뺀다(`undefined` — 세션을 닫았거나 탭이 닫힐 때). 다른 탭의 것은 건드리지 않는다. */
export function setDbTabSession(tabId: string, active: DbActiveSession | undefined) {
  dbStatusStore.setState((s) => {
    if (active) return { tabs: { ...s.tabs, [tabId]: active } };
    if (!Object.hasOwn(s.tabs, tabId)) return s;
    const tabs = { ...s.tabs };
    delete tabs[tabId];
    return { tabs };
  });
}

/**
 * 탭 컴포넌트가 부른다: 마운트돼 있는 동안 그 탭의 세션을 둔다. `active` 가 바뀌면 바꾸고, 언마운트(탭 닫힘) · `tabId` 가 바뀌면
 * 뺀다. `active` 는 세션이 바뀔 때만 새 객체로 넘긴다 — 렌더마다 새 객체면 `db.sessionChanged` 로 바뀐 상태를 탭의 옛 값으로 덮는다.
 */
export function useDbTabSession(tabId: string, active: DbActiveSession | undefined) {
  useEffect(() => setDbTabSession(tabId, active), [tabId, active]);
  useEffect(() => () => setDbTabSession(tabId, undefined), [tabId]);
}

/** 에디터 영역의 지금 탭을 둔다(`DbStatus` 가 부른다). */
export function setDbFrontTab(tabId: string | undefined) {
  dbStatusStore.setState({ frontTabId: tabId });
}

/** 상태바가 보이는 세션: 지금 탭이 둔 세션. 지금 탭이 DB 탭이 아니면 `undefined`. */
export function frontDbSession(state: DbStatusState): DbActiveSession | undefined {
  return state.frontTabId === undefined ? undefined : state.tabs[state.frontTabId];
}

/** `db.sessionChanged` 를 넣는다. 그 세션을 둔 탭의 것만 바꾼다. */
export function applyDbSessionChanged(event: DbSessionChanged) {
  dbStatusStore.setState((s) => {
    let tabs: Record<string, DbActiveSession> | undefined;
    for (const [tabId, active] of Object.entries(s.tabs)) {
      if (active.session.sessionId !== event.session.sessionId) continue;
      tabs ??= { ...s.tabs };
      tabs[tabId] = { ...active, session: event.session };
    }
    return tabs ? { tabs } : s;
  });
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
  const active = useStore(dbStatusStore, frontDbSession);
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

/**
 * 이 창의 프로젝트로 `db` 항목을 두고 그 프로젝트의 `db.sessionChanged` 를 따른다. 에디터 영역의 지금 탭(`workspace.activeTab()`)을
 * 따라 「지금 탭」 을 둔다 — 도킹 영역(`api`)이 붙기 전에는 지금 탭이 없다. `StatusBarProvider` 안에 둔다.
 */
export function DbStatus({
  projectId,
  workspace,
  api,
}: {
  projectId: string;
  workspace: Pick<EditorWorkspace, "activeTab">;
  api: Pick<DockviewApi, "onDidActivePanelChange" | "onDidRemovePanel"> | undefined;
}) {
  const { client } = useHost();
  useEffect(() => {
    dbStatusStore.setState({ projectId });
    return () => dbStatusStore.setState({ projectId: undefined });
  }, [projectId]);
  useEffect(() => {
    let live = true;
    const refresh = () => setDbFrontTab(workspace.activeTab()?.id);
    // 활성 · 제거 이벤트가 끝난 뒤에 읽는다 — 그 안에서는 최근 탭 순서가 아직 바뀌기 전일 수 있다(editor-status.tsx 와 같다).
    const later = () =>
      queueMicrotask(() => {
        if (live) refresh();
      });
    refresh();
    const activated = api?.onDidActivePanelChange(later);
    const removed = api?.onDidRemovePanel(later);
    return () => {
      live = false;
      activated?.dispose();
      removed?.dispose();
      setDbFrontTab(undefined);
    };
  }, [workspace, api]);
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
