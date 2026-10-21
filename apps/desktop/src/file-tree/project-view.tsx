// 창 하나의 파일 트리(프로젝트 뷰, docs/design/12-m1-plan.md M1-14)를 Host 에 잇는다. 트리 모델은 창마다 하나이고(창마다 웹뷰가
// 따로라 모듈 상태로 둔다) 툴 윈도우가 숨거나 다시 열려도 펼침 · 선택이 남는다.
//
// - 루트 = 활성 워크트리 안의 프로젝트 루트(없으면 프로젝트의 등록 루트). 활성 워크트리가 바뀌면 다시 뿌리내린다 — 워크트리 안의
//   루트는 `project.resolve` 의 `worktree.root` 로 받는다(Host 의 `fs.*` 접근 범위와 같은 계산).
// - `fs.changed`(M1-06)를 툴 윈도우가 보이든 말든 받아 모델에 반영한다. 구독이 열리면 펼친 폴더를 한 번 다시 읽는다 — 열기 전의
//   변경을 놓쳤을 수 있다.
// - 「프로젝트 뷰에서 선택」 은 프로젝트 툴 윈도우를 띄우고 요청을 `revealRequests` 에 남긴다. 툴 윈도우(file-tree.tsx)가 마운트돼
//   있으면 곧바로, 아니면 마운트될 때 받는다. 「다음에서 열기」(⌥F1) 팝업의 첫 항목(`OpenWithPlace`)이 이것이다.

import type { Action } from "@pitwall/keymap";
import type { RpcClient } from "@pitwall/rpc-client";
import type { DockviewApi } from "dockview-react";
import { useEffect, useMemo, useRef } from "react";
import { createStore } from "zustand/vanilla";

import type { EditorWorkspace } from "../editor/editor-workspace";
import { showPanel } from "../layout/project-layout";
import { useActiveWorktree } from "../open-with/active-worktree";
import type { OpenWithPlace } from "../open-with/open-with";
import { useHost, useRpcQuery } from "../rpc/host";
import { type FileTreeStrings, useFileTreeStrings } from "./strings";
import { FileTreeModel, type FsChanged } from "./tree-model";

/** `lang.detect` 한 번에 넣을 수 있는 파일 수(crates/pw-lang/src/methods.rs `MAX_FILES`). */
const DETECT_CHUNK = 10_000;

/** 프로젝트 툴 윈도우의 id(layout/project-layout.ts 의 PANELS). */
export const PROJECT_TOOL_WINDOW = "project";

/** 모델이 Host 를 부를 때 쓰는 지금의 연결 · 프로젝트 · 워크트리. `worktree` 는 루트를 바꾸기 직전에 고친다. */
export interface TreeScope {
  client: RpcClient | undefined;
  projectId: string | undefined;
  /** 외부 도구 팝업의 제목. */
  name: string | undefined;
  worktree: string | undefined;
}

/** Host 를 부르는 모델. `scope()` 는 부를 때마다 읽는다. */
export function hostFileTree(scope: () => TreeScope): FileTreeModel {
  const host = () => {
    const { client, projectId, worktree } = scope();
    if (!client || projectId === undefined) throw new Error("Host is not connected");
    return { client, projectId, worktree: worktree ?? null };
  };
  return new FileTreeModel({
    list: async (path) => {
      const { client, ...rest } = host();
      return client.call("fs.list", { ...rest, path });
    },
    dimmed: async (paths) => {
      const { client, ...rest } = host();
      const out: boolean[] = [];
      for (let i = 0; i < paths.length; i += DETECT_CHUNK) {
        const { files } = await client.call("lang.detect", {
          ...rest,
          files: paths.slice(i, i + DETECT_CHUNK).map((path) => ({ path })),
        });
        for (const f of files) out.push(f.vendored || f.generated || f.documentation);
      }
      return out;
    },
  });
}

const EMPTY_SCOPE: TreeScope = {
  client: undefined,
  projectId: undefined,
  name: undefined,
  worktree: undefined,
};

/** 이 창의 트리 범위. 툴 윈도우가 구독한다(파일을 열 워크트리 · 외부 도구 대상). */
export const treeScope = createStore<TreeScope>()(() => EMPTY_SCOPE);

let windowModel: FileTreeModel | undefined;

/** 이 창의 트리 모델. */
export function windowFileTree(): FileTreeModel {
  windowModel ??= hostFileTree(() => treeScope.getState());
  return windowModel;
}

/** 시험용: 창의 모델 · 범위 · 요청을 버린다. */
export function resetWindowFileTree() {
  windowModel = undefined;
  treeScope.setState(EMPTY_SCOPE, true);
  revealRequests.setState({ path: undefined, seq: 0 });
}

