// Search Everywhere 의 브랜치 탭(⇧⇧, 결정 M1-D18 의 꼴 — 13-m2-plan.md M2-13). 창의 ref(`windowBranch()`)에서 로컬 · 원격 브랜치와
// 태그를 이름으로 거르고(액션 탭과 같은 자모 단위 비교), 고르면 체크아웃한다 — 브랜치 팝업의 체크아웃과 같은 길(branch-ops.ts)이다.
// 지금 브랜치를 고르면 팝업만 닫는다. 전체 탭에서는 `ALL_TAB_LIMIT` 개까지와 「더 보기」.
// SE 팝업은 KeymapProvider 아래(이 창의 컴포넌트 밖)에 그려지므로 결과 컴포넌트는 클로저로 이 창의 동작을 받는다.

import { Command } from "cmdk";

import {
  GROUP_HEADING_CLASS,
  MoreItem,
  type SearchResultsProps,
  type SearchSource,
} from "../search-everywhere/search-everywhere";
import { SEARCH_ITEM_CLASS, searchFilter } from "../ui/search-popup";
import { type BranchOps, type BranchRef, allRefs, refName, shortName } from "./branch-ops";
import { useBranch } from "./branch-state";
import { useBranchStrings } from "./branch-strings";

const KIND_ORDER: Record<BranchRef["kind"], number> = { local: 0, remote: 1, tag: 2 };

function makeBranchResults(ops: () => BranchOps) {
  return function BranchResults({ query, limit, heading, onMore, choose }: SearchResultsProps) {
    const T = useBranchStrings();
    const { refs } = useBranch();
    if (!refs || refs.workTree === null || refs.unsupported) return null;
    const scored = allRefs(refs)
      .map((ref) => ({ ref, score: searchFilter(shortName(ref), query) }))
      .filter((x) => x.score > 0)
      .sort((a, b) => b.score - a.score || KIND_ORDER[a.ref.kind] - KIND_ORDER[b.ref.kind]);
    const shown = limit === undefined ? scored : scored.slice(0, limit);
    if (shown.length === 0) return null;
    const items = shown.map(({ ref }) => {
      const current = ref.kind === "local" && ref.branch.head;
      const group =
        ref.kind === "local"
          ? T.popup.groups.local
          : ref.kind === "remote"
            ? T.popup.groups.remote
            : T.popup.groups.tags;
      return (
        <Command.Item
          key={refName(ref)}
          value={`branch:${refName(ref)}`}
          data-ref={refName(ref)}
          onSelect={() => choose(() => void (current ? undefined : ops().checkout(ref)))}
          className={SEARCH_ITEM_CLASS}
        >
          <span className={`min-w-0 flex-1 truncate ${current ? "font-semibold" : ""}`}>
            {shortName(ref)}
          </span>
          {current && (
            <span className="flex-none text-[11px] text-ui-muted-foreground">
              {T.popup.current}
            </span>
          )}
          <span className="flex-none text-[11px] text-ui-muted-foreground">{group}</span>
        </Command.Item>
      );
    });
    if (heading === undefined) return <>{items}</>;
    return (
      <Command.Group heading={heading} className={GROUP_HEADING_CLASS}>
        {items}
        {onMore && scored.length > shown.length && <MoreItem id="branches" onMore={onMore} />}
      </Command.Group>
    );
  };
}

/** 브랜치 탭의 결과 묶음. `ops` 는 그때의 창 동작을 돌려준다. */
export function branchSearchSource(ops: () => BranchOps): SearchSource {
  return { id: "branches", Results: makeBranchResults(ops) };
}
