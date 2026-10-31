// SQL 콘솔의 창 단위(14-m3-plan.md M3-15): 프로젝트 창에 한 번 둔다.
//
// - 액션: `db.console.open`(⌘⇧L, Win/Linux Ctrl+Shift+F10 — 인자 `{ connectionId?, database? }` 로 DB 탐색기(M3-13)가 부른다, 인자가
//   없으면 이 창에서 마지막으로 연결한 연결), `db.console.run`(⌘↩) · `db.console.runAll` · `db.console.cancel`(⌘F2) ·
//   `db.console.explain` — 키를 받은 순간 포커스가 든 콘솔, 없으면 에디터 영역의 지금 탭의 콘솔(console-window.ts).
//   ⌘↩ · ⌘F2 의 키는 `when = "dbConsoleFocus"` 다(⌘↩ 은 커밋 창의 커밋, ⌘F2 는 실행 「중지」 와 범위가 갈린다).
// - 열린 트랜잭션 질문(transaction-dialog.tsx)을 창에 하나 그린다 — 콘솔 탭들의 닫기 질문이 이것으로 차례로 묻는다.
// - 탭이 닫히면(도킹 영역에서 패널이 빠지면) 그 콘솔의 글을 마저 쓰고 세션을 닫는다. 창을 새로고침 · 닫는 것은 탭을 닫는 것이
//   아니다 — 세션은 Host 가 정리하고 열린 트랜잭션은 보존한다(M3-05 ⓘ).
// - 창이 열릴 때 `db.session.unattendedRollbacks` 를 읽어, 그사이 보는 사람이 없어 롤백된 열린 트랜잭션을 알린다. 이 창에서 이미
//   알린 것(롤백 시각)은 다시 알리지 않는다(웹뷰 저장소 — 읽지 못하면 다시 알린다).

import type { Action } from "@pitwall/keymap";
import type { DbUnattendedRollback } from "@pitwall/rpc-client";
import type { DockviewApi } from "dockview-react";
import { useEffect, useMemo, useRef, useState } from "react";

import type { EditorWorkspace } from "../../editor/editor-workspace";
import { useI18n } from "../../i18n/i18n-provider";
import { useRegisterActions } from "../../keymap/keymap-provider";
import { useNotify } from "../../notifications/notifications";
import { useHost } from "../../rpc/host";
import { openConsoleTab } from "./console-tab";
import { consoleOf, consoleWindow, lastConnection, targetConsole } from "./console-window";
import { type DbConsoleStrings, useDbConsoleStrings } from "./strings";
import { useTransactionPrompt } from "./transaction-dialog";

/** `db.console.open` 의 인자. */
export interface ConsoleOpenArgs {
  connectionId?: string | null;
  database?: string | null;
}

function openArgs(args: unknown): ConsoleOpenArgs {
  if (!args || typeof args !== "object") return {};
  const { connectionId, database } = args as Record<string, unknown>;
  return {
    ...(typeof connectionId === "string" && { connectionId }),
    ...(typeof database === "string" && { database }),
  };
}

export function consoleActions(deps: {
  strings: DbConsoleStrings["actions"];
  activeTab(): string | undefined;
  open(args: ConsoleOpenArgs): void;
}): Action[] {
  const T = deps.strings;
  const target = () => targetConsole(deps.activeTab());
  return [
    {
      id: "db.console.open",
      label: T.open,
      keywords: ["SQL Console", "Query Console", "New Console"],
      run: (args) => deps.open(openArgs(args)),
    },
    {
      id: "db.console.run",
      label: T.run,
      keywords: ["Execute", "Run Statement"],
      enabled: () => target()?.canRun() === true,
      run: () => target()?.run(false),
    },
    {
      id: "db.console.runAll",
      label: T.runAll,
      keywords: ["Execute All", "Run Script"],
      enabled: () => target()?.canRun() === true,
      run: () => target()?.run(true),
    },
    {
      id: "db.console.cancel",
      label: T.cancel,
      keywords: ["Cancel Query", "Stop"],
      enabled: () => target()?.canCancel() === true,
      run: () => target()?.cancel(),
    },
    {
      id: "db.console.explain",
      label: T.explain,
      keywords: ["Explain Plan", "Query Plan"],
      enabled: () => target()?.canRun() === true,
      run: () => target()?.explain(),
    },
  ];
}

