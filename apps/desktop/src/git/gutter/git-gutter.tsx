// 프로젝트 창의 거터 · blame(13-m2-plan M2-17): 이 창의 Host 연결 · git 상태 · 브랜치 상태로 호스트(gutter-host.ts)를 만들어 창
// 저장소(`gitGutterHost`)에 걸고 액션을 등록한다. 텍스트 뷰어는 저장소의 호스트를 에디터 확장의 context(`git`)로 넘긴다.
//
//   git.nextChange · git.previousChange   ⌃⌥⇧↓ / ↑ — 활성 탭 뷰의 다음 · 앞 변경으로(`when = "editorFocus"`)
//   git.toggleBlame                       ⌃⌥A — 활성 탭 뷰의 blame 주석 칸
//   registry.run("git.blameRevision", { rev, path })  리비전의 파일을 blame 주석과 함께(blame 문서 탭). `path` 는 저장소 꼭대기 기준
//
// 감시를 더 열지 않는다: HEAD 는 창의 브랜치 저장소(`windowBranch`), 파일 상태는 창의 git 상태 저장소(`windowGitStatus`)를 듣는다.

import type { Action } from "@pitwall/keymap";
import type { RpcClient } from "@pitwall/rpc-client";
import { useEffect, useMemo, useRef } from "react";

import type { EditorWorkspace } from "../../editor/editor-workspace";
import { useI18n } from "../../i18n/i18n-provider";
import { useKeymap, useRegisterActions } from "../../keymap/keymap-provider";
import { useProjectStrings } from "../../layout/strings";
import { useNotify } from "../../notifications/notifications";
import { activeWorktree } from "../../open-with/active-worktree";
import { useHost } from "../../rpc/host";
import { type BranchStore, windowBranch } from "../branch-state";
import { openTab } from "../../editor/editor-tabs";
import { type GitDocumentSpec, gitDocumentKey } from "../documents/spec";
import { type GitStatusStore, windowGitStatus } from "../git-status";
import { toggleAnnotations } from "./blame-extension";
import { goToChange } from "./gutter-extension";
import { BLAME_REVISION_ACTION, GutterHost } from "./gutter-host";
import { gitGutterHost } from "./host-store";
import type { GutterSource } from "./gutter-model";
import { type GitGutterStrings, useGitGutterStrings } from "./strings";

/** Host RPC · 창의 git 상태 · 브랜치 상태로 만든 소스. */
export function hostGutterSource(
  client: () => RpcClient,
  projectId: string,
  status: GitStatusStore,
  branch: BranchStore,
): GutterSource {
  const scope = () => ({ projectId, worktree: activeWorktree() ?? null });
  return {
    relative: (path) => status.relative(path),
    letter: (path) => status.decoration(path)?.letter,
    head() {
      const head = branch.get().refs?.head;
      if (head === undefined) return undefined;
      if (head === null || head.kind === "unborn") return null;
      return head.commit;
    },
    subscribe(listener) {
      const a = status.subscribe(listener);
      const b = branch.subscribe(listener);
      return () => {
        a();
        b();
      };
    },
    blob: (params) => client().call("git.blob", { ...scope(), ...params }),
    blame: (params, signal) => client().stream("git.blame", { ...scope(), ...params }, { signal }),
    async workTreeDiff(path) {
      const diff = await client().call("git.diff", {
        ...scope(),
        from: { kind: "index" },
        to: { kind: "workTree" },
        paths: [path],
      });
      return diff.files.find((f) => f.path === path);
    },
    async stageLines(path, hunks) {
      await client().call("git.stageLines", { ...scope(), path, hunks });
    },
  };
}

