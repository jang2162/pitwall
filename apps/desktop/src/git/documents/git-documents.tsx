// 프로젝트 창의 git 문서(spec.ts): 이 창의 Host 연결로 읽는 길(source.ts)을 걸고, 문서를 여는 액션을 등록한다(13-m2-plan M2-15 —
// 행 사이 이음매). 다른 행은 이 액션을 id 로 부른다 — Git 로그(M2-14)의 더블클릭 · 비교, 상태바(M2-13) 등.
//
//   registry.run("git.showCommit", { rev })            커밋 상세 탭
//   registry.run("git.compareCommits", { a, b })       두 커밋 비교 탭(a → b)
//   registry.run("git.openRevisionFile", { rev, path }) 리비전의 파일(읽기 전용) 탭. `path` 는 저장소 꼭대기 기준(`/`)
//
// 셋 다 인자가 있어야만 돈다(`args: true` — 키 · 메뉴 · 액션 찾기에서는 부를 수 없다, packages/keymap registry.ts). 인자의 꼴이
// 틀리면(빈 문자열 · 문자열이 아님) 아무것도 열지 않고 경고 알림을 띄운다. 리비전 문법(`HEAD~1` · 브랜치 이름)은 받고, 열기 전에
// `git.show` 로 커밋 해시로 푼다 — 탭은 그 해시를 가리킨다. 풀지 못하면(없는 리비전 · 저장소 없음) 오류 알림이다.
// 탭은 액션을 부른 때의 활성 워크트리로 열린다(그 워크트리의 저장소를 읽는다).
//
// `diff.nextChange` · `diff.previousChange`(`F7` · `⇧F7`, `when = "diffFocus"`)는 포커스가 든 diff 뷰의 다음 · 앞 차이로 간다.

import type { Action } from "@pitwall/keymap";
import { useEffect, useMemo, useRef } from "react";

import { openTab } from "../../editor/editor-tabs";
import type { EditorWorkspace } from "../../editor/editor-workspace";
import { useRegisterActions } from "../../keymap/keymap-provider";
import { useProjectStrings } from "../../layout/strings";
import { useNotify } from "../../notifications/notifications";
import { activeWorktree } from "../../open-with/active-worktree";
import { useHost } from "../../rpc/host";
import { activeDiff } from "../diff/diff-view";
import { type GitDocumentSource, gitDocumentSource } from "./source";
import { type GitDocumentSpec, gitDocumentKey } from "./spec";

const short = (hash: string) => hash.slice(0, 7);

const isText = (v: unknown): v is string => typeof v === "string" && v !== "";

function field(args: unknown, name: string): string | undefined {
  if (typeof args !== "object" || args === null) return undefined;
  const v = (args as Record<string, unknown>)[name];
  return isText(v) ? v : undefined;
}

/** 탭 이름. */
export function gitDocumentTitle(
  spec: GitDocumentSpec,
  T: ReturnType<typeof useProjectStrings>["git"]["documents"],
): string {
  switch (spec.kind) {
    case "commit":
      return T.commitTitle(short(spec.rev));
    case "compare":
      return T.compareTitle(short(spec.a), short(spec.b));
    case "revision":
      return T.revisionTitle(spec.path.split("/").pop() ?? spec.path, short(spec.rev));
    case "diff":
    // 머지 문서는 여는 쪽(src/git/merge/git-merge.tsx)이 「이름 (머지)」 로 이름을 단다.
    case "merge":
      return spec.path.split("/").pop() ?? spec.path;
  }
}

/**
 * git 문서 탭을 연다(이미 있으면 그 탭을 앞으로). 열린 탭을 돌려준다. `preview` 면 미리보기 탭(다음 미리보기가 대신한다 — 커밋 툴
 * 윈도우의 diff 미리보기, M2-16).
 */
export function openGitDocument(
  workspace: EditorWorkspace,
  spec: GitDocumentSpec,
  title: string,
  worktree: string | undefined,
  preview = false,
) {
  const api = workspace.api();
  if (!api) return undefined;
  const { panel } = openTab(api, {
    path: gitDocumentKey(spec),
    worktree,
    git: spec,
    title,
    preview,
    recent: workspace.recent(),
  });
  panel.api.setActive();
  return panel;
}

/** 이 창의 Host 연결로 읽는 길. */
function useHostSource(projectId: string): GitDocumentSource {
  const { client } = useHost();
  const latest = useRef(client);
  latest.current = client;
  return useMemo(() => {
    const host = () => {
      const c = latest.current;
      if (!c) throw new Error("Host is not connected");
      return c;
    };
    const scope = (worktree: string | undefined) => ({ projectId, worktree: worktree ?? null });
    return {
      show: (rev, worktree) => host().call("git.show", { ...scope(worktree), rev }),
      diff: (params, worktree) => host().call("git.diff", { ...scope(worktree), ...params }),
      blob: (params, worktree) => host().call("git.blob", { ...scope(worktree), ...params }),
      readFile: (path, worktree) => host().call("fs.read", { ...scope(worktree), path }),
      async workTree(worktree) {
        const { repo } = await host().call("git.repo", scope(worktree));
        if (!repo) throw new Error("no repository");
        return repo.workTree;
      },
      async detect(path, head, worktree) {
        const { files } = await host().call("lang.detect", {
          ...scope(worktree),
          files: [{ path, head }],
        });
        return files[0]?.language ?? null;
      },
      syntax: {
        open: (params) => host().call("syntax.open", params),
        highlights: (params) => host().call("syntax.highlights", params),
        close: (params) => host().call("syntax.close", params),
      },
    } satisfies GitDocumentSource;
  }, [projectId]);
}

