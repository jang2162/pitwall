// 프로젝트 창의 Redis 키 문서(14-m3-plan M3-18): 문서가 쓸 프로젝트 id 를 두고, 문서를 여는 액션을 등록한다. 다른 행은 액션을 id 로
// 부른다 — DB 탐색기(M3-13)의 Redis 연결 더블클릭 등.
//
//   registry.run("db.redis.open", { connectionId, database? })   그 연결 · DB 번호의 Redis 키 탭(이미 있으면 그 탭을 앞으로)
//
// 인자가 있어야만 돈다(`args: true` — 키 · 메뉴 · 액션 찾기에서는 부를 수 없다). `database` 가 없으면 연결 설정의 것, 그것도 없으면
// `"0"`. 탭은 연결하지 않은 채 열린다 — 연결은 탭의 [연결] 이 한다(결정 M3-D13 과 같은 규칙: 여는 것과 붙는 것을 가른다).

import type { Action } from "@pitwall/keymap";
import { useEffect, useMemo, useRef } from "react";
import { createStore } from "zustand";

import { openTab, tabParams } from "../../editor/editor-tabs";
import type { EditorWorkspace } from "../../editor/editor-workspace";
import { useRegisterActions } from "../../keymap/keymap-provider";
import { useNotify } from "../../notifications/notifications";
import { activeWorktree } from "../../open-with/active-worktree";
import { useHost } from "../../rpc/host";
import { type DbDocumentSpec, dbDocumentKey, newDbTabId } from "../documents/spec";
import { type DbRedisStrings, useDbRedisStrings } from "./strings";

/** 이 창의 프로젝트(문서가 RPC 의 `projectId` 로 쓴다). */
export const redisProject = createStore<{ projectId: string | undefined }>()(() => ({
  projectId: undefined,
}));

type RedisSpec = Extract<DbDocumentSpec, { kind: "redis" }>;

const isText = (v: unknown): v is string => typeof v === "string" && v !== "";

/**
 * Redis 키 탭을 연다 — 같은 연결 · DB 번호 · 워크트리의 탭이 있으면 그 탭을 앞으로. 열린 탭을 돌려준다. 연결 · 세션 RPC 는 부르지
 * 않는다.
 */
export function openRedisDocument(
  workspace: EditorWorkspace,
  {
    connectionId,
    database,
    title,
    worktree,
  }: { connectionId: string; database: string; title: string; worktree: string | undefined },
) {
  const api = workspace.api();
  if (!api) return undefined;
  const existing = api.panels.find((p) => {
    const { db, worktree: w } = tabParams(p);
    return (
      db?.kind === "redis" &&
      db.connectionId === connectionId &&
      db.database === database &&
      w === worktree
    );
  });
  if (existing) {
    existing.api.setActive();
    return existing;
  }
  const spec: RedisSpec = { kind: "redis", tabId: newDbTabId(), connectionId, database };
  const { panel } = openTab(api, {
    path: dbDocumentKey(spec),
    worktree,
    db: spec,
    title,
    recent: workspace.recent(),
  });
  panel.api.setActive();
  return panel;
}

export function redisActions(deps: {
  open(args: { connectionId: string; database?: string }): Promise<void>;
  strings: DbRedisStrings["actions"];
  fail(message: string): void;
}): Action[] {
  return [
    {
      id: "db.redis.open",
      label: deps.strings.open,
      args: true,
      run: async (args: unknown) => {
        const a = (typeof args === "object" && args !== null ? args : {}) as Record<
          string,
          unknown
        >;
        const database = a.database;
        if (
          !isText(a.connectionId) ||
          (database !== undefined && !(isText(database) && /^\d+$/.test(database)))
        ) {
          deps.fail(deps.strings.badArgs);
          return;
        }
        await deps.open({
          connectionId: a.connectionId,
          ...(database !== undefined && { database: database as string }),
        });
      },
    },
  ];
}

/** 프로젝트 창에 한 번 둔다. */
export function DbRedisFeature({
  projectId,
  workspace,
}: {
  projectId: string;
  workspace: EditorWorkspace;
}) {
  const T = useDbRedisStrings();
  const { client } = useHost();
  const notify = useNotify();
  const latest = useRef({ client, notify });
  latest.current = { client, notify };
  useEffect(() => {
    redisProject.setState({ projectId });
    return () => {
      if (redisProject.getState().projectId === projectId)
        redisProject.setState({ projectId: undefined });
    };
  }, [projectId]);
  const actions = useMemo(
    () =>
      redisActions({
        strings: T.actions,
        fail: (message) => latest.current.notify({ level: "warning", title: message }),
        open: async ({ connectionId, database }) => {
          const worktree = activeWorktree();
          // 이름 · 기본 DB 번호는 연결 설정에서(설정 읽기 — 연결을 열지 않는다). 읽지 못하면 id 와 0.
          let name = connectionId;
          let db = database;
          try {
            const entry = await latest.current.client?.call("db.connection.get", {
              projectId,
              worktree: worktree ?? null,
              connectionId,
            });
            if (entry?.config) {
              name = entry.config.name;
              const configured = entry.config.database;
              if (db === undefined && configured != null && /^\d+$/.test(configured))
                db = configured;
            }
          } catch {
            // 설정을 못 읽어도 탭은 연다 — 연결할 때 Host 가 까닭을 준다.
          }
          db ??= "0";
          openRedisDocument(workspace, {
            connectionId,
            database: db,
            title: T.title(name, db),
            worktree,
          });
        },
      }),
    [T, projectId, workspace],
  );
  useRegisterActions(actions);
  return null;
}
