// 상태바 `mcp` 항목과 웰컴 창의 연결 수(16-m5-plan M5-10 ⑤, 10-ui.md 「S4. 프로젝트 창」 그림의 「MCP 2」 · 「S1. 웰컴」 의
// 「MCP: 연결 n」).
//
// - 상태바: 이 프로젝트를 세션 프로젝트로 가진 MCP 세션 수. 0 이면 항목을 그리지 않는다. 누르면 세션 목록 팝업(클라이언트 · 시작
//   시각 · 호출 수) — 「에이전트 활동」 툴 윈도우(M5-13)가 생기면 그것을 여는 것으로 바꾼다.
// - 웰컴 창: 이 앱에 열린 세션 전부의 수.
// - 수는 `mcp.sessions`(sessions.ts)에서 센다. 세션이 열리고 닫히면 `mcp.sessionsChanged` 로 다시 읽는다.

import type { McpSessionInfo } from "@pitwall/rpc-client";
import * as Dialog from "@radix-ui/react-dialog";
import { Bot } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { createStore, useStore } from "zustand";

import { useKeymapPopup } from "../keymap/keymap-provider";
import { STATUS_SLOTS, useStatusBarItem } from "../status-bar/status-bar";
import { sessionsOf, useMcpSessions } from "./sessions";
import { type McpStrings, useMcpStrings } from "./strings";

const BUTTON =
  "rounded-sm border border-ui-border px-2 py-0.5 text-[12px] hover:bg-ui-hover disabled:opacity-50";

/** 이 창의 프로젝트. 항목 컴포넌트는 props 를 받지 않는다(status-bar.tsx 머리 주석) — 창마다 웹뷰가 따로라 모듈 상태로 둔다. */
const mcpState = createStore<{ projectId: string | undefined }>()(() => ({
  projectId: undefined,
}));

function time(at: number) {
  return new Date(at).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
}

function SessionRow({ session, T }: { session: McpSessionInfo; T: McpStrings["sessions"] }) {
  return (
    <li data-testid="mcp-session" className="flex items-baseline gap-2 py-0.5 text-[12px]">
      <span className="min-w-0 flex-1 truncate" title={session.client ?? undefined}>
        {session.client ?? T.unknownClient}
      </span>
      <span className="text-ui-muted-foreground">{T.started(time(session.startedAt))}</span>
      <span className="text-ui-muted-foreground">{T.calls(session.calls)}</span>
    </li>
  );
}

function SessionsPopup({
  sessions,
  onClose,
}: {
  sessions: readonly McpSessionInfo[];
  onClose: () => void;
}) {
  const T = useMcpStrings().sessions;
  useKeymapPopup(true);
  const closeButton = useRef<HTMLButtonElement>(null);
  return (
    <Dialog.Root open onOpenChange={(open) => !open && onClose()}>
      <Dialog.Portal>
        <Dialog.Content
          data-testid="mcp-sessions-popup"
          aria-describedby={undefined}
          onOpenAutoFocus={(e) => {
            e.preventDefault();
            closeButton.current?.focus();
          }}
          className="fixed bottom-7 left-2 flex max-h-[70vh] w-[420px] max-w-[94vw] flex-col gap-1 overflow-y-auto rounded-md border border-ui-border bg-ui-panel px-3 py-2 text-ui-foreground shadow-lg"
        >
          <Dialog.Title className="text-sm font-semibold">{T.title}</Dialog.Title>
          {sessions.length === 0 ? (
            <p className="text-[12px] text-ui-muted-foreground">{T.empty}</p>
          ) : (
            <ul>
              {sessions.map((s) => (
                <SessionRow key={s.id} session={s} T={T} />
              ))}
            </ul>
          )}
          <div className="flex justify-end">
            <button ref={closeButton} type="button" className={BUTTON} onClick={onClose}>
              {T.close}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function McpStatusFor({ projectId }: { projectId: string }) {
  const T = useMcpStrings();
  const mine = sessionsOf(useMcpSessions(), projectId);
  const [open, setOpen] = useState(false);
  // 마지막 세션이 닫히면 항목이 사라진다 — 열린 팝업은 빈 목록을 보이며 남는다(닫기는 사람이).
  if (mine.length === 0 && !open) return null;
  return (
    <>
      {mine.length > 0 && (
        <button
          type="button"
          data-testid="status-mcp"
          title={T.status.hint(mine.length)}
          onClick={() => setOpen(true)}
          className="flex items-center gap-1 rounded-sm px-1 hover:bg-ui-hover"
        >
          <Bot size={11} aria-hidden />
          {T.status.label(mine.length)}
        </button>
      )}
      {open && <SessionsPopup sessions={mine} onClose={() => setOpen(false)} />}
    </>
  );
}

/** 상태바 항목: 이 프로젝트의 MCP 세션 수. */
export function McpStatusItem() {
  const projectId = useStore(mcpState, (s) => s.projectId);
  return projectId === undefined ? null : <McpStatusFor projectId={projectId} />;
}

/** 이 창의 프로젝트로 MCP 항목을 둔다. `StatusBarProvider` 안에 둔다. */
export function McpStatus({ projectId }: { projectId: string }) {
  useEffect(() => {
    mcpState.setState({ projectId });
    return () => mcpState.setState({ projectId: undefined });
  }, [projectId]);
  useStatusBarItem({ id: "mcp", ...STATUS_SLOTS.mcp, component: McpStatusItem });
  return null;
}

/** 웰컴 창 아래 줄의 「MCP: 연결 n」 — 이 앱에 열린 세션 전부. */
export function McpConnections() {
  const T = useMcpStrings().welcome;
  const count = useMcpSessions().length;
  return <span data-testid="welcome-mcp">{T.connections(count)}</span>;
}
