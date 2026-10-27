// 두 리비전 비교(`git.compareCommits { a, b }`, 결정 M2-D5 의 F5-4)를 부르는 좁은 도우미. 비교 화면 · 그 액션의 주인은 M2-15 다 — 이
// 행은 액션 id 로만 부르고 그 코드를 가져오지 않는다.
//
// 그 액션은 인자가 있어야만 돈다(`args: true`). 그래서 `isEnabled` 는 늘 거짓이다 — 보일지는 `isAvailable` 로 정한다.

import type { ActionRegistry } from "@pitwall/keymap";

export const COMPARE_COMMITS_ACTION = "git.compareCommits";

export interface CompareCommitsArgs {
  /** 기준 리비전(지금 HEAD 쪽). */
  readonly a: string;
  /** 견줄 리비전(고른 브랜치 · 태그). */
  readonly b: string;
}

/** 비교 액션이 이 창에 있고 켜져 있는가. */
export function canCompareCommits(registry: ActionRegistry): boolean {
  return registry.isAvailable(COMPARE_COMMITS_ACTION);
}

/** 비교 화면을 연다. 액션이 없거나 꺼져 있으면 `false`. */
export function compareCommits(registry: ActionRegistry, args: CompareCommitsArgs): boolean {
  return registry.run(COMPARE_COMMITS_ACTION, args);
}
