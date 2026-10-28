// 진행 중 작업 배너(13-m2-plan M2-19, 10-ui.md 「Git」): merge · rebase · cherry-pick · revert 가 충돌로 멈춰 있는 동안 종류 · 단계 ·
// 남은 단계 · 충돌 파일 수와 [충돌 해결…] [계속] [건너뛰기] [중단] 을 한 줄로 보인다. 에디터 영역 위(프로젝트 창)와 커밋 툴 윈도우
// (M2-16 이 비워 둔 줄)에 같은 부품을 단다.
//
// - 읽기는 창 공용 저장소다: 진행 중 작업은 브랜치 상태(branch-state.ts 의 `git.operation` — `git.refsChanged` 로 다시 읽는다),
//   충돌 파일 수는 git status(git-status.ts) — 감시를 따로 열지 않는다.
// - 단추는 액션을 id 로 부른다(git-merge.tsx 가 등록한다): `git.resolveConflicts` · `git.continueOperation` · `git.skipOperation` ·
//   `git.abortOperation`. 단추는 그 작업이 할 수 있는 동작(`OperationInfo.actions`)만 보인다. [중단] 의 확인은 액션이 묻는다.

import type { OperationAction, OperationInfo } from "@pitwall/rpc-client";
import { useMemo, useSyncExternalStore } from "react";

import { useKeymap, useRegistryVersion } from "../../keymap/keymap-provider";
import { useBranch } from "../branch-state";
import { windowGitStatus } from "../git-status";
import { type GitMergeStrings, useGitMergeStrings } from "./strings";

/** 단추 → 액션 id. */
export const OPERATION_ACTIONS: Record<OperationAction, string> = {
  continue: "git.continueOperation",
  skip: "git.skipOperation",
  abort: "git.abortOperation",
};

const BUTTON = "rounded-sm border border-ui-border px-2 py-0.5 hover:bg-ui-hover";

/** 창의 git status 에서 충돌 중인 파일 수. */
export function useConflictCount(): number {
  const status = windowGitStatus();
  const version = useSyncExternalStore(
    (l) => status.subscribe(l),
    () => status.getVersion(),
  );
  return useMemo(() => {
    void version;
    return status.entries().filter((e) => e.conflict).length;
  }, [status, version]);
}

/** 배너의 설명 글(종류 · 리베이스 대상 · 단계 · 남은 단계). */
export function operationSummary(T: GitMergeStrings, op: OperationInfo): string[] {
  const parts = [T.banner.title(op.kind)];
  if (op.kind === "rebase" && op.onto)
    parts.push(T.banner.rebase(op.branch ?? "HEAD", op.onto.slice(0, 7)));
  if (op.step) parts.push(T.banner.step(op.step.current, op.step.total));
  else if (op.remaining !== null) parts.push(T.banner.remaining(op.remaining));
  return parts;
}

export function OperationBanner({ placement }: { placement: "editor" | "commit" }) {
  const T = useGitMergeStrings();
  const { operation } = useBranch();
  const conflicts = useConflictCount();
  const { registry } = useKeymap();
  useRegistryVersion(registry);
  if (!operation) return null;
  const run = (id: string) => () => void registry.run(id);
  const has = (id: string) => registry.isAvailable(id);
  return (
    <div
      role="region"
      aria-label={T.banner.label}
      data-testid="git-operation-banner"
      data-placement={placement}
      data-operation={operation.kind}
      className="flex flex-none flex-wrap items-center gap-x-3 gap-y-1 border-b border-ui-border bg-ui-toolbar px-3 py-1 text-[12px] text-ui-foreground"
    >
      <span className="font-semibold">{operationSummary(T, operation).join(" · ")}</span>
      <span
        data-testid="git-operation-conflicts"
        className={conflicts > 0 ? "text-git-conflicted" : "text-ui-muted-foreground"}
      >
        {T.banner.conflicts(conflicts)}
      </span>
      <span className="flex-1" />
      {conflicts > 0 && has("git.resolveConflicts") && (
        <button
          type="button"
          data-git-operation="resolve"
          className={BUTTON}
          onClick={run("git.resolveConflicts")}
        >
          {T.banner.resolve}
        </button>
      )}
      {(["continue", "skip", "abort"] as const)
        .filter((a) => operation.actions.includes(a) && has(OPERATION_ACTIONS[a]))
        .map((a) => (
          <button
            key={a}
            type="button"
            data-git-operation={a}
            className={BUTTON}
            onClick={run(OPERATION_ACTIONS[a])}
          >
            {T.banner[a]}
          </button>
        ))}
    </div>
  );
}
