// DB 탐색기 툴 윈도우(`⌘8`, 14-m3-plan.md M3-13): 화면(db-explorer.tsx)을 이 창의 Host · 액션 · 알림 · 다이얼로그에 잇는다.
//
// - 모델은 (Host 연결, 프로젝트, 활성 워크트리)마다 하나다 — 워크트리를 바꾸면 그 워크트리의 공유 설정으로 새로 읽는다.
// - 그 프로젝트의 `db.schemaChanged` 로 펼친 노드를 다시 읽고, `config.changed` 의 `db/<연결 id>` 로 목록을 다시 읽고 열었던 연결을
//   「설정 바뀜」 으로 둔다(저절로 다시 붙지 않는다 — 결정 M3-D13). 신뢰가 바뀌면(Host 범위 `project.changed` 의 `kind: trust` —
//   `projectId` 는 신뢰의 주인이라 워크트리 프로젝트와 다를 수 있어 id 는 보지 않는다) 목록을 다시 읽는다. 연결은 열지 않는다.
// - 툴 윈도우가 처음 보일 때 올린다(보이지 않는 동안 목록을 읽지 않는다). 연결은 사람이 펼치거나 「연결」 을 누를 때만 연다.
// - 액션 `db.explorer.ddl`(`⌘B`)은 고른 행이 테이블 · 뷰일 때만 켜진다. `db.explorer.reveal`(인자 `{ connectionId, object, kind }`)은
//   그 객체를 트리에서 골라 포커스를 둔다(Search Everywhere 의 「DB 객체」 탭, M3-20 — `ExplorerModel.reveal`).

import type { Action } from "@pitwall/keymap";
import type { ObjectRef } from "@pitwall/rpc-client";
import type { IDockviewPanelProps } from "dockview-react";
import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";

import { useEditorWorkspace } from "../../editor/editor-workspace";
import { useGitDialogs } from "../../git/dialogs/git-dialogs";
import { useKeymap, useRegisterActions } from "../../keymap/keymap-provider";
import { useNotify, usePanelVisible } from "../../notifications/notifications";
import { useActiveWorktree } from "../../open-with/active-worktree";
import { useHost } from "../../rpc/host";
import { writeClipboardText } from "../grid/clipboard";
import { useDbAccessPrompt } from "../safety/access-prompt";
import { DbExplorerView, type ExplorerOps } from "./db-explorer";
import { openDdlDocument } from "./ddl-document";
import { EXPLORER_ACTIONS } from "./explorer-actions";
import { type BrowsedKind, ExplorerModel, shapeOf } from "./explorer-model";
import { type DbExplorerStrings, useDbExplorerStrings } from "./strings";

/** `db.schemaChanged`(crates/pw-db/src/schema.rs `TOPIC_SCHEMA_CHANGED`). */
export const DB_SCHEMA_CHANGED_TOPIC = "db.schemaChanged";
/** `config.changed`(pw_core::config::TOPIC_CHANGED) — 연결 설정은 `db/<연결 id>` 로 온다(crates/pw-db/src/config.rs). */
export const CONFIG_CHANGED_TOPIC = "config.changed";
/** `project.changed`(pw_core::registry::TOPIC_CHANGED, Host 범위) — 신뢰가 바뀌면 `kind: "trust"`. */
export const PROJECT_CHANGED_TOPIC = "project.changed";
const CONNECTION_PREFIX = "db/";

/** `config.changed` 의 `changed` 에서 연결 id. */
export function changedConnections(payload: unknown): string[] {
  const changed = (payload as { changed?: unknown } | null)?.changed;
  if (!Array.isArray(changed)) return [];
  return changed
    .filter((c): c is string => typeof c === "string" && c.startsWith(CONNECTION_PREFIX))
    .map((c) => c.slice(CONNECTION_PREFIX.length));
}

/** 모델에 그 프로젝트의 이벤트를 잇는다. 해제 함수를 돌려준다. */
export function followExplorerEvents(
  client: { subscribe: import("@pitwall/rpc-client").RpcClient["subscribe"] },
  projectId: string,
  model: ExplorerModel,
): () => void {
  const opened = client.subscribe(
    { projectId, topics: [DB_SCHEMA_CHANGED_TOPIC, CONFIG_CHANGED_TOPIC] },
    (event) => {
      if (event.topic === DB_SCHEMA_CHANGED_TOPIC) {
        const connectionId = (event.payload as { connectionId?: unknown } | null)?.connectionId;
        if (typeof connectionId === "string") void model.schemaChanged(connectionId);
        return;
      }
      const ids = changedConnections(event.payload);
      if (ids.length === 0) return;
      model.configChanged(ids);
      void model.reloadList();
    },
  );
  const trust = client.subscribe({ topics: [PROJECT_CHANGED_TOPIC] }, (event) => {
    if ((event.payload as { kind?: unknown } | null)?.kind === "trust") void model.reloadList();
  });
  for (const s of [opened, trust])
    s.catch((e: unknown) => console.warn("[db] cannot follow the explorer's events", e));
  return () => {
    for (const s of [opened, trust]) s.then((x) => x.unsubscribe()).catch(() => {});
  };
}

export function DbExplorerToolWindow({ api }: IDockviewPanelProps) {
  const visible = usePanelVisible(api);
  const [shown, setShown] = useState(visible);
  if (visible && !shown) setShown(true);
  return (
    <div
      data-tool-window={api.id}
      tabIndex={-1}
      className="flex h-full flex-col bg-ui-panel text-ui-foreground outline-none"
    >
      {shown && <DbExplorerPanel />}
    </div>
  );
}