const SEEN_KEY = "pitwall.db.unattendedSeen";
/** UI 언어(`i18n.get`)를 기다리는 시간 — 넘으면 지금 언어로 알린다. */
const LANGUAGE_WAIT_MS = 2000;

function seenUntil(projectId: string): number {
  try {
    const all = JSON.parse(localStorage.getItem(SEEN_KEY) ?? "{}") as Record<string, unknown>;
    const at = all[projectId];
    return typeof at === "number" ? at : 0;
  } catch {
    return 0;
  }
}

function markSeen(projectId: string, at: number) {
  try {
    const all = JSON.parse(localStorage.getItem(SEEN_KEY) ?? "{}") as Record<string, unknown>;
    localStorage.setItem(SEEN_KEY, JSON.stringify({ ...all, [projectId]: at }));
  } catch {
    // 다음에 창이 열릴 때 다시 알릴 뿐이다.
  }
}

export function DbConsoleFeature({
  projectId,
  workspace,
  api,
}: {
  projectId: string;
  workspace: Pick<EditorWorkspace, "activeTab" | "api" | "recent" | "focus">;
  api: Pick<DockviewApi, "onDidRemovePanel"> | undefined;
}) {
  const T = useDbConsoleStrings();
  const prompt = useTransactionPrompt();
  const notify = useNotify();
  const { client } = useHost();
  const latest = useRef({ notify, T });
  latest.current = { notify, T };

  useEffect(() => {
    consoleWindow.setState({ projectId, askTransaction: prompt.ask });
    return () => consoleWindow.setState({ projectId: undefined, askTransaction: undefined });
  }, [projectId, prompt.ask]);

  useEffect(() => {
    const removed = api?.onDidRemovePanel((panel) => consoleOf(panel.id)?.dispose());
    return () => removed?.dispose();
  }, [api]);

  const actions = useMemo(
    () =>
      consoleActions({
        strings: T.actions,
        activeTab: () => workspace.activeTab()?.id,
        open: (args) =>
          void openConsoleTab(
            workspace,
            {
              connectionId: args.connectionId ?? lastConnection(projectId) ?? null,
              database: args.database ?? null,
            },
            T.title,
          ),
      }),
    [T, workspace, projectId],
  );
  useRegisterActions(actions);

  // 창이 열릴 때 한 번: 보는 사람이 없어 롤백된 트랜잭션. UI 언어가 정해진 뒤에 알린다(응답 전의 번들 언어로 알리지 않게) — 언어를
  // 받지 못하면 `LANGUAGE_WAIT_MS` 뒤에 그대로 알린다.
  const languageReady = useI18n().setting !== undefined;
  const [waited, setWaited] = useState(false);
  useEffect(() => {
    const timer = setTimeout(() => setWaited(true), LANGUAGE_WAIT_MS);
    return () => clearTimeout(timer);
  }, []);
  const speak = languageReady || waited;
  useEffect(() => {
    if (!client || !speak) return;
    let live = true;
    client.call("db.session.unattendedRollbacks", { projectId }).then(
      (records: DbUnattendedRollback[]) => {
        if (!live) return;
        const since = seenUntil(projectId);
        const fresh = records.filter((r) => r.rolledBackAt > since);
        const { notify, T } = latest.current;
        for (const r of fresh)
          notify({
            level: "warning",
            title: T.unattended.title,
            body:
              r.since === null || r.keptMinutes === 0
                ? T.unattended.bodyUnknown(r.name)
                : T.unattended.body(r.name, new Date(r.since).toLocaleString(), r.keptMinutes),
          });
        const last = Math.max(since, ...fresh.map((r) => r.rolledBackAt));
        if (last > since) markSeen(projectId, last);
      },
      () => {},
    );
    return () => {
      live = false;
    };
  }, [client, projectId, speak]);

  return prompt.element ?? null;
}