/** 거터 · blame 액션. `view` 는 활성 탭의 텍스트 뷰(없으면 아무것도 하지 않는다). */
export function gitGutterActions(deps: {
  view(): import("@codemirror/view").EditorView | undefined;
  strings: GitGutterStrings;
  /** blame 문서 탭을 연다. */
  openBlame(rev: string, path: string): void;
}): Action[] {
  const T = deps.strings.actions;
  const isText = (v: unknown): v is string => typeof v === "string" && v !== "";
  return [
    {
      id: "git.nextChange",
      label: T.nextChange,
      keywords: ["Next Change", "VCS"],
      enabled: () => deps.view() !== undefined,
      run: () => {
        const view = deps.view();
        if (view) goToChange(view, 1);
      },
    },
    {
      id: "git.previousChange",
      label: T.previousChange,
      keywords: ["Previous Change", "VCS"],
      enabled: () => deps.view() !== undefined,
      run: () => {
        const view = deps.view();
        if (view) goToChange(view, -1);
      },
    },
    {
      id: "git.toggleBlame",
      label: T.toggleBlame,
      keywords: ["Annotate", "Blame", "Annotate with Git Blame"],
      enabled: () => deps.view() !== undefined,
      run: () => {
        const view = deps.view();
        if (view) toggleAnnotations(view);
      },
    },
    {
      id: BLAME_REVISION_ACTION,
      label: T.blameRevision,
      args: true,
      run: (args) => {
        const a = (typeof args === "object" && args !== null ? args : {}) as Record<
          string,
          unknown
        >;
        if (!isText(a.rev) || !isText(a.path) || a.path.startsWith("/")) return;
        deps.openBlame(a.rev, a.path);
      },
    },
  ];
}

/** 프로젝트 창에 한 번 둔다. */
export function GitGutter({
  projectId,
  workspace,
}: {
  projectId: string;
  workspace: EditorWorkspace;
}) {
  const T = useGitGutterStrings();
  const D = useProjectStrings().git.documents;
  const { client } = useHost();
  const { registry } = useKeymap();
  const notify = useNotify();
  const { locales } = useI18n();
  const latest = useRef(client);
  latest.current = client;
  const notifyRef = useRef(notify);
  notifyRef.current = notify;
  const locale = locales[0];

  const host = useMemo(() => {
    const rpc = () => {
      const c = latest.current;
      if (!c) throw new Error("Host is not connected");
      return c;
    };
    return new GutterHost({
      source: hostGutterSource(rpc, projectId, windowGitStatus(), windowBranch()),
      strings: T,
      available: (id) => registry.isAvailable(id),
      run: (id, args) => registry.run(id, args),
      async versions(path) {
        const doc = workspace.documents.get(path);
        const document = doc?.status === "text" ? doc.version : null;
        const read = await rpc().call("fs.read", {
          projectId,
          worktree: activeWorktree() ?? null,
          path,
        });
        return { document, disk: read.version };
      },
      notify: (level, title, body) => notifyRef.current({ level, title, ...(body && { body }) }),
      locale,
    });
  }, [T, projectId, registry, workspace, locale]);

  useEffect(() => {
    gitGutterHost.setState({ host });
    return () => {
      if (gitGutterHost.getState().host === host) gitGutterHost.setState({ host: undefined });
    };
  }, [host]);

  const actions = useMemo(
    () =>
      gitGutterActions({
        view: () => {
          const panel = workspace.activeTab();
          return panel ? workspace.view(panel.id) : undefined;
        },
        strings: T,
        openBlame: (rev, path) => {
          // git 문서 탭 열기(src/git/documents/git-documents.tsx `openGitDocument` 와 같다 — 그 모듈은 diff 뷰를 끌고 와 host-store.ts
          // 머리 주석의 순환을 만든다).
          const api = workspace.api();
          if (!api) return;
          const spec: GitDocumentSpec = { kind: "blame", rev, path };
          const { panel } = openTab(api, {
            path: gitDocumentKey(spec),
            worktree: activeWorktree(),
            git: spec,
            title: D.blameTitle(path.split("/").pop() ?? path, rev.slice(0, 7)),
            recent: workspace.recent(),
          });
          panel.api.setActive();
        },
      }),
    [T, D, workspace],
  );
  useRegisterActions(actions);
  return null;
}
