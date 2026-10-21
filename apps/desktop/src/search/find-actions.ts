// 전체에서 찾기의 액션. 기본 키(⌘⇧F)는 extensions/keymap-default/keymaps/default.toml. 프로젝트 창이 등록한다(find-in-files.tsx).

import type { Action } from "@pitwall/keymap";

import type { FindStrings } from "./strings";

export function findActions(deps: {
  strings: Pick<FindStrings, "action">;
  open(): void;
}): Action[] {
  return [
    {
      id: "search.findInFiles",
      label: deps.strings.action,
      keywords: ["Find in Files", "Find in Path", "grep", "검색"],
      run: deps.open,
    },
  ];
}
