// 연결 편집 다이얼로그를 여는 액션(14-m3-plan.md M3-14). 프로젝트 창이 그린다.
//
// - `db.connection.add`: 「연결 추가」. 액션 찾기에 보인다. DB 탐색기(M3-13)의 툴바 · 빈 상태가 이 id 로 부른다.
// - `db.connection.edit`: 인자 `{ connectionId: string }` 가 있어야만 돈다(`args: true` — 액션 찾기에 나오지 않는다). 탐색기의
//   우클릭 「연결 편집」 이 `registry.run("db.connection.edit", { connectionId })` 로 부른다.
//
// 키 바인딩은 두지 않는다. 부르는 연결은 프로젝트 창의 연결이다 — 설정 쓰기 · 비밀 쓰기 · 연결 테스트가 그 창의 범위를 지난다.
//
// **비밀 입력 요청**(16-m5-plan M5-17 ②): 에이전트의 DB 툴이 비밀이 필요한 연결에 닿으면 Host 가 그 프로젝트 범위로
// `db.secretRequested`(crates/pw-db/src/connection_tool.rs)를 낸다. 이 창은 「이 연결의 비밀번호를 입력」 알림을 띄우고, 그 단추가
// 기존 연결 편집 다이얼로그를 비밀번호 칸에 포커스를 두고 연다. 비밀은 에이전트를 거치지 않는다 — 사람이 이 칸에 넣어 저장한다.

import type { Action } from "@pitwall/keymap";
import type { DbSecretRequested, EventMessage, RpcClient } from "@pitwall/rpc-client";
import { useEffect, useMemo, useRef, useState } from "react";

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

/** 비밀 입력 요청의 토픽(crates/pw-db/src/connection_tool.rs 의 `TOPIC_SECRET_REQUESTED`). */
export const SECRET_REQUESTED_TOPIC = "db.secretRequested";

/** 그 프로젝트의 비밀 입력 요청을 구독해 `show` 로 넘긴다. 돌려준 함수가 구독을 걷는다. */
export function followSecretRequests(
  client: RpcClient,
  projectId: string,
  show: (request: DbSecretRequested) => void,
): () => void {
  let disposed = false;
  const opened = client.subscribe(
    { projectId, topics: [SECRET_REQUESTED_TOPIC] },
    (event: EventMessage) => {
      if (disposed || event.topic !== SECRET_REQUESTED_TOPIC) return;
      const payload = event.payload as unknown as DbSecretRequested;
      if (typeof payload?.connectionId !== "string") return;
      show(payload);
    },
  );
  opened.catch((error: unknown) => console.warn("[db] could not follow secret requests", error));
  return () => {
    disposed = true;
    opened.then((s) => s.unsubscribe()).catch(() => {});
  };
}

interface OpenDialog {
  connectionId: string | undefined;
  seq: number;
  /** 비밀 입력 요청으로 열었다 — 그 요청의 워크트리 설정으로 열고 비밀번호 칸으로 간다. */
  secret?: { worktree: string | null };
}

export function DbConnectionFeature({ projectId }: { projectId: string }) {
  const T = useDbConnectionStrings();
  const { client } = useHost();
  const notify = useNotify();
  // 열린 다이얼로그. `seq` 는 같은 연결을 다시 열 때 새로 읽게 한다.
  const [open, setOpen] = useState<OpenDialog>();
  const seq = useRef(0);
  const actions = useMemo(
    () =>
      connectionActions(T.actions, (connectionId) => setOpen({ connectionId, seq: ++seq.current })),
    [T],
  );
  useRegisterActions(actions);
  const secretT = useRef(T.secretRequest);
  secretT.current = T.secretRequest;
  useEffect(() => {
    if (!client) return;
    return followSecretRequests(client, projectId, (request) =>
      notify({
        level: "warning",
        title: secretT.current.title(request.name),
        body: secretT.current.body,
        action: {
          label: secretT.current.action,
          run: () =>
            setOpen({
              connectionId: request.connectionId,
              seq: ++seq.current,
              secret: { worktree: request.worktree },
            }),
        },
      }),
    );
  }, [client, projectId, notify]);
  if (!open || !client) return null;
  const worktree = open.secret ? (open.secret.worktree ?? undefined) : activeWorktree();
  return (
    <ConnectionDialog
      key={open.seq}
      deps={{ client, projectId, ...(worktree !== undefined && { worktree }), notify }}
      {...(open.connectionId !== undefined && { connectionId: open.connectionId })}
      focusSecret={open.secret !== undefined}
      onClose={() => setOpen(undefined)}
    />
  );
}
