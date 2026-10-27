// 두 리비전 비교(`git.compareCommits { a, b }`, 결정 M2-D5 의 F5-4)를 부르는 좁은 도우미. 비교 화면 · 그 액션의 주인은 M2-15 다 — 이
// 행은 액션 id 로만 부르고 그 코드를 가져오지 않는다.
//
// 지금 액션 레지스트리의 `run(id)` 은 인자를 넘기지 못한다. 인자를 받는 `run(id, args)` 는 M2-15 가 packages/keymap 의 registry.ts 에
// 더한다 — 그때까지는 등록된 액션의 `run` 에 인자를 직접 넘긴다. M2-15 가 병합되면 이 파일을 `registry.run(id, args)` 한 줄로 바꾼다.

import type { ActionRegistry } from "@pitwall/keymap";

export const COMPARE_COMMITS_ACTION = "git.compareCommits";

export interface CompareCommitsArgs {
  /** 기준 리비전(지금 HEAD 쪽). */
  readonly a: string;
  /** 견줄 리비전(고른 브랜치 · 태그). */
  readonly b: string;
}

/** 비교 액션이 이 창에 있는가. */
export function canCompareCommits(registry: ActionRegistry): boolean {
  return registry.isEnabled(COMPARE_COMMITS_ACTION);
}

/** 비교 화면을 연다. 액션이 없거나 꺼져 있으면 `false`. */
export function compareCommits(registry: ActionRegistry, args: CompareCommitsArgs): boolean {
  const action = registry.get(COMPARE_COMMITS_ACTION);
  if (!action || !registry.isEnabled(COMPARE_COMMITS_ACTION)) return false;
  const run = action.run as (args: CompareCommitsArgs) => void | Promise<void>;
  try {
    void Promise.resolve(run(args)).catch((e: unknown) =>
      console.error("[git] compare failed", COMPARE_COMMITS_ACTION, e),
    );
  } catch (e) {
    console.error("[git] compare failed", COMPARE_COMMITS_ACTION, e);
  }
  return true;
}