/** 포커스가 든 diff 뷰. 뷰가 포커스를 잃었으면(다른 곳을 누름) 없다. */
function focusedDiff() {
  const c = activeDiff.getState().current;
  const active = document.activeElement;
  return c && active && c.views().some((v) => v.dom.contains(active)) ? c : undefined;
}

/** git 문서 액션들. `source` · `open` · `fail` 은 시험이 가짜를 넘긴다. */
export function gitDocumentActions(deps: {
  source: () => GitDocumentSource | undefined;
  open(spec: GitDocumentSpec, title: string, worktree: string | undefined): void;
  strings: ReturnType<typeof useProjectStrings>["git"]["documents"];
  /** 실패를 알린다(`level` 이 `warning` 이면 인자가 틀림). */
  fail(level: "error" | "warning", title: string, body?: string): void;
}): Action[] {
  const T = deps.strings;
  const resolve = async (rev: string, worktree: string | undefined) =>
    (await deps.source()!.show(rev, worktree)).commit;
  const run =
    (
      id: string,
      label: string,
      open: (args: unknown, worktree: string | undefined) => Promise<boolean>,
    ) =>
    async (args: unknown) => {
      const worktree = activeWorktree();
      try {
        if (!(await open(args, worktree))) deps.fail("warning", T.badArgs(label));
      } catch (e) {
        deps.fail("error", T.failed(e instanceof Error ? e.message : String(e)));
        console.warn(`[git] ${id} failed`, e);
      }
    };
  const ready = () => deps.source() !== undefined;
  return [
    {
      id: "git.showCommit",
      label: T.actions.showCommit,
      args: true,
      enabled: ready,
      run: run("git.showCommit", T.actions.showCommit, async (args, worktree) => {
        const rev = field(args, "rev");
        if (!rev) return false;
        const spec: GitDocumentSpec = { kind: "commit", rev: await resolve(rev, worktree) };
        deps.open(spec, T.commitTitle(short(spec.rev)), worktree);
        return true;
      }),
    },
    {
      id: "git.compareCommits",
      label: T.actions.compareCommits,
      args: true,
      enabled: ready,
      run: run("git.compareCommits", T.actions.compareCommits, async (args, worktree) => {
        const a = field(args, "a");
        const b = field(args, "b");
        if (!a || !b) return false;
        const [ra, rb] = await Promise.all([resolve(a, worktree), resolve(b, worktree)]);
        const spec: GitDocumentSpec = { kind: "compare", a: ra, b: rb };
        deps.open(spec, T.compareTitle(short(ra), short(rb)), worktree);
        return true;
      }),
    },
    {
      id: "git.openRevisionFile",
      label: T.actions.openRevisionFile,
      args: true,
      enabled: ready,
      run: run("git.openRevisionFile", T.actions.openRevisionFile, async (args, worktree) => {
        const rev = field(args, "rev");
        const path = field(args, "path");
        if (!rev || !path || path.startsWith("/")) return false;
        const spec: GitDocumentSpec = { kind: "revision", rev: await resolve(rev, worktree), path };
        deps.open(spec, gitDocumentTitle(spec, T), worktree);
        return true;
      }),
    },
    {
      id: "diff.nextChange",
      label: T.actions.nextChange,
      keywords: ["Next Difference", "Compare Next Change"],
      enabled: () => focusedDiff() !== undefined,
      run: () => void focusedDiff()?.go(1),
    },
    {
      id: "diff.previousChange",
      label: T.actions.previousChange,
      keywords: ["Previous Difference", "Compare Previous Change"],
      enabled: () => focusedDiff() !== undefined,
      run: () => void focusedDiff()?.go(-1),
    },
  ];
}

/** 프로젝트 창에 한 번 둔다: 읽는 길을 걸고 액션을 등록한다. */
export function GitDocuments({
  projectId,
  workspace,
}: {
  projectId: string;
  workspace: EditorWorkspace;
}) {
  const T = useProjectStrings().git.documents;
  const notify = useNotify();
  const source = useHostSource(projectId);
  useEffect(() => {
    gitDocumentSource.setState({ source });
    return () => {
      if (gitDocumentSource.getState().source === source)
        gitDocumentSource.setState({ source: undefined });
    };
  }, [source]);
  const notifyRef = useRef(notify);
  notifyRef.current = notify;
  const actions = useMemo(
    () =>
      gitDocumentActions({
        source: () => gitDocumentSource.getState().source,
        open: (spec, title, worktree) => openGitDocument(workspace, spec, title, worktree),
        strings: T,
        fail: (level, title, body) => notifyRef.current({ level, title, ...(body && { body }) }),
      }),
    [T, workspace],
  );
  useRegisterActions(actions);
  return null;
}