/** 「프로젝트 뷰에서 선택」 요청. 툴 윈도우가 받아 고르고 비운다(`path` 를 지운다). */
export const revealRequests = createStore<{ path: string | undefined; seq: number }>()(() => ({
  path: undefined,
  seq: 0,
}));

/** 프로젝트 툴 윈도우를 띄우고 `path` 를 트리에서 고르게 한다. */
export function selectInProjectView(api: DockviewApi | undefined, path: string) {
  if (api) showPanel(api, PROJECT_TOOL_WINDOW);
  revealRequests.setState((s) => ({ path, seq: s.seq + 1 }));
}

/** 「다음에서 열기」 팝업의 「프로젝트 뷰」: 대상 파일(활성 탭)을 트리에서 고른다. */
export function projectViewPlace(
  strings: FileTreeStrings,
  model: FileTreeModel,
  api: () => DockviewApi | undefined,
): OpenWithPlace {
  return {
    id: "projectView",
    label: strings.projectView.label,
    problem: (target) => {
      if (!target.file) return strings.projectView.noFile;
      if (!model.contains(target.file)) return strings.projectView.outside;
      return undefined;
    },
    run: (target) => {
      if (target.file) selectInProjectView(api(), target.file);
    },
  };
}

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** 창의 트리 루트: 활성 워크트리 안의 프로젝트 루트, 없으면 등록 루트. 알게 되면 `onRoot`. 프로젝트 이름을 돌려준다. */
function useTreeRoot(
  client: RpcClient | undefined,
  projectId: string,
  worktree: string | undefined,
  onRoot: (root: string | undefined) => void,
): string | undefined {
  const project = useRpcQuery("project.get", { id: projectId });
  const registered = project.data?.roots[0];
  const latest = useRef(onRoot);
  latest.current = onRoot;
  useEffect(() => {
    if (worktree === undefined) {
      if (registered !== undefined) latest.current(registered);
      return;
    }
    if (!client) return;
    let current = true;
    client
      .call("project.resolve", { path: worktree })
      .then((r) => current && latest.current(r?.worktree?.root ?? r?.root))
      .catch((e: unknown) =>
        console.warn("[tree] the worktree root is unknown", worktree, errorText(e)),
      );
    return () => {
      current = false;
    };
  }, [client, worktree, registered]);
  return project.data?.name;
}

/**
 * 창의 파일 트리를 Host 에 잇고(루트 · `fs.changed`), 「다음에서 열기」 팝업의 「프로젝트 뷰」 항목과 트리 액션을 돌려준다. 창이 사는
 * 동안 한 번 부른다(프로젝트 창의 `ProjectOpenWith`).
 */
export function useProjectView({
  projectId,
  workspace,
}: {
  projectId: string;
  workspace: EditorWorkspace;
}): { places: OpenWithPlace[]; actions: Action[] } {
  const strings = useFileTreeStrings();
  const { client } = useHost();
  const worktree = useActiveWorktree();
  const model = windowFileTree();

  const name = useTreeRoot(client, projectId, worktree, (root) => {
    treeScope.setState({ worktree });
    void model.setRoot(root);
  });
  useEffect(() => treeScope.setState({ client, projectId, name }), [client, projectId, name]);

  useEffect(() => {
    if (!client) return;
    let disposed = false;
    let unsubscribe: (() => Promise<void>) | undefined;
    client
      .subscribe({ projectId, topics: ["fs.changed"] }, (event) => {
        if (!disposed) void model.applyChange(event.payload as unknown as FsChanged);
      })
      .then((subscription) => {
        unsubscribe = () => subscription.unsubscribe();
        if (disposed) void unsubscribe().catch(() => {});
        else void model.refresh();
      })
      .catch((e: unknown) => console.warn("[tree] fs.changed subscription failed", e));
    return () => {
      disposed = true;
      void unsubscribe?.().catch(() => {});
    };
  }, [client, projectId, model]);

  const places = useMemo(
    () => [projectViewPlace(strings, model, () => workspace.api())],
    [strings, model, workspace],
  );
  const actions = useMemo(
    (): Action[] => [
      {
        id: "fileTree.selectIn",
        label: strings.actions.selectIn,
        keywords: ["Select in Project View", "Select Opened File", "프로젝트 뷰"],
        enabled: () => {
          const file = workspace.activeContext()?.file;
          return file !== undefined && model.contains(file);
        },
        run: () => {
          const file = workspace.activeContext()?.file;
          if (file) selectInProjectView(workspace.api(), file);
        },
      },
      {
        id: "fileTree.toggleIgnored",
        label: strings.actions.toggleIgnored,
        keywords: ["Show Excluded Files", "Show Ignored Files", "gitignore"],
        run: () => model.setIgnoredMode(model.ignoredMode() === "hide" ? "dim" : "hide"),
      },
    ],
    [strings, model, workspace],
  );
  return { places, actions };
}
