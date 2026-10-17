// 액션 찾기(⌘⇧A, 10-ui §1 원칙 2). 상단 검색 팝업(ui/search-popup.tsx)에 등록된 액션을 늘어놓는다.

import { Command } from "cmdk";

import { SEARCH_ITEM_CLASS, SearchPopup } from "../ui/search-popup";
import { useKeymap, useRegistryVersion } from "./keymap-provider";

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
        .filter((a) => registry.isEnabled(a.id) && !a.popup && a.id !== "action.find")
        .sort((a, b) => a.label.localeCompare(b.label, "ko"))
    : [];

  return (
    <SearchPopup
      open={open}
      onOpenChange={onOpenChange}
      label="액션 찾기"
      placeholder="액션 이름"
      empty="맞는 액션이 없다"
    >
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
            className={SEARCH_ITEM_CLASS}
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
    </SearchPopup>
  );
}
