// M2-15 가 등록하는 git 문서 액션을 부르는 곳(docs/design/13-m2-plan.md 의 행 사이 이음매): 커밋 상세 탭 `git.showCommit { rev }`,
// 두 커밋 비교 탭 `git.compareCommits { a, b }`, 리비전의 파일 읽기 전용 열기 `git.openRevisionFile { rev, path }`. 이 행은 액션
// id 로만 부르고 M2-15 의 코드를 들이지 않는다 — 등록되지 않았으면(M2-15 전) 아무것도 하지 않는다.
//
// TODO(M2-15): 지금 `ActionRegistry.run(id)` 은 인자를 넘기지 못해 등록된 액션의 `run` 을 인자와 함께 직접 부른다. M2-15 가
// packages/keymap registry.ts 에 `Action.run(args?)` · `ActionRegistry.run(id, args?)` 를 더하면 이 파일을 지우고
// `registry.run(id, args)` 로 바꾼다.

import type { ActionRegistry } from "@pitwall/keymap";

export type GitDocumentAction =
  | { id: "git.showCommit"; args: { rev: string } }
  | { id: "git.compareCommits"; args: { a: string; b: string } }
  | { id: "git.openRevisionFile"; args: { rev: string; path: string } };

/** 실행했으면 참. 등록되지 않았거나 실행할 수 없으면 거짓. */
export function runGitAction(registry: ActionRegistry, action: GitDocumentAction): boolean {
  const registered = registry.get(action.id);
  if (!registered || !registry.isEnabled(action.id)) return false;
  const run = registered.run as (args: unknown) => void | Promise<void>;
  try {
    void Promise.resolve(run(action.args)).catch((e: unknown) =>
      console.error("[git] action failed", action.id, e),
    );
  } catch (e) {
    console.error("[git] action failed", action.id, e);
  }
  return true;
}
