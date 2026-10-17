// 화면 상단 중앙의 검색 팝업(10-ui §2 S5, S2 「검색 팝업과 같은 컴포넌트」). 액션 찾기(⌘⇧A)와 프로젝트 찾기(S2)가 함께 쓴다.
// cmdk 의 팝업에 앱의 겉모양과 자모 단위 비교를 입힌다 — 한글을 조합하는 동안에도 목록이 맞는다(packages/keymap hangul.ts).
// ↑↓ · Enter · Esc 는 cmdk 가 받는다. 조합을 확정하는 Enter 는 cmdk 가 고르지 않는다(isComposing · keyCode 229 를 거른다).

import { matchScore } from "@pitwall/keymap";
import { Command } from "cmdk";
import type { ReactNode } from "react";

export function searchFilter(value: string, search: string, keywords?: string[]): number {
  return Math.max(matchScore(value, search), ...(keywords ?? []).map((k) => matchScore(k, search)));
}

/** 목록 한 줄. 고른 줄은 cmdk 가 `data-selected` 를 단다. */
export const SEARCH_ITEM_CLASS =
  "flex h-[26px] cursor-default items-center gap-3 px-3 data-[disabled=true]:text-ui-muted-foreground data-[selected=true]:bg-ui-active";

export function SearchPopup({
  open,
  onOpenChange,
  label,
  title,
  placeholder,
  empty,
  children,
}: {
  open: boolean;
  onOpenChange(open: boolean): void;
  /** 화면 읽기 프로그램에 읽히는 이름. */
  label: string;
  /** 입력 칸 위의 제목 줄. 없으면 두지 않는다. */
  title?: ReactNode;
  placeholder: string;
  /** 맞는 줄이 없을 때. */
  empty: ReactNode;
  /** `Command.Item` · `Command.Group` · `Command.Separator`. */
  children: ReactNode;
}) {
  return (
    <Command.Dialog
      open={open}
      onOpenChange={onOpenChange}
      label={label}
      filter={searchFilter}
      loop
      overlayClassName="fixed inset-0 bg-black/20"
      contentClassName="fixed top-[15%] left-1/2 w-[520px] max-w-[90vw] -translate-x-1/2 overflow-hidden rounded-md border border-ui-border bg-ui-panel text-ui-foreground shadow-lg"
    >
      {title !== undefined && (
        <div className="truncate border-b border-ui-border px-3 py-1 text-center text-[12px] text-ui-muted-foreground">
          {title}
        </div>
      )}
      <Command.Input
        autoFocus
        placeholder={placeholder}
        className="w-full border-b border-ui-border bg-transparent px-3 py-2 outline-none placeholder:text-ui-muted-foreground"
      />
      <Command.List className="max-h-[320px] overflow-y-auto py-1">
        <Command.Empty className="px-3 py-2 text-ui-muted-foreground">{empty}</Command.Empty>
        {children}
      </Command.List>
    </Command.Dialog>
  );
}
