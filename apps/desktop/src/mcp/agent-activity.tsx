// 「에이전트 활동」 툴 윈도우(16-m5-plan M5-13, 10-ui.md 「2.5 주요 툴 윈도우 상세」 의 「에이전트 활동」). 자리는 오른쪽 툴 윈도우 바의
// `agent` 다(layout/project-layout.ts 의 PANELS). 상태바 「MCP n」 을 누르면 열린다(mcp-status.tsx).
//
// 위에서부터:
// - 「승인 대기」 칸 — 자리만(pending-approvals.tsx). 승인 다이얼로그 행(M5-14)이 채운다.
// - 「연결된 에이전트」 — 이 프로젝트를 세션 프로젝트로 가진 세션(`mcp.sessions`): 클라이언트 · 시작 시각 · 호출 수. 호출 수는
//   이벤트 없이 바뀌므로 `mcp.activity` 를 받으면 세션 목록도 다시 읽는다.
// - 호출 로그 — `mcp.audit.list { projectId }`: 이 프로젝트의 행과 프로젝트 없는 행(carry-notes R6 — 머리에 그렇다고 적고, 프로젝트
//   없는 행에는 표지를 단다). 시각 · 클라이언트 · 툴 · 인자 요약 · 결과 · 소요 · 승인 방식. 결과는 색과 함께 글자로 보이고,
//   `started` 는 「끝을 모름」 이다. 보이는 줄만 그린다(TanStack Virtual) — 끝에 닿으면 다음 쪽을 읽는다.
//   - 거르기: 결과 · 툴은 Host 가(`outcome` · `tool`), 클라이언트는 받은 행에서 거른다(RPC 에 칸이 없다). 클라이언트로 거르면 맞는
//     줄이 화면을 채울 때까지 쪽을 더 읽는다.
//   - 새 행: `mcp.activity`(이 프로젝트 범위와 Host 범위 — 프로젝트 없는 호출) → 맨 위부터 그 행까지 다시 읽는다(audit-log.ts).
//   - 다른 프로세스(헤드리스 `pitwall mcp` · `pitwall tool`)가 쓴 행은 이벤트가 없다 — 창이 앞으로 올 때(`focus` · 보이게 됨)
//     다시 읽는다(carry-notes R8).
//   - 「기록 지우기」 — 확인(처음 포커스 [취소]) 뒤 `mcp.audit.clear { projectId }`, 그리고 처음부터 다시 읽는다. 프로젝트 없는
//     행은 남는다.
//
// 에이전트가 보낸 글(클라이언트 이름 · 툴 이름 · 인자 · 상세)은 React 텍스트로만 그리고, 제어 문자 · 양방향 제어 문자는 `�` 로
// 바꿔 보인다(03-architecture.md 「보안」 의 git 인증 줄과 같은 규칙).

import type {
  AuditOutcome,
  AuditRow,
  McpSessionInfo,
  McpToolInfo,
  RpcClient,
} from "@pitwall/rpc-client";
import * as Dialog from "@radix-ui/react-dialog";
import { useQueryClient } from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import type { DockviewPanelApi, IDockviewPanelProps } from "dockview-react";
import { Trash2 } from "lucide-react";
import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useStore } from "zustand";

import { useI18n } from "../i18n/i18n-provider";
import { useKeymapPopup } from "../keymap/keymap-provider";
import { useNotify } from "../notifications/notifications";
import { useHost, useRpcQuery } from "../rpc/host";
import { rpcQueryKey } from "../rpc/queries";
import { type AuditLog, createAuditLog } from "./audit-log";
import { useMcpProjectId } from "./mcp-status";
import { PendingApprovals } from "./pending-approvals";
import { sessionsOf, useMcpSessions } from "./sessions";
import { type McpStrings, useMcpStrings } from "./strings";

