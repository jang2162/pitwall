// 연결 편집 다이얼로그를 여는 액션(14-m3-plan.md M3-14). 프로젝트 창이 그린다.
//
// - `db.connection.add`: 「연결 추가」. 액션 찾기에 보인다. DB 탐색기(M3-13)의 툴바 · 빈 상태가 이 id 로 부른다.
// - `db.connection.edit`: 인자 `{ connectionId: string }` 가 있어야만 돈다(`args: true` — 액션 찾기에 나오지 않는다). 탐색기의
//   우클릭 「연결 편집」 이 `registry.run("db.connection.edit", { connectionId })` 로 부른다.
//
// 키 바인딩은 두지 않는다. 부르는 연결은 프로젝트 창의 연결이다 — 설정 쓰기 · 비밀 쓰기 · 연결 테스트가 그 창의 범위를 지난다.

import type { Action } from "@pitwall/keymap";
import { useMemo, useRef, useState } from "react";

import { useRegisterActions } from "../../keymap/keymap-provider";
import { useNotify } from "../../notifications/notifications";
import { activeWorktree } from "../../open-with/active-worktree";
import { useHost } from "../../rpc/host";
import { ConnectionDialog } from "./connection-dialog";
import { type DbConnectionStrings, useDbConnectionStrings } from "./strings";

/** `db.connection.edit` 의 인자. 맞지 않으면 `undefined`. */
export function editArgs(args: unknown): { connectionId: string } | undefined {
  if (typeof args !== "object" || args === null) return undefined;
  const id = (args as { connectionId?: unknown }).connectionId;
  return typeof id === "string" && id !== "" ? { connectionId: id } : undefined;
}

export function connectionActions(
  T: DbConnectionStrings["actions"],
  open: (connectionId: string | undefined) => void,
): Action[] {
  return [
    {
      id: "db.connection.add",
      label: T.add,
      keywords: ["Add Data Source", "New Connection", "Database", "DB"],
      run: () => open(undefined),
    },
    {
      id: "db.connection.edit",
      label: T.edit,
      args: true,
      run: (args) => {
        const parsed = editArgs(args);
        if (!parsed) {
          console.warn("[db] db.connection.edit needs { connectionId }", args);
          return;
        }
        open(parsed.connectionId);
      },
    },
  ];
}

export function DbConnectionFeature({ projectId }: { projectId: string }) {
  const T = useDbConnectionStrings();
  const { client } = useHost();
  const notify = useNotify();
  // 열린 다이얼로그. `seq` 는 같은 연결을 다시 열 때 새로 읽게 한다.
  const [open, setOpen] = useState<{ connectionId: string | undefined; seq: number }>();
  const seq = useRef(0);
  const actions = useMemo(
    () =>
      connectionActions(T.actions, (connectionId) => setOpen({ connectionId, seq: ++seq.current })),
    [T],
  );
  useRegisterActions(actions);
  if (!open || !client) return null;
  const worktree = activeWorktree();
  return (
    <ConnectionDialog
      key={open.seq}
      deps={{ client, projectId, ...(worktree !== undefined && { worktree }), notify }}
      {...(open.connectionId !== undefined && { connectionId: open.connectionId })}
      onClose={() => setOpen(undefined)}
    />
  );
}