/** 탐색기가 여는 DDL 문서(ddl-document.tsx — 에디터 탭). */
export type OpenDdl = (connectionId: string, object: ObjectRef) => void;

/** 툴 윈도우는 프로젝트 창의 에디터 작업 공간 안에서 그려진다. 측정 하네스처럼 밖이면 없다. */
function useEditorWorkspaceOptional() {
  try {
    return useEditorWorkspace();
  } catch {
    return undefined;
  }
}

function DbExplorerPanel() {
  const T = useDbExplorerStrings();
  const { client, target } = useHost();
  const projectId = "projectId" in target ? target.projectId : undefined;
  const worktree = useActiveWorktree();
  const workspace = useEditorWorkspaceOptional();
  const openDdl = useCallback<OpenDdl>(
    (connectionId, object) => {
      if (workspace)
        openDdlDocument(workspace, connectionId, object, T.ddl.title(object.name), worktree);
    },
    [workspace, T, worktree],
  );
  const { registry, shortcuts } = useKeymap();
  const notify = useNotify();
  const dialogs = useGitDialogs();

  const access = useDbAccessPrompt();
  const resolveAccess = useRef(access.resolve);
  resolveAccess.current = access.resolve;

  const model = useMemo(() => {
    if (!client || !projectId) return undefined;
    const made: ExplorerModel = new ExplorerModel({
      calls: client,
      projectId,
      worktree,
      access: (error, connectionId) =>
        resolveAccess.current(error, {
          calls: client,
          projectId,
          worktree: worktree ?? null,
          connectionId,
          connectionName: made.entry(connectionId)?.config?.name ?? connectionId,
        }),
    });
    return made;
  }, [client, projectId, worktree]);

  useEffect(() => {
    if (!model || !client || !projectId) return;
    void model.reloadList();
    return followExplorerEvents(client, projectId, model);
  }, [model, client, projectId]);

  // 드라이버의 지원 플래그로 트리 단계를 정한다(받기 전 · 실패하면 내장 드라이버의 모양).
  useEffect(() => {
    if (!model || !client) return;
    let live = true;
    client.call("db.driver.list", null).then(
      (drivers) => {
        if (live) model.setShapes(new Map(drivers.map((d) => [d.kind, shapeOf(d.capabilities)])));
      },
      (e: unknown) => console.warn("[db] cannot read the db drivers", e),
    );
    return () => {
      live = false;
    };
  }, [model, client]);

  const ops = useMemo<ExplorerOps>(
    () => ({
      copy: writeClipboardText,
      confirm: dialogs.confirm,
      notify,
      openDdl,
      shortcut: (action) => shortcuts(action)[0]?.text,
    }),
    [dialogs.confirm, notify, openDdl, shortcuts],
  );

  useSyncExternalStore(model?.subscribe ?? noop, model?.getVersion ?? zero);
  // 고른 행으로 스크롤하고 트리에 포커스를 두라는 요청(`db.explorer.reveal`).
  const [focusRequest, setFocusRequest] = useState(0);
  const actions = useMemo(
    () =>
      model
        ? explorerActions({
            strings: T,
            model,
            openDdl: ops.openDdl,
            reveal: async (target) => {
              await model.reveal(target.connectionId, target.object, target.kind);
              setFocusRequest((n) => n + 1);
            },
          })
        : [],
    [model, ops, T],
  );
  useRegisterActions(actions);

  if (!model) return null;
  return (
    <>
      <DbExplorerView model={model} registry={registry} ops={ops} focusRequest={focusRequest} />
      {dialogs.element}
      {access.element}
    </>
  );
}

const noop = () => () => {};
const zero = () => 0;

/** `db.explorer.reveal` 의 인자. */
export interface RevealTarget {
  connectionId: string;
  object: ObjectRef;
  kind: BrowsedKind;
}

/** 인자가 `RevealTarget` 꼴인가(다른 기능이 레지스트리로 넘긴 값이다). */
function revealTarget(args: unknown): RevealTarget | undefined {
  const a = args as Partial<RevealTarget> | null | undefined;
  if (typeof a?.connectionId !== "string" || (a.kind !== "table" && a.kind !== "view"))
    return undefined;
  if (typeof a.object?.name !== "string") return undefined;
  return a as RevealTarget;
}

/** 탐색기가 등록하는 액션. `reveal` 이 없으면 `db.explorer.reveal` 을 등록하지 않는다. */
export function explorerActions({
  strings,
  model,
  openDdl,
  reveal,
}: {
  strings: Pick<DbExplorerStrings, "menu">;
  model: Pick<ExplorerModel, "selected" | "node">;
  openDdl: OpenDdl;
  reveal?: (target: RevealTarget) => Promise<void>;
}): Action[] {
  return [
    ...(reveal
      ? [
          {
            id: EXPLORER_ACTIONS.reveal,
            label: strings.menu.reveal,
            args: true,
            run: (args: unknown) => {
              const target = revealTarget(args);
              return target ? reveal(target) : undefined;
            },
          } satisfies Action,
        ]
      : []),
    {
      id: EXPLORER_ACTIONS.ddl,
      label: strings.menu.ddl,
      keywords: ["Show DDL", "Database Explorer"],
      enabled: () => selectedObject(model) !== undefined,
      run: () => {
        const node = selectedObject(model);
        if (node) openDdl(node.connectionId, node.object);
      },
    },
  ];
}

function selectedObject(model: Pick<ExplorerModel, "selected" | "node">) {
  const id = model.selected();
  const node = id === undefined ? undefined : model.node(id);
  return node?.kind === "object" ? node : undefined;
}