export const ACTIVITY_ROW_HEIGHT = 22;
const ACTIVITY_TOPIC = "mcp.activity";
const OUTCOMES: readonly AuditOutcome[] = ["ok", "error", "denied", "started"];
/** 끝에서 이만큼 줄 안쪽이 그려지면 다음 쪽을 읽는다. */
const LOAD_AHEAD = 20;
/** 클라이언트 거르기의 「클라이언트 모름」(`client: null`) 값. 클라이언트 이름으로 나올 수 없는 글이다. */
const UNKNOWN_CLIENT = "\u0000";
/** 거르기 · 열 머리 · 줄이 함께 쓰는 열 너비. */
const COLUMNS = "grid grid-cols-[64px_96px_120px_minmax(80px,1fr)_76px_56px_88px] gap-x-2";
const BUTTON =
  "rounded-sm border border-ui-border bg-ui-panel px-2 py-0.5 text-[12px] hover:bg-ui-hover disabled:opacity-50";
const SELECT = "min-w-0 rounded-sm border border-ui-border bg-ui-panel px-1 text-[12px]";

type ActivityStrings = McpStrings["activity"];

/** 숨은 글자: 제어 문자(C0 · C1)와 양방향 제어 문자(ALM · LRM · RLM · LRE~RLO · LRI~PDI). */
function hidden(code: number) {
  return (
    code <= 0x1f ||
    (code >= 0x7f && code <= 0x9f) ||
    code === 0x061c ||
    code === 0x200e ||
    code === 0x200f ||
    (code >= 0x202a && code <= 0x202e) ||
    (code >= 0x2066 && code <= 0x2069)
  );
}

/** 에이전트가 보낸 글의 숨은 글자를 U+FFFD 로 바꾼다. */
export function displayText(text: string): string {
  let out = "";
  for (const c of text) out += hidden(c.codePointAt(0)!) ? "\ufffd" : c;
  return out;
}

