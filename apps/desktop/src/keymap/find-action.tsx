// 액션 찾기(⌘⇧A, 10-ui §1 원칙 2). cmdk 의 팝업이다. 비교는 자모 단위라 한글을 조합하는 동안에도 목록이 맞는다
// (packages/keymap hangul.ts). 조합을 확정하는 Enter 는 cmdk 가 고르지 않는다(isComposing · keyCode 229 를 거른다).

import { matchScore } from "@pitwall/keymap";
import { Command } from "cmdk";

import { useKeymap, useRegistryVersion } from "./keymap-provider";

function filter(value: string, search: string, keywords?: string[]): number {
  return Math.max(matchScore(value, search), ...(keywords ?? []).map((k) => matchScore(k, search)));
}

export function FindActionDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange(open: boolean): void;
}) {
  const { registry, shortcuts } = useKeymap();
  useRegistryVersion(registry);
  const actions = open
    ? registry
        .list()
        .filter((a) => registry.isEnabled(a.id) && a.id !== "action.find")
        .sort((a, b) => a.label.localeCompare(b.label, "ko"))
    : [];

  return (
    <Command.Dialog
      open={open}
      onOpenChange={onOpenChange}
      label="액션 찾기"
      filter={filter}
      loop
      overlayClassName="fixed inset-0 bg-black/20"
      contentClassName="fixed top-[15%] left-1/2 w-[520px] max-w-[90vw] -translate-x-1/2 overflow-hidden rounded-md border border-ui-border bg-ui-panel text-ui-foreground shadow-lg"
    >
      <Command.Input
        autoFocus
        placeholder="액션 이름"
        className="w-full border-b border-ui-border bg-transparent px-3 py-2 outline-none placeholder:text-ui-muted-foreground"
      />
      <Command.List className="max-h-[320px] overflow-y-auto py-1">
        <Command.Empty className="px-3 py-2 text-ui-muted-foreground">
          맞는 액션이 없다
        </Command.Empty>
        {actions.map((action) => {
          const keys = shortcuts(action.id);
          return (
            <Command.Item
              key={action.id}
              value={action.id}
              keywords={[action.label, ...(action.keywords ?? [])]}
              onSelect={() => {
                onOpenChange(false);
                // 팝업이 닫히며 포커스를 이전 자리로 돌려놓은 뒤에 실행한다 — 액션이 옮긴 포커스를 덮지 않게.
                setTimeout(() => registry.run(action.id), 0);
              }}
              className="flex h-[26px] cursor-default items-center gap-3 px-3 data-[selected=true]:bg-ui-active"
            >
              <span className="flex-1 truncate">{action.label}</span>
              {keys.map((k) => (
                <kbd
                  key={k.text}
                  title={k.reserved ? `OS 예약 키(${k.reserved}) — 앱에 오지 않는다` : undefined}
                  className={`font-sans text-[11px] text-ui-muted-foreground ${k.reserved ? "line-through" : ""}`}
                >
                  {k.text}
                </kbd>
              ))}
            </Command.Item>
          );
        })}
      </Command.List>
    </Command.Dialog>
  );
}
