// 커밋 툴 윈도우의 diff 미리보기(docs/design/13-m2-plan.md M2-16): 파일 하나의 작업 트리 diff(index ↔ 작업 트리) · 스테이징된
// diff(HEAD ↔ index) 문서에 hunk · 줄 체크(hunk-staging.ts)를 단다. git 문서 탭(documents/document-panel.tsx)이 `diff` 문서 중 이
// 둘을 여기로 넘긴다.
//
// - 다시 읽기: 체크로 쓴 뒤 곧바로, 그리고 창의 git 상태에서 이 경로의 항목이 다시 읽힐 때마다(터미널 · 다른 창의 `git add`, 저장한
//   편집 — `windowGitStatus().entry`). 감시를 더 열지 않는다.
// - 쓰기는 커밋 툴 윈도우와 같은 `CommitOps`(commit-feature.tsx 가 건다)로 그 탭의 워크트리에 한다. 쓰는 동안에는 다른 체크를 받지
//   않는다(앞 쓰기가 끝나고 다시 읽은 hunk 로 다음을 고른다).

import { useCallback, useMemo, useRef, useState, useSyncExternalStore } from "react";

import { ChangesView } from "../documents/changes-view";
import type { GitDocumentSpec } from "../documents/spec";
import { windowGitStatus } from "../git-status";
import { commitFeature } from "./commit-state";
import { type StageSelection, stagingSlots } from "./hunk-staging";
import { useGitCommitStrings } from "./strings";

type DiffSpec = Extract<GitDocumentSpec, { kind: "diff" }>;

/** 체크를 달 수 있는 diff 문서인가(작업 트리 · 스테이징된 diff). */
export function isStagingDiff(spec: DiffSpec): boolean {
  return spec.from === undefined && (spec.to.kind === "workTree" || spec.to.kind === "index");
}

export function StagingChanges({
  spec,
  worktree,
}: {
  spec: DiffSpec;
  worktree: string | undefined;
}) {
  const T = useGitCommitStrings();
  const feature = commitFeature((s) => s.current);
  const store = windowGitStatus();
  const entry = useSyncExternalStore(
    useCallback((l) => store.subscribe(l), [store]),
    () => store.entry(spec.path),
  );
  const [written, setWritten] = useState(0);
  const busy = useRef(false);
  const mode = spec.to.kind === "workTree" ? "stage" : "unstage";

  const slots = useMemo(() => {
    if (!feature) return undefined;
    const write = async (job: () => Promise<unknown>) => {
      if (busy.current) return;
      busy.current = true;
      try {
        await job();
      } finally {
        busy.current = false;
        setWritten((n) => n + 1);
      }
    };
    const slot = stagingSlots({
      mode,
      toggle: (selection: StageSelection) =>
        void write(() =>
          feature.ops.stageLines(mode, spec.path, spec.oldPath, selection, worktree),
        ),
      ...(mode === "stage" && {
        revert: (selection: StageSelection) =>
          void write(() => feature.ops.discardLines(spec.path, selection, worktree)),
      }),
      labels:
        mode === "stage"
          ? { group: T.hunk.stage, line: T.hunk.stageLine, revert: T.hunk.revert }
          : { group: T.hunk.unstage, line: T.hunk.unstageLine, revert: T.hunk.revert },
    });
    // 공백 무시로 받은 hunk 는 실제 줄과 맞지 않는다 — 칸을 달지 않는다.
    return (ignoreWhitespace: boolean) => (ignoreWhitespace ? undefined : slot);
  }, [feature, mode, spec.path, spec.oldPath, worktree, T]);

  const reloadKey = useMemo(() => [entry, written], [entry, written]);
  return (
    <ChangesView
      from={undefined}
      to={spec.to}
      worktree={worktree}
      paths={[spec.path, ...(spec.oldPath ? [spec.oldPath] : [])]}
      tree={false}
      slots={slots}
      reloadKey={reloadKey}
    />
  );
}
