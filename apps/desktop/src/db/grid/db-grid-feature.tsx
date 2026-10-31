// 결과 그리드의 창 단위 액션(M3-16): `db.grid.copyAs`(⌘⇧C, Win/Linux Ctrl+Shift+C — 키맵 `when = "dbGridFocus"`). 창에 한 번
// 등록하고, 키를 받은 순간 포커스가 든 그리드(grid-focus.ts)에 「다른 형식으로 복사」 를 연다. 프로젝트 창이 그린다.

import type { Action } from "@pitwall/keymap";
import { useMemo } from "react";

import { useRegisterActions } from "../../keymap/keymap-provider";
import { focusedGrid } from "./grid-focus";
import { type DbGridStrings, useDbGridStrings } from "./strings";

export function gridActions(T: DbGridStrings): Action[] {
  return [
    {
      id: "db.grid.copyAs",
      label: T.copy.copyAsTitle,
      keywords: ["Copy As", "CSV", "JSON", "INSERT", "TSV", "Grid"],
      enabled: () => focusedGrid() !== undefined,
      run: () => focusedGrid()?.openCopyAs(),
    },
  ];
}

export function DbGridFeature() {
  const T = useDbGridStrings();
  useRegisterActions(useMemo(() => gridActions(T), [T]));
  return null;
}