// 시각은 UI 언어(`useI18n().locales[0]`)로 쓴다 — 시스템 로캘이 아니다(git 문서 패널 · 거터와 같은 규칙). `undefined` 를 넘기면
// 영어 UI 에 「오전 07:20」 이 나온다(M5-13b).
function clockTime(at: number, locale: string | undefined) {
  return new Date(at).toLocaleTimeString(locale, {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}

function shortTime(at: number, locale: string | undefined) {
  return new Date(at).toLocaleTimeString(locale, { hour: "2-digit", minute: "2-digit" });
}

const OUTCOME_CLASS: Record<AuditOutcome, string> = {
  ok: "text-git-added",
  error: "text-[var(--pw-diagnostic-error)]",
  denied: "text-[var(--pw-diagnostic-warning)]",
  started: "text-ui-muted-foreground italic",
};

// ---- 이벤트 · 창 앞으로 ---------------------------------------------------------------------------------------------------------

/** 창이 앞으로 올 때 함께 오는 `focus` · `visibilitychange` 를 한 번의 다시 읽기로 묶는 간격. */
export const WAKE_COALESCE_MS = 100;

/** 툴 윈도우가 지금 보이는가: 그 그룹의 활성 탭이고, 그 그룹이 접힌 가장자리 dock 이 아니다. */
function isShown(api: DockviewPanelApi): boolean {
  const group = api.group?.api;
  return api.isVisible && !(group?.location.type === "edge" && group.isCollapsed());
}

function usePanelShown(api: DockviewPanelApi): boolean {
  const [shown, setShown] = useState(() => isShown(api));
  useEffect(() => {
    const update = () => setShown(isShown(api));
    let groupOff: { dispose(): void } | undefined;
    const watchGroup = () => {
      groupOff?.dispose();
      groupOff = api.group?.api.onDidCollapsedChange(update);
      update();
    };
    watchGroup();
    const visibility = api.onDidVisibilityChange(update);
    const moved = api.onDidGroupChange(watchGroup);
    return () => {
      groupOff?.dispose();
      visibility.dispose();
      moved.dispose();
    };
  }, [api]);
  return shown;
}

/**
 * 로그를 살아 있게 둔다: `mcp.activity` 를 이 프로젝트 범위와 Host 범위(프로젝트 없는 호출)로 구독해 행 id 를 넘기고, 세션의 호출
 * 수도 다시 읽게 한다. 구독이 열리면 한 번 다시 읽는다(첫 쪽과 구독 사이에 쓰인 행). 창이 앞으로 오면 다시 읽는다.
 *
 * 패널이 보이지 않는 동안(다른 탭 뒤 · 접힌 dock)은 읽지 않고 「밀렸다」 고만 적어 두었다가, 다시 보일 때 한 번 읽는다(M5-13 리뷰
 * F3). 창이 앞으로 올 때 `focus` 와 `visibilitychange` 가 함께 오면 [`WAKE_COALESCE_MS`] 안의 것을 한 번으로 묶는다.
 */
function useLiveLog(
  log: AuditLog,
  client: RpcClient,
  host: string,
  projectId: string,
  shown: boolean,
  visibleIds: () => readonly number[],
) {
  const queryClient = useQueryClient();
  const shownRef = useRef(shown);
  shownRef.current = shown;
  const visibleRef = useRef(visibleIds);
  visibleRef.current = visibleIds;
  /** 보이지 않는 동안 놓친 것이 있다. */
  const missed = useRef(false);

  useEffect(() => {
    let disposed = false;
    let ids: number[] = [];
    let scheduled = false;
    const flush = () => {
      scheduled = false;
      const batch = ids;
      ids = [];
      if (disposed) return;
      void queryClient.invalidateQueries({ queryKey: rpcQueryKey(host, "mcp.sessions") });
      if (shownRef.current) log.activity(batch);
      else missed.current = true;
    };
    const onEvent = (event: { topic: string; payload: unknown }) => {
      if (disposed || event.topic !== ACTIVITY_TOPIC) return;
      const id = (event.payload as { id?: unknown } | null)?.id;
      if (typeof id !== "number") return;
      ids.push(id);
      // 이벤트가 몰려도(선기록 · 끝 · 여러 세션) 한 번에 넘긴다.
      if (!scheduled) {
        scheduled = true;
        queueMicrotask(flush);
      }
    };
    const scopes = [{ projectId, topics: [ACTIVITY_TOPIC] }, { topics: [ACTIVITY_TOPIC] }];
    const subscriptions = scopes.map((scope) =>
      client.subscribe(scope, onEvent).then(
        (s) => {
          if (disposed) void s.unsubscribe().catch(() => {});
          return s;
        },
        (e: unknown) => {
          console.warn("[mcp] cannot subscribe to mcp.activity", e);
          return undefined;
        },
      ),
    );
    void Promise.all(subscriptions).then(() => {
      if (disposed) return;
      if (shownRef.current) log.refreshAll(visibleRef.current());
      else missed.current = true;
    });
    return () => {
      disposed = true;
      for (const s of subscriptions) void s.then((opened) => opened?.unsubscribe()).catch(() => {});
    };
  }, [log, client, host, projectId, queryClient]);

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const wake = () => {
      // 보이는지는 묶은 뒤에 본다(아래) — 보이지 않으면 밀린 것으로만 적는다.
      if (timer !== undefined) return;
      timer = setTimeout(() => {
        timer = undefined;
        if (shownRef.current) log.refreshAll(visibleRef.current());
        else missed.current = true;
      }, WAKE_COALESCE_MS);
    };
    const onVisible = () => {
      if (document.visibilityState === "visible") wake();
    };
    window.addEventListener("focus", wake);
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      clearTimeout(timer);
      window.removeEventListener("focus", wake);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [log]);

  // 다시 보이면 숨어 있는 동안 밀린 것을 한 번 읽는다.
  useEffect(() => {
    if (!shown || !missed.current) return;
    missed.current = false;
    log.refreshAll(visibleRef.current());
  }, [shown, log]);
}

// ---- 툴 윈도우 ------------------------------------------------------------------------------------------------------------------

export function AgentActivityToolWindow({ api }: IDockviewPanelProps) {
  const projectId = useMcpProjectId();
  return (
    <div
      data-tool-window={api.id}
      tabIndex={-1}
      className="flex h-full flex-col bg-ui-panel text-ui-foreground outline-none"
    >
      {/* 프로젝트가 없으면(도킹 영역만의 시험 · 프로젝트 없는 창) Host 도 찾지 않는다. */}
      {projectId !== undefined && <AgentActivity api={api} projectId={projectId} />}
    </div>
  );
}

function AgentActivity({ api, projectId }: { api: DockviewPanelApi; projectId: string }) {
  const { client, key } = useHost();
  return (
    <>
      <PendingApprovals projectId={projectId} />
      <ConnectedAgents projectId={projectId} />
      {client && (
        <CallLog
          key={`${key}\n${projectId}`}
          api={api}
          client={client}
          host={key}
          projectId={projectId}
        />
      )}
    </>
  );
}

function ConnectedAgents({ projectId }: { projectId: string }) {
  const T = useMcpStrings().sessions;
  const mine = sessionsOf(useMcpSessions(), projectId);
  const title = useId();
  return (
    <section
      data-testid="mcp-agents"
      aria-labelledby={title}
      className="flex-none border-b border-ui-border px-2 py-1"
    >
      <h3 id={title} className="text-[11px] font-semibold text-ui-muted-foreground uppercase">
        {T.title}
      </h3>
      {mine.length === 0 ? (
        <p className="text-[12px] text-ui-muted-foreground">{T.empty}</p>
      ) : (
        <ul>
          {mine.map((s) => (
            <SessionRow key={s.id} session={s} T={T} />
          ))}
        </ul>
      )}
    </section>
  );
}

function SessionRow({ session, T }: { session: McpSessionInfo; T: McpStrings["sessions"] }) {
  const locale = useI18n().locales[0];
  const name = session.client === null ? T.unknownClient : displayText(session.client);
  return (
    <li data-testid="mcp-session" className="flex items-baseline gap-2 py-0.5 text-[12px]">
      <span className="min-w-0 flex-1 truncate" title={name}>
        {name}
      </span>
      <span className="text-ui-muted-foreground">
        {T.started(shortTime(session.startedAt, locale))}
      </span>
      <span className="text-ui-muted-foreground">{T.calls(session.calls)}</span>
    </li>
  );
}

function clientKey(client: string | null) {
  return client ?? UNKNOWN_CLIENT;
}

function CallLog({
  api,
  client,
  host,
  projectId,
}: {
  api: DockviewPanelApi;
  client: RpcClient;
  host: string;
  projectId: string;
}) {
  const strings = useMcpStrings();
  const T = strings.activity;
  const locale = useI18n().locales[0];
  const notify = useNotify();
  const [log] = useState(() =>
    createAuditLog({ fetch: (params) => client.call("mcp.audit.list", params), projectId }),
  );
  // 시작과 정리를 짝으로 건다 — 정리만 걸면 StrictMode(dev)가 effect 를 한 번 정리했다 다시 걸 때 로그가 정리된 채 남아 첫 쪽을
  // 버리고 「불러오는 중」 에 멈춘다(M5-13b).
  useEffect(() => {
    log.start();
    return () => log.dispose();
  }, [log]);
  const { rows, end, loading, error, filter } = useStore(log.store);
  const [clientFilter, setClientFilter] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [clearing, setClearing] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);
  const titleId = useId();

  const tools = useRpcQuery("mcp.tools.list", { projectId }, { retry: false }).data;
  const sessions = useMcpSessions();
  const toolNames = useMemo(
    () => toolChoices(tools, rows, filter.tool),
    [tools, rows, filter.tool],
  );
  const clientNames = useMemo(
    () => clientChoices(sessions, rows, clientFilter),
    [sessions, rows, clientFilter],
  );
  // 클라이언트 거르기는 받은 행에서 한다 — `mcp.audit.list` 에 client 인자가 없다. 한계: 고른 클라이언트의 행이 드물면(또는 없으면)
  // 맞는 줄이 화면을 채울 때까지 쪽을 이어 읽어 보관 기간 전부를 받고 쥔다. Host 에 인자를 더하는 것은 M5-12 병합 뒤 후속이다
  // (16-m5-plan M5-10 ③, M5-13 리뷰 F2) — 그때 결과 · 툴과 같이 `AuditFilter` 로 옮긴다.
  const shown = useMemo(
    () => (clientFilter === null ? rows : rows.filter((r) => clientKey(r.client) === clientFilter)),
    [rows, clientFilter],
  );

  const virtualizer = useVirtualizer({
    count: shown.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ACTIVITY_ROW_HEIGHT,
    getItemKey: (index) => shown[index]?.id ?? index,
    overscan: 10,
  });
  // 아래로 내려 읽는 중에 맨 위에 새 행이 붙으면, 보던 줄이 그 자리에 있게 붙은 만큼 스크롤을 내린다. 맨 위(0)에 있을 때는
  // 그대로 두어 새 행이 보인다(M5-13 리뷰 R1). 보던 줄은 화면 맨 위 줄로 잡는다 — 그 줄이 새 목록의 몇째로 갔는지로 붙은 수를 센다.
  const anchor = useRef<{ rows: readonly AuditRow[]; top: number }>({ rows: shown, top: 0 });
  useLayoutEffect(() => {
    const el = scrollRef.current;
    const before = anchor.current;
    anchor.current = { rows: shown, top: el?.scrollTop ?? 0 };
    if (!el || before.rows === shown || before.top <= 0) return;
    const was = Math.floor(before.top / ACTIVITY_ROW_HEIGHT);
    const id = before.rows[was]?.id;
    if (id === undefined) return;
    const now = shown.findIndex((r) => r.id === id);
    if (now < 0 || now === was) return;
    const top = before.top + (now - was) * ACTIVITY_ROW_HEIGHT;
    // 스크롤 상자를 직접 옮긴다 — 가상 목록은 그 스크롤 이벤트로 따라온다.
    el.scrollTop = top;
    anchor.current = { rows: shown, top };
  }, [shown]);
  const onScroll = () => {
    anchor.current = { rows: anchor.current.rows, top: scrollRef.current?.scrollTop ?? 0 };
  };
  const items = virtualizer.getVirtualItems();
  const lastDrawn = items.at(-1)?.index ?? -1;
  const shownPanel = usePanelShown(api);
  const drawnIds = () =>
    virtualizer
      .getVirtualItems()
      .map((item) => shown[item.index]?.id)
      .filter((id): id is number => id !== undefined);
  useLiveLog(log, client, host, projectId, shownPanel, drawnIds);
  // 끝에 가까워지면(거른 줄이 적어 끝이 보이는 때도) 다음 쪽.
  useEffect(() => {
    if (!loading && !end && lastDrawn >= shown.length - LOAD_AHEAD) log.loadMore();
  }, [lastDrawn, shown.length, loading, end, log, rows.length]);

  const clear = async () => {
    setConfirming(false);
    setClearing(true);
    try {
      await client.call("mcp.audit.clear", { projectId });
    } catch (e) {
      notify({
        level: "error",
        title: T.clearFailed,
        body: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setClearing(false);
      log.reset();
    }
  };

  return (
    <section
      data-testid="mcp-call-log"
      aria-labelledby={titleId}
      className="flex min-h-0 flex-1 flex-col"
    >
      <div className="flex flex-none flex-wrap items-center gap-1 border-b border-ui-border px-2 py-1 text-[12px]">
        <h3 id={titleId} className="text-[11px] font-semibold text-ui-muted-foreground uppercase">
          {T.logTitle}
        </h3>
        <span data-testid="mcp-log-scope" className="min-w-0 truncate text-ui-muted-foreground">
          {T.logScope}
        </span>
        <button
          type="button"
          data-testid="mcp-clear"
          className={`${BUTTON} ml-auto flex items-center gap-1`}
          disabled={clearing}
          onClick={() => setConfirming(true)}
        >
          <Trash2 size={12} aria-hidden />
          {T.clear}
        </button>
      </div>
      <div className="flex flex-none flex-wrap items-center gap-1 border-b border-ui-border px-2 py-1">
        <select
          data-mcp-filter="outcome"
          aria-label={T.filter.outcome}
          className={SELECT}
          value={filter.outcome ?? ""}
          onChange={(e) =>
            log.setFilter({ ...filter, outcome: (e.target.value || null) as AuditOutcome | null })
          }
        >
          <option value="">{T.filter.allOutcomes}</option>
          {OUTCOMES.map((o) => (
            <option key={o} value={o}>
              {T.outcome[o]}
            </option>
          ))}
        </select>
        <select
          data-mcp-filter="tool"
          aria-label={T.filter.tool}
          className={SELECT}
          value={filter.tool ?? ""}
          onChange={(e) => log.setFilter({ ...filter, tool: e.target.value || null })}
        >
          <option value="">{T.filter.allTools}</option>
          {toolNames.map((name) => (
            <option key={name} value={name}>
              {displayText(name)}
            </option>
          ))}
        </select>
        <select
          data-mcp-filter="client"
          aria-label={T.filter.client}
          className={SELECT}
          value={clientFilter ?? ""}
          onChange={(e) => setClientFilter(e.target.value === "" ? null : e.target.value)}
        >
          <option value="">{T.filter.allClients}</option>
          {clientNames.map((name) => (
            <option key={name} value={name}>
              {name === UNKNOWN_CLIENT ? strings.sessions.unknownClient : displayText(name)}
            </option>
          ))}
        </select>
      </div>
      {error !== undefined && (
        <p
          role="alert"
          data-testid="mcp-log-error"
          className="px-2 py-1 text-[12px] text-ui-accent"
        >
          {T.loadFailed(error)}
        </p>
      )}
      <div
        ref={scrollRef}
        role="table"
        aria-labelledby={titleId}
        // 다 읽기 전에는 전체 줄 수를 모른다 — ARIA 의 「모름」 은 -1 이다. 받은 줄 수는 `data-loaded-rows` 에 둔다.
        aria-rowcount={end ? shown.length + 1 : -1}
        data-loaded-rows={shown.length}
        tabIndex={0}
        data-testid="mcp-log-rows"
        onScroll={onScroll}
        className="min-h-0 flex-1 overflow-auto text-[12px] outline-none"
      >
        <div role="rowgroup" className="sticky top-0 z-10 min-w-[640px] bg-ui-panel">
          <div
            role="row"
            aria-rowindex={1}
            className={`${COLUMNS} border-b border-ui-border px-2 text-[11px] text-ui-muted-foreground`}
          >
            {(["time", "client", "tool", "args", "outcome", "duration", "approval"] as const).map(
              (c) => (
                <span key={c} role="columnheader" className="truncate">
                  {T.column[c]}
                </span>
              ),
            )}
          </div>
        </div>
        {shown.length === 0 ? (
          // 표 아래의 글도 행 · 셀로 둔다 — 표에 행이 아닌 자식이 있으면 보조 기술이 표를 잘못 읽는다.
          <div role="rowgroup">
            <div role="row">
              <p
                role="cell"
                data-testid="mcp-log-empty"
                className="px-2 py-1 text-ui-muted-foreground"
              >
                {loading || (!end && clientFilter !== null)
                  ? T.loading
                  : clientFilter !== null || filter.outcome !== null || filter.tool !== null
                    ? T.emptyFiltered
                    : T.empty}
              </p>
            </div>
          </div>
        ) : (
          <div
            role="rowgroup"
            className="relative min-w-[640px]"
            style={{ height: virtualizer.getTotalSize() }}
          >
            {items.map((item) => {
              const row = shown[item.index];
              if (!row) return null;
              return (
                <CallRow
                  key={item.key}
                  row={row}
                  index={item.index}
                  start={item.start}
                  T={T}
                  locale={locale}
                  unknownClient={strings.sessions.unknownClient}
                />
              );
            })}
          </div>
        )}
      </div>
      {confirming && (
        <ClearConfirm T={T} onAnswer={(ok) => (ok ? void clear() : setConfirming(false))} />
      )}
    </section>
  );
}

/** 툴 거르기의 이름: 카탈로그(`mcp.tools.list`)와 받은 행의 툴(다른 프로세스의 툴 · 지금 카탈로그에 없는 툴), 고른 것. */
function toolChoices(
  tools: readonly McpToolInfo[] | undefined,
  rows: readonly AuditRow[],
  selected: string | null,
): string[] {
  const names = new Set<string>();
  for (const t of tools ?? []) names.add(t.name);
  for (const r of rows) names.add(r.tool);
  if (selected !== null) names.add(selected);
  return [...names].sort();
}

/** 클라이언트 거르기의 값: 이 앱의 세션과 받은 행의 클라이언트, 고른 것. */
function clientChoices(
  sessions: readonly McpSessionInfo[],
  rows: readonly AuditRow[],
  selected: string | null,
): string[] {
  const names = new Set<string>();
  for (const s of sessions) names.add(clientKey(s.client));
  for (const r of rows) names.add(clientKey(r.client));
  if (selected !== null) names.add(selected);
  return [...names].sort();
}

function CallRow({
  row,
  index,
  start,
  T,
  locale,
  unknownClient,
}: {
  row: AuditRow;
  index: number;
  start: number;
  T: ActivityStrings;
  locale: string | undefined;
  unknownClient: string;
}) {
  const clientName = row.client === null ? unknownClient : displayText(row.client);
  const tool = displayText(row.tool);
  const args = displayText(row.args);
  const detail = row.detail === null ? undefined : displayText(row.detail);
  const done = row.outcome !== "started";
  return (
    <div
      role="row"
      aria-rowindex={index + 2}
      data-mcp-row={row.id}
      data-outcome={row.outcome}
      className={`${COLUMNS} absolute top-0 left-0 w-full items-center px-2 hover:bg-ui-hover`}
      style={{ transform: `translateY(${start}px)`, height: ACTIVITY_ROW_HEIGHT }}
    >
      <span
        role="cell"
        className="truncate tabular-nums"
        title={new Date(row.at).toLocaleString(locale)}
      >
        {clockTime(row.at, locale)}
      </span>
      <span role="cell" className="truncate" title={clientName}>
        {clientName}
      </span>
      <span role="cell" className="flex min-w-0 items-baseline gap-1" title={tool}>
        <span className="truncate">{tool}</span>
        {row.projectId === null && (
          <span data-mcp-no-project className="flex-none text-[10px] text-ui-muted-foreground">
            {T.noProject}
          </span>
        )}
      </span>
      <span role="cell" className="truncate text-ui-muted-foreground" title={args}>
        {args}
      </span>
      <span
        role="cell"
        data-mcp-outcome
        className={`truncate ${OUTCOME_CLASS[row.outcome]}`}
        title={detail ?? T.outcome[row.outcome]}
      >
        {T.outcome[row.outcome]}
      </span>
      <span role="cell" className="truncate text-right tabular-nums">
        {done ? T.duration(row.durationMs) : "—"}
      </span>
      <span role="cell" data-mcp-approval className="truncate">
        {row.approval === null ? T.approvalNone : T.approval[row.approval]}
      </span>
    </div>
  );
}

function ClearConfirm({ T, onAnswer }: { T: ActivityStrings; onAnswer(ok: boolean): void }) {
  useKeymapPopup(true);
  const cancel = useRef<HTMLButtonElement>(null);
  return (
    <Dialog.Root open onOpenChange={(open) => !open && onAnswer(false)}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content
          data-testid="mcp-clear-confirm"
          onOpenAutoFocus={(e) => {
            // 처음 포커스는 [취소] — 치던 Enter · Space 한 번으로 지워지지 않게(신뢰 확인 · 위험 작업 확인과 같은 규칙).
            e.preventDefault();
            cancel.current?.focus();
          }}
          className="fixed top-1/2 left-1/2 flex w-[420px] max-w-[94vw] -translate-x-1/2 -translate-y-1/2 flex-col gap-2 rounded-md border border-ui-border bg-ui-panel px-4 py-3 text-[13px] text-ui-foreground shadow-lg"
        >
          <Dialog.Title className="text-sm font-semibold">{T.clearTitle}</Dialog.Title>
          <Dialog.Description className="text-ui-muted-foreground">
            {T.clearBody}
          </Dialog.Description>
          <div className="flex justify-end gap-2">
            <button ref={cancel} type="button" className={BUTTON} onClick={() => onAnswer(false)}>
              {T.clearCancel}
            </button>
            <button
              type="button"
              data-testid="mcp-clear-confirm-button"
              className={`${BUTTON} border-ui-accent text-ui-accent`}
              onClick={() => onAnswer(true)}
            >
              {T.clearConfirm}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
